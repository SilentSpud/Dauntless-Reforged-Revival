//! Tells the allowlist helper which addresses belong to logged-in players (`src/feed.ts`). Fire and
//! forget: a player's request never waits for the helper, and a helper that is down only costs log
//! lines.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::json;
use service_kit::linelog::{self, Throttle};

use crate::config::Allowlist;

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Default)]
struct State {
    last_sent: HashMap<String, i64>,
    pending: HashSet<String>,
    /// Failures in a row, and when the helper may be asked again. A helper that is not answering
    /// must not be asked again on every single login: one live host logged 222,875 suppressed
    /// warnings in four hours against two endpoints that were never going to answer (workers that
    /// host was not running). The first answer of any kind clears both.
    failures: u32,
    skip_until: i64,
}

/// A single failure still retries at once (a worker that blips recovers on the next login); from this
/// many failures in a row the helper is left alone for a doubling interval.
const BACKOFF_AFTER: u32 = 3;
const BACKOFF_BASE_MS: i64 = 2_000;
const BACKOFF_MAX_MS: i64 = 60_000;

struct Inner {
    target: String,
    secret: String,
    /// An address the helper accepted is not sent again for this long. The helper keeps an entry
    /// for 10 minutes, and a playing client sends a heartbeat every 20 seconds.
    refresh_ms: i64,
    timeout: Duration,
    client: Client<HttpConnector, Full<Bytes>>,
    warnings: Throttle,
    state: Mutex<State>,
    idle: tokio::sync::Notify,
    sent: AtomicU64,
    now: Clock,
}

#[derive(Clone)]
pub struct AllowlistFeed {
    inner: Arc<Inner>,
    others: Vec<AllowlistFeed>,
}

impl AllowlistFeed {
    pub fn new(options: &Allowlist) -> Self {
        Self::with_clock(options, Arc::new(crate::now_ms))
    }

    pub fn with_clock(options: &Allowlist, now: Clock) -> Self {
        let others = options
            .additional_urls
            .iter()
            .map(|url| {
                Self::with_clock(
                    &Allowlist {
                        url: url.clone(),
                        additional_urls: Vec::new(),
                        ..options.clone()
                    },
                    now.clone(),
                )
            })
            .collect();
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(4)
            // The helper closes idle connections soon; a report a minute needs no warm ones
            .pool_idle_timeout(Duration::from_secs(2))
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build_http();
        Self {
            inner: Arc::new(Inner {
                target: format!("{}/allow", options.url.trim_end_matches('/')),
                secret: options.secret.clone(),
                refresh_ms: options.refresh.as_millis() as i64,
                timeout: options.timeout,
                client,
                warnings: Throttle::new(60_000),
                state: Mutex::default(),
                idle: tokio::sync::Notify::new(),
                sent: AtomicU64::new(0),
                now,
            }),
            others,
        }
    }

    /// Requests sent to the helper, for tests and the status log.
    pub fn sent(&self) -> u64 {
        self.inner.sent.load(Ordering::Relaxed)
    }

    pub fn report(&self, ip: &str) {
        for feed in &self.others {
            feed.report(ip);
        }
        if ip == "unknown" {
            return;
        }
        let inner = &self.inner;
        let now = (inner.now)();
        {
            let mut state = inner.state.lock().unwrap_or_else(PoisonError::into_inner);
            // Backing off from a helper that is down. The address is deliberately not recorded in
            // `last_sent`, so it is offered again as soon as the interval passes
            if now < state.skip_until {
                return;
            }
            if state
                .last_sent
                .get(ip)
                .is_some_and(|last| now - last < inner.refresh_ms)
            {
                return;
            }
            if state.pending.contains(ip) {
                return;
            }
            if state.last_sent.len() > 50_000 {
                state.last_sent.retain(|_, at| now - *at < inner.refresh_ms);
            }
            state.pending.insert(ip.to_owned());
        }
        let inner = self.inner.clone();
        let ip = ip.to_owned();
        tokio::spawn(async move {
            match post(&inner, &ip).await {
                Ok(200) => {
                    inner.answered();
                    inner.mark_sent(&ip);
                    linelog::debug("allowlist refreshed", json!({ "ip": ip }));
                }
                Ok(400) => {
                    inner.answered();
                    // The helper refused this address (a private one, say). Asking again every
                    // heartbeat would not change that, so wait like after a success
                    inner.mark_sent(&ip);
                    if inner.warnings.allow(&format!("refused:{ip}"), (inner.now)()).is_some() {
                        linelog::warn(
                            "allowlist helper refused a player's address; that player's game ports stay closed",
                            json!({ "ip": ip }),
                        );
                    }
                }
                Ok(status) => inner.failed(&format!("status {status}")),
                Err(detail) => inner.failed(&detail),
            }
            let mut state = inner.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.pending.remove(&ip);
            if state.pending.is_empty() {
                inner.idle.notify_waiters();
            }
        });
    }

    /// Resolves when nothing is on its way to the helper (tests and shutdown).
    pub async fn idle(&self) {
        for feed in &self.others {
            Box::pin(feed.idle()).await;
        }
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pending
                .is_empty()
            {
                return;
            }
            notified.await;
        }
    }
}

impl Inner {
    fn mark_sent(&self, ip: &str) {
        let now = (self.now)();
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_sent
            .insert(ip.to_owned(), now);
    }

    /// Any answer, a refusal included, proves the helper is up.
    fn answered(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.failures = 0;
        state.skip_until = 0;
    }

