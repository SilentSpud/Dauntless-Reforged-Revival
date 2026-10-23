//! Brings the firewall to the desired address set (`src/allowlist/sync.ts`), never more often than
//! once per `min_interval`: the first change after a quiet period applies at once, and changes that
//! arrive while a run is in progress or too soon after one are folded into a single later run. A
//! failed run is retried with backoff (the interval doubling, at most a minute).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

pub type Apply = Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;
pub type OnResult = Arc<dyn Fn(&ApplyResult) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    pub ok: bool,
    pub ips: Vec<String>,
    /// When the run started (milliseconds since the epoch)
    pub at: i64,
    pub ms: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct State {
    desired: Vec<String>,
    /// `None`: not known (at start, and after a failed run), so the next run always applies
    applied: Option<Vec<String>>,
    last_start: Option<Instant>,
    failures: u32,
    stopped: bool,
    running: bool,
    runs: u64,
    last: Option<ApplyResult>,
}

struct Inner {
    apply: Apply,
    on_result: OnResult,
    min_interval: Duration,
    state: Mutex<State>,
    /// Wakes the scheduler: a new request, or the end of a run
    wake: Notify,
    /// Held while the firewall is being changed, so runs never overlap
    run_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct FirewallSync {
    inner: Arc<Inner>,
}

impl FirewallSync {
    /// Starts the scheduler on the current runtime.
    pub fn new(apply: Apply, min_interval: Duration, on_result: OnResult) -> Self {
        let inner = Arc::new(Inner {
            apply,
            on_result,
            min_interval,
            state: Mutex::default(),
            wake: Notify::new(),
            run_lock: tokio::sync::Mutex::new(()),
        });
        tokio::spawn(schedule(inner.clone()));
        Self { inner }
    }

    pub fn request(&self, ips: Vec<String>) {
        self.inner.lock().desired = ips;
        self.inner.wake.notify_one();
    }

    pub fn pending(&self) -> bool {
        let state = self.inner.lock();
        state.running || state.applied.as_ref() != Some(&state.desired)
    }

    pub fn runs(&self) -> u64 {
        self.inner.lock().runs
    }

    pub fn last_result(&self) -> Option<ApplyResult> {
        self.inner.lock().last.clone()
    }

