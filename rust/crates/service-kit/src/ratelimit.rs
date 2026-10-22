//! Rate limits as `express-rate-limit` 8 applies them in the Node services (MIGRATION_PLAN.md M11).
//!
//! Each key gets its own window that starts at its first request (the memory store's behaviour,
//! not windows aligned to the clock). Every response carries the IETF draft-8 headers
//! `RateLimit: "<name>"; r=<remaining>; t=<seconds>` and
//! `RateLimit-Policy: "<name>"; q=<limit>; w=<window>; pk=:<partition key>:`, the name being
//! `<limit>-in-<window>` such as `60-in-1min`. A request over the limit gets 429, `Retry-After` and
//! the limiter's JSON reply. With `skip_successful`, a request answered below 400 gives its hit
//! back. Keys come from the service ([`Limited::new`]); [`ip_key`] makes them from an address the
//! way `ipKeyGenerator` does (an IPv6 address counts for its /56).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::{HeaderName, HeaderValue, RETRY_AFTER};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

use crate::json::Json;

const SHARDS: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Client {
    hits: u64,
    reset_at: Instant,
}

/// One limiter: a limit per window, counted per key.
pub struct Limiter {
    name: String,
    limit: u64,
    window: Duration,
    skip_successful: bool,
    reply: serde_json::Value,
    hasher: RandomState,
    shards: Vec<Mutex<HashMap<String, Client>>>,
}

/// The state of a key after one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub remaining: u64,
    pub reset_secs: u64,
    pub limited: bool,
    reset_at: Instant,
}

impl Limiter {
    pub fn new(limit: u64, window: Duration, reply: serde_json::Value) -> Self {
        Self {
            name: format!("{limit}-in-{}", duration_name(window)),
            limit,
            window,
            skip_successful: false,
            reply,
            hasher: RandomState::new(),
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
        }
    }

    /// Requests answered with a status below 400 don't count (`skipSuccessfulRequests`).
    pub fn skip_successful(mut self) -> Self {
        self.skip_successful = true;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn shard(&self, key: &str) -> std::sync::MutexGuard<'_, HashMap<String, Client>> {
        let index = (self.hasher.hash_one(key) as usize) % SHARDS;
        // A poisoned shard only means a panic elsewhere mid-update of a counter; keep counting.
        self.shards[index]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Counts one request for `key`.
    pub fn hit(&self, key: &str, now: Instant) -> Decision {
        let mut shard = self.shard(key);
        let client = shard.entry(key.to_owned()).or_insert(Client {
            hits: 0,
            reset_at: now + self.window,
        });
        if client.reset_at <= now {
            *client = Client {
                hits: 0,
                reset_at: now + self.window,
            };
        }
        client.hits += 1;
        let reset_secs = client
            .reset_at
            .saturating_duration_since(now)
            .as_millis()
            .div_ceil(1000) as u64;
        Decision {
            remaining: self.limit.saturating_sub(client.hits),
            reset_secs,
            limited: client.hits > self.limit,
            reset_at: client.reset_at,
        }
    }

    /// Gives a request's hit back, unless its window has already ended.
    pub fn undo(&self, key: &str, decision: &Decision, now: Instant) {
        if now >= decision.reset_at {
            return;
        }
        if let Some(client) = self.shard(key).get_mut(key) {
            client.hits = client.hits.saturating_sub(1);
        }
    }

    /// Forgets keys whose window has ended; run it every window ([`Limiter::spawn_sweeper`]).
    pub fn sweep(&self, now: Instant) {
        for shard in &self.shards {
            shard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|_, client| client.reset_at > now);
        }
    }

    pub fn spawn_sweeper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let limiter = Arc::downgrade(self);
        let period = self.window;
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                match limiter.upgrade() {
                    Some(limiter) => limiter.sweep(Instant::now()),
                    None => return,
                }
            }
        })
    }

    /// The two draft-8 header values for `key` after `decision`.
    pub fn headers(&self, key: &str, decision: &Decision) -> [(HeaderName, String); 2] {
        [
            (
                HeaderName::from_static("ratelimit"),
                format!("\"{}\"; r={}; t={}", self.name, decision.remaining, decision.reset_secs),
            ),
            (
                HeaderName::from_static("ratelimit-policy"),
                format!(
                    "\"{}\"; q={}; w={}; pk=:{}:",
                    self.name,
                    self.limit,
                    self.window.as_millis().div_ceil(1000),
                    partition_key(key)
                ),
            ),
        ]
    }
}

/// `express-rate-limit`'s window name: `30sec`, `1min`, `10min`, `1hr`, `2hrs`, `1day`, `2days`.
fn duration_name(window: Duration) -> String {
    let ms = window.as_millis() as f64;
    let (seconds, minutes, hours, days) = (ms / 1e3, ms / 6e4, ms / 3.6e6, ms / 8.64e7);
    let number = |n: f64| {
        if n.fract() == 0.0 {
            format!("{}", n as u64)
        } else {
            format!("{n}")
        }
    };
    if seconds < 60.0 {
        format!("{}sec", number(seconds))
    } else if minutes < 60.0 {
        format!("{}min", number(minutes))
    } else if hours < 24.0 {
        format!("{}hr{}", number(hours), if hours > 1.0 { "s" } else { "" })
    } else {
        format!("{}day{}", number(days), if days > 1.0 { "s" } else { "" })
    }
}