    fn failed(&self, detail: &str) {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.failures += 1;
            if state.failures >= BACKOFF_AFTER {
                let doublings = (state.failures - BACKOFF_AFTER).min(31);
                let wait = BACKOFF_BASE_MS.saturating_mul(1 << doublings).min(BACKOFF_MAX_MS);
                state.skip_until = (self.now)() + wait;
            }
        }
        self.warn_down(detail);
    }

    fn warn_down(&self, detail: &str) {
        if let Some(suppressed) = self.warnings.allow("down", (self.now)()) {
            linelog::warn(
                "allowlist helper did not take an address; players who log in now get no game ports until it answers",
                json!({ "detail": detail, "suppressed": suppressed }),
            );
        }
    }
}

async fn post(inner: &Inner, ip: &str) -> Result<u16, String> {
    let body = json!({ "ip": ip }).to_string();
    inner.sent.fetch_add(1, Ordering::Relaxed);
    let request = http::Request::post(&inner.target)
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .header("x-allowlist-secret", &inner.secret)
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let exchange = async {
        let response = inner.client.request(request).await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        response.into_body().collect().await.map_err(|e| e.to_string())?;
        Ok(status)
    };
    tokio::time::timeout(inner.timeout, exchange)
        .await
        .unwrap_or_else(|_| Err("timed out".into()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU16;

    use super::*;

    /// A helper that counts the addresses it was sent and answers `status`.
    async fn helper(status: Arc<AtomicU16>, count: Arc<AtomicU64>) -> String {
        let handler = move |request: axum::extract::Request| {
            let (status, count) = (status.clone(), count.clone());
            async move {
                assert_eq!(
                    request.headers()["x-allowlist-secret"],
                    "test-secret-test-secret-test-secret"
                );
                let body = axum::body::to_bytes(request.into_body(), 1024).await.unwrap();
                let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert!(parsed["ip"].is_string());
                count.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn a_failed_worker_is_retried_apart_from_the_healthy_main_helper() {
        linelog::set_sink(Some(Box::new(|_, _| {})));
        let main_count = Arc::new(AtomicU64::new(0));
        let worker_count = Arc::new(AtomicU64::new(0));
        let worker_status = Arc::new(AtomicU16::new(503));
        let main = helper(Arc::new(AtomicU16::new(200)), main_count.clone()).await;
        let worker = helper(worker_status.clone(), worker_count.clone()).await;
        let feed = AllowlistFeed::new(&Allowlist {
            url: main,
            secret: "test-secret-test-secret-test-secret".into(),
            refresh: Duration::from_secs(60),
            timeout: Duration::from_secs(1),
            additional_urls: vec![worker],
        });
        feed.report("8.8.8.8");
        feed.idle().await;
        worker_status.store(200, Ordering::SeqCst);
        feed.report("8.8.8.8");
        feed.idle().await;
        feed.report("8.8.8.8");
        feed.idle().await;
        assert_eq!(
            (main_count.load(Ordering::SeqCst), worker_count.load(Ordering::SeqCst)),
            (1, 2)
        );
        feed.report("unknown");
        feed.idle().await;
        assert_eq!(feed.sent(), 1);
    }
    /// A feed on a test clock for one helper (`feed-backoff.test.ts`)
    fn on_clock(url: String) -> (AllowlistFeed, Arc<std::sync::atomic::AtomicI64>) {
        let now = Arc::new(std::sync::atomic::AtomicI64::new(1_000_000));
        let clock = now.clone();
        let feed = AllowlistFeed::with_clock(
            &Allowlist {
                url,
                secret: "test-secret-test-secret-test-secret".into(),
                refresh: Duration::from_secs(60),
                timeout: Duration::from_secs(1),
                additional_urls: Vec::new(),
            },
            Arc::new(move || clock.load(Ordering::SeqCst)),
        );
        (feed, now)
    }

    #[tokio::test]
    async fn a_helper_that_keeps_failing_is_left_alone_instead_of_retried_on_every_login() {
        linelog::set_sink(Some(Box::new(|_, _| {})));
        let count = Arc::new(AtomicU64::new(0));
        let (feed, now) = on_clock(helper(Arc::new(AtomicU16::new(503)), count.clone()).await);
        // Distinct addresses, so the refresh interval never hides the behaviour under test
        for i in 0..12 {
            feed.report(&format!("8.8.8.{i}"));
            feed.idle().await;
        }
        // Three attempts before the backoff engages, and nothing after it does
        assert_eq!(count.load(Ordering::SeqCst), 3, "attempts before backing off");

        // Still inside the first backoff window: nothing is sent
        now.fetch_add(1_500, Ordering::SeqCst);
        feed.report("9.9.9.9");
        feed.idle().await;
        assert_eq!(count.load(Ordering::SeqCst), 3, "sent a request while backing off");

        // Past it: exactly one probe, which fails and extends the window again
        now.fetch_add(1_000, Ordering::SeqCst);
        feed.report("9.9.9.10");
        feed.idle().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            4,
            "did not probe once the backoff elapsed"
        );
    }

    #[tokio::test]
    async fn an_answer_clears_the_backoff_so_a_recovered_helper_is_used_again_at_once() {
        linelog::set_sink(Some(Box::new(|_, _| {})));
        let count = Arc::new(AtomicU64::new(0));
        let status = Arc::new(AtomicU16::new(503));
        let (feed, now) = on_clock(helper(status.clone(), count.clone()).await);
        for i in 0..6 {
            feed.report(&format!("8.8.8.{i}"));
            feed.idle().await;
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);

        status.store(200, Ordering::SeqCst);
        now.fetch_add(2_500, Ordering::SeqCst);
        feed.report("9.9.9.9");
        feed.idle().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            4,
            "did not probe after the backoff elapsed"
        );

        // The helper answered, so the next login goes straight through with no waiting
        feed.report("9.9.9.10");
        feed.idle().await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            5,
            "still backing off after the helper recovered"
        );
    }
}