    /// Resolves once the firewall matches the desired set (or the sync was stopped).
    pub async fn settled(&self, timeout: Duration) {
        let until = Instant::now() + timeout;
        while self.pending() && !self.inner.lock().stopped && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Shutdown: waits for a run in progress, then applies `ips` once more right away (ignoring the
    /// interval) and stops scheduling.
    pub async fn finish(&self, ips: Vec<String>) {
        self.inner.lock().stopped = true;
        self.inner.wake.notify_one();
        let _running = self.inner.run_lock.lock().await;
        let same = {
            let mut state = self.inner.lock();
            state.desired = ips;
            state.applied.as_ref() == Some(&state.desired)
        };
        if !same {
            run(&self.inner).await;
        }
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One run: applies what is desired now and records the result.
async fn run(inner: &Inner) {
    let snapshot = {
        let mut state = inner.lock();
        if state.applied.as_ref() == Some(&state.desired) {
            return;
        }
        state.last_start = Some(Instant::now());
        state.runs += 1;
        state.running = true;
        state.desired.clone()
    };
    let at = crate::now_ms();
    let started = Instant::now();
    let outcome = (inner.apply)(snapshot.clone()).await;
    let result = ApplyResult {
        ok: outcome.is_ok(),
        ips: snapshot.clone(),
        at,
        ms: started.elapsed().as_millis() as u64,
        error: outcome.err(),
    };
    {
        let mut state = inner.lock();
        if result.ok {
            state.applied = Some(snapshot);
            state.failures = 0;
        } else {
            state.applied = None;
            state.failures += 1;
        }
        state.running = false;
        state.last = Some(result.clone());
    }
    (inner.on_result)(&result);
}

async fn schedule(inner: Arc<Inner>) {
    loop {
        // Wait until there is something to do
        loop {
            let notified = inner.wake.notified();
            {
                let state = inner.lock();
                if state.stopped {
                    return;
                }
                if state.applied.as_ref() != Some(&state.desired) {
                    break;
                }
            }
            notified.await;
        }
        let delay = {
            let state = inner.lock();
            let backoff = if state.failures == 0 {
                Duration::ZERO
            } else {
                (inner.min_interval * 2u32.saturating_pow(state.failures - 1)).min(Duration::from_secs(60))
            };
            let spacing = state.last_start.map_or(Duration::ZERO, |last| {
                (last + inner.min_interval).saturating_duration_since(Instant::now())
            });
            backoff.max(spacing)
        };
        tokio::time::sleep(delay).await;
        let _running = inner.run_lock.lock().await;
        if inner.lock().stopped {
            return;
        }
        run(&inner).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    type Runs = Arc<Mutex<Vec<(Instant, Vec<String>)>>>;

    fn recorder(runs: Runs, delay: Duration) -> Apply {
        Arc::new(move |ips| {
            let runs = runs.clone();
            Box::pin(async move {
                runs.lock().unwrap().push((Instant::now(), ips));
                tokio::time::sleep(delay).await;
                Ok(())
            })
        })
    }

    fn ignore() -> OnResult {
        Arc::new(|_| {})
    }

    fn ips(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    async fn wait_for(what: &str, condition: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn applies_at_once_then_folds_quick_changes_never_closer_than_the_interval() {
        let runs: Runs = Arc::default();
        let sync = FirewallSync::new(
            recorder(runs.clone(), Duration::from_millis(50)),
            Duration::from_millis(300),
            ignore(),
        );
        sync.request(ips(&["203.0.113.1"]));
        wait_for("the first run", || runs.lock().unwrap().len() == 1).await;
        sync.request(ips(&["203.0.113.1", "203.0.113.2"]));
        sync.request(ips(&["203.0.113.1", "203.0.113.2", "203.0.113.3"]));
        sync.settled(Duration::from_secs(3)).await;
        {
            let runs = runs.lock().unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[1].1, ips(&["203.0.113.1", "203.0.113.2", "203.0.113.3"]));
            assert!(runs[1].0 - runs[0].0 >= Duration::from_millis(290));
        }
        // Asking for what is already applied does nothing
        sync.request(ips(&["203.0.113.1", "203.0.113.2", "203.0.113.3"]));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(runs.lock().unwrap().len(), 2);
        // A change well after the interval applies at once again
        let asked = Instant::now();
        sync.request(Vec::new());
        wait_for("the third run", || runs.lock().unwrap().len() == 3).await;
        let runs = runs.lock().unwrap();
        assert!(runs[2].0 - asked < Duration::from_millis(100));
        assert!(runs[2].1.is_empty());
    }

    #[tokio::test]
    async fn retries_a_failed_run_and_reports_each_result() {
        let calls = Arc::new(AtomicU32::new(0));
        let results: Arc<Mutex<Vec<bool>>> = Arc::default();
        let apply: Apply = {
            let calls = calls.clone();
            Arc::new(move |_| {
                let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                Box::pin(async move { if first { Err("access denied".to_owned()) } else { Ok(()) } })
            })
        };
        let on_result: OnResult = {
            let results = results.clone();
            Arc::new(move |result| results.lock().unwrap().push(result.ok))
        };
        let sync = FirewallSync::new(apply, Duration::from_millis(100), on_result);
        sync.request(ips(&["203.0.113.9"]));
        wait_for("the retry", || results.lock().unwrap().len() == 2).await;
        assert_eq!(*results.lock().unwrap(), [false, true]);
        assert!(sync.last_result().unwrap().ok);
    }

    #[tokio::test]
    async fn finish_waits_for_a_run_in_progress_then_applies_at_once() {
        let runs: Runs = Arc::default();
        let sync = FirewallSync::new(
            recorder(runs.clone(), Duration::from_millis(150)),
            Duration::from_secs(10),
            ignore(),
        );
        sync.request(ips(&["203.0.113.1"]));
        tokio::time::sleep(Duration::from_millis(20)).await;
        sync.finish(Vec::new()).await;
        let seen: Vec<Vec<String>> = runs.lock().unwrap().iter().map(|(_, ips)| ips.clone()).collect();
        assert_eq!(seen, [ips(&["203.0.113.1"]), Vec::new()]);
        // Stopped: later requests are ignored
        sync.request(ips(&["203.0.113.5"]));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runs.lock().unwrap().len(), 2);
    }
}