/// The first 12 hex characters of the key's SHA-256, base64-encoded as text.
fn partition_key(key: &str) -> String {
    let digest: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    BASE64.encode(&digest.as_bytes()[..12])
}

/// The key of a client address, like `ipKeyGenerator`: an IPv4 address (also one mapped into IPv6)
/// as itself, an IPv6 address as its /56 network, anything else unchanged.
pub fn ip_key(address: &str) -> String {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.to_string();
            }
            // The deprecated IPv4-compatible notation (::1.2.3.4) is recognised by its dotted part.
            if address.contains('.')
                && let Some(v4) = v6.to_ipv4()
            {
                return v4.to_string();
            }
            let masked = u128::from(v6) & (!0u128 << (128 - 56));
            format!("{}/56", Ipv6Addr::from(masked))
        }
        _ => address.to_owned(),
    }
}

/// How a request becomes a limiter key; `None` lets it through uncounted.
type KeyFn = dyn Fn(&Request) -> Option<String> + Send + Sync;

/// Middleware state: a limiter and how to get a request's key (`None` skips the limiter).
#[derive(Clone)]
pub struct Limited {
    limiter: Arc<Limiter>,
    key: Arc<KeyFn>,
}

impl Limited {
    pub fn new(limiter: Arc<Limiter>, key: impl Fn(&Request) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            limiter,
            key: Arc::new(key),
        }
    }
}

/// Applies a [`Limited`] to a route: `axum::middleware::from_fn_with_state(limited, enforce)`.
pub async fn enforce(State(limited): State<Limited>, request: Request, next: Next) -> Response {
    let key = (limited.key)(&request);
    around(&limited.limiter, key.as_deref(), next.run(request)).await
}

/// Counts one request for `key` and runs `handler` unless the key is over its limit; `None` runs it
/// uncounted. For a limit applied after something the handler does first (authentication).
pub async fn around(limiter: &Limiter, key: Option<&str>, handler: impl Future<Output = Response>) -> Response {
    let Some(key) = key else {
        return handler.await;
    };
    let decision = limiter.hit(key, Instant::now());
    let mut response = if decision.limited {
        let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(&limiter.reply)).into_response();
        if let Ok(value) = HeaderValue::from_str(&decision.reset_secs.to_string()) {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    } else {
        handler.await
    };
    for (name, value) in limiter.headers(key, &decision) {
        if let Ok(value) = HeaderValue::from_str(&value) {
            response.headers_mut().append(name, value);
        }
    }
    if !decision.limited && limiter.skip_successful && response.status().as_u16() < 400 {
        limiter.undo(key, &decision, Instant::now());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(limit: u64, window_secs: u64) -> Limiter {
        Limiter::new(limit, Duration::from_secs(window_secs), serde_json::json!({}))
    }

    #[test]
    fn names_match_express_rate_limit() {
        assert_eq!(limiter(60, 60).name(), "60-in-1min");
        assert_eq!(limiter(30, 600).name(), "30-in-10min");
        assert_eq!(limiter(5, 30).name(), "5-in-30sec");
        assert_eq!(limiter(5, 7200).name(), "5-in-2hrs");
        assert_eq!(limiter(5, 86400).name(), "5-in-1day");
    }

    #[test]
    fn each_key_has_its_own_window() {
        let limiter = limiter(2, 60);
        let start = Instant::now();
        assert!(!limiter.hit("a", start).limited);
        assert_eq!(limiter.hit("a", start + Duration::from_secs(10)).remaining, 0);
        let third = limiter.hit("a", start + Duration::from_secs(20));
        assert!(third.limited);
        assert_eq!(third.reset_secs, 40);
        assert!(
            !limiter.hit("b", start + Duration::from_secs(20)).limited,
            "another key"
        );
        assert!(
            !limiter.hit("a", start + Duration::from_secs(60)).limited,
            "a new window"
        );
    }

    #[test]
    fn undo_gives_the_hit_back_within_the_window() {
        let limiter = limiter(1, 60);
        let start = Instant::now();
        let first = limiter.hit("a", start);
        limiter.undo("a", &first, start);
        assert!(!limiter.hit("a", start).limited);
    }

    #[test]
    fn sweep_forgets_ended_windows() {
        let limiter = limiter(1, 60);
        let start = Instant::now();
        limiter.hit("a", start);
        limiter.sweep(start + Duration::from_secs(61));
        assert!(limiter.shards.iter().all(|s| s.lock().unwrap().is_empty()));
    }

    // Values from express-rate-limit 8.7 (getPartitionKey, ipKeyGenerator).
    #[test]
    fn keys_and_headers_match_express_rate_limit() {
        assert_eq!(partition_key("127.0.0.1"), "MTJjYTE3YjQ5YWYy");
        assert_eq!(ip_key("203.0.113.9"), "203.0.113.9");
        assert_eq!(ip_key("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(ip_key("2001:db8:abcd:12ff:1:2:3:4"), "2001:db8:abcd:1200::/56");
        assert_eq!(ip_key("unknown"), "unknown");
        let limiter = limiter(60, 60);
        let decision = limiter.hit("127.0.0.1", Instant::now());
        let [(_, rate), (_, policy)] = limiter.headers("127.0.0.1", &decision);
        assert_eq!(rate, "\"60-in-1min\"; r=59; t=60");
        assert_eq!(policy, "\"60-in-1min\"; q=60; w=60; pk=:MTJjYTE3YjQ5YWYy:");
    }
}
