//! The supervisor of one persistent hub, Ramsgate or the Training Grounds
//! (`src/controllers/hubSupervisor.ts`, 2026-10-10 1.14.7).
//!
//! - `ensure(reason)`: single-flight, concurrent callers share one start; never fails (a failed start
//!   gives `None` and arms the retry timer). Before the backoff is due it only re-arms the timer, so
//!   requests cannot skip the backoff.
//! - `on_exit(hub)`: acts only for the current hub. An exit after a stable run restarts at once and
//!   resets the failure count; an earlier exit counts as a failure (crash-loop backoff).
//! - `tick()`: the watchdog's safety net. A dead current hub is treated as an exit, a missing one is
//!   retried once its backoff is due.
//!
//! Backoff 5 s, 15 s, 30 s, 60 s, 120 s, then 300 s. The clock is monotonic: a wall-clock step back
//! must not push a down hub's next attempt into the future.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

pub const DEFAULT_BACKOFF_MS: [u64; 6] = [5_000, 15_000, 30_000, 60_000, 120_000, 300_000];
pub const DEFAULT_STABLE_UPTIME_MS: f64 = 120_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub trait Timer: Send {
    fn cancel(&self);
}

pub type StartFn<T> = Arc<dyn Fn(String) -> BoxFuture<'static, Result<Arc<T>, String>> + Send + Sync>;
pub type SetTimer = Arc<dyn Fn(Box<dyn FnOnce() + Send>, u64) -> Box<dyn Timer> + Send + Sync>;
pub type LogFn = Arc<dyn Fn(Level, &str, Option<&str>) + Send + Sync>;

pub struct Options<T> {
    pub label: String,
    /// Takes the `ensure` reason ("boot", "exit: ...", "retry", "watchdog", "request").
    pub start: StartFn<T>,
    pub is_alive: Arc<dyn Fn(&T) -> bool + Send + Sync>,
    pub now: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub set_timer: SetTimer,
    pub log: LogFn,
    pub backoff_ms: Vec<u64>,
    pub stable_uptime_ms: f64,
}

impl<T> Options<T> {
    /// The defaults: the monotonic clock, tokio timers, the standard backoff, no log.
    pub fn new(label: &str, start: StartFn<T>, is_alive: Arc<dyn Fn(&T) -> bool + Send + Sync>) -> Self {
        Self {
            label: label.to_owned(),
            start,
            is_alive,
            now: Arc::new(monotonic_ms),
            set_timer: Arc::new(tokio_timer),
            log: Arc::new(|_, _, _| {}),
            backoff_ms: DEFAULT_BACKOFF_MS.to_vec(),
            stable_uptime_ms: DEFAULT_STABLE_UPTIME_MS,
        }
    }
}

/// Milliseconds on a monotonic clock (`performance.now()`).
pub fn monotonic_ms() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

struct TokioTimer(tokio::task::AbortHandle);

impl Timer for TokioTimer {
    fn cancel(&self) {
        self.0.abort();
    }
}

fn tokio_timer(fn_: Box<dyn FnOnce() + Send>, ms: u64) -> Box<dyn Timer> {
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        fn_();
    });
    Box::new(TokioTimer(task.abort_handle()))
}

/// The on/off switches of the hubs (TRAINING_HUB, RAMSGATE_HUB): 0, false, no and off (any case,
/// trimmed) are off, anything else on; unset or blank takes the default.
pub fn switch_on(value: Option<&str>, default_on: bool) -> bool {
    match value.map(service_kit::js::trim) {
        None | Some("") => default_on,
        Some(v) => !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"),
    }
}

type Attempt<T> = Shared<BoxFuture<'static, Option<Arc<T>>>>;

struct State<T> {
    hub: Option<Arc<T>>,
    in_flight: Option<Attempt<T>>,
    failures: u32,
    next_attempt_at: f64,
    ready_since: f64,
    timer: Option<Box<dyn Timer>>,
}

struct Inner<T> {
    options: Options<T>,
    state: Mutex<State<T>>,
}

pub struct HubSupervisor<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for HubSupervisor<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

fn same<T>(a: &Arc<T>, b: &Arc<T>) -> bool {
    Arc::ptr_eq(a, b)
}

impl<T: Send + Sync + 'static> HubSupervisor<T> {
    pub fn new(mut options: Options<T>) -> Self {
        if options.backoff_ms.is_empty() {
            options.backoff_ms = DEFAULT_BACKOFF_MS.to_vec();
        }
        Self {
            inner: Arc::new(Inner {
                options,
                state: Mutex::new(State {
                    hub: None,
                    in_flight: None,
                    failures: 0,
                    next_attempt_at: 0.0,
                    ready_since: 0.0,
                    timer: None,
                }),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State<T>> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now(&self) -> f64 {
        (self.inner.options.now)()
    }

    fn log(&self, level: Level, message: &str, error: Option<&str>) {
        // Logging must never break supervision.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.inner.options.log)(level, message, error)
        }));
    }

    pub fn current(&self) -> Option<Arc<T>> {
        self.state().hub.clone()
    }

    pub fn pending(&self) -> bool {
        self.state().in_flight.is_some()
    }

    pub fn failure_count(&self) -> u32 {
        self.state().failures
    }

    pub fn next_attempt_due_at(&self) -> f64 {
        self.state().next_attempt_at
    }

    pub fn retry_armed(&self) -> bool {
        self.state().timer.is_some()
    }

    pub fn ensure(&self, reason: &str) -> BoxFuture<'static, Option<Arc<T>>> {
        let mut state = self.state();
        if let Some(hub) = state.hub.clone() {
            if (self.inner.options.is_alive)(&hub) {
                return futures_util::future::ready(Some(hub)).boxed();
            }
            self.mark_gone(&mut state, &hub, &format!("{reason}: process gone"));
        }
        if let Some(attempt) = &state.in_flight {
            return attempt.clone().boxed();
        }
        if self.now() < state.next_attempt_at {
            self.arm(&mut state);
            return futures_util::future::ready(None).boxed();
        }
        Self::cancel_timer(&mut state);
        let this = self.clone();
        let reason = reason.to_owned();
        // Spawned, so the start runs whether or not anyone awaits it; in flight before it can finish.
        let task = tokio::spawn(async move { this.run(reason).await });
        let attempt: Attempt<T> = async move { task.await.ok().flatten() }.boxed().shared();
        state.in_flight = Some(attempt.clone());
        attempt.boxed()
    }

    pub fn on_exit(&self, hub: &Arc<T>, reason: &str) {
        let mut state = self.state();
        // A late exit of a failed start or an older run.
        if !self.mark_gone(&mut state, hub, reason) {
            return;
        }
        drop(state);
        drop(self.ensure(&format!("exit: {reason}")));
    }

    /// An on-demand hub stopped on purpose (idle, Director patch 0033): forgotten without a restart,
    /// a failure or a backoff, so the next `ensure` starts it at once. False when `hub` is not current.
    pub fn release(&self, hub: &Arc<T>, reason: &str) -> bool {
        let mut state = self.state();
        if !state.hub.as_ref().is_some_and(|h| same(h, hub)) {
            return false;
        }
        state.hub = None;
        state.failures = 0;
        state.next_attempt_at = 0.0;
        Self::cancel_timer(&mut state);
        drop(state);
        self.log(
            Level::Info,
            &format!(
                "[HubSupervisor] {} released ({reason}) - started again on the next request",
                self.inner.options.label
            ),
            None,
        );
        true
    }

    pub fn tick(&self) {
        let mut state = self.state();
        if let Some(hub) = state.hub.clone() {
            if !(self.inner.options.is_alive)(&hub) {
                drop(state);
                self.on_exit(&hub, "watchdog: process gone");
                return;
            }
            let up = self.now() - state.ready_since;
            if state.failures > 0 && up >= self.inner.options.stable_uptime_ms {
                state.failures = 0;
                drop(state);
                self.log(
                    Level::Info,
                    &format!(
                        "[HubSupervisor] {} stable for {}s - failure count reset",
                        self.inner.options.label,
                        (up / 1000.0).round()
                    ),
                    None,
                );
            }
            return;
        }
        if state.in_flight.is_some() {
            return;
        }
        drop(state);
        drop(self.ensure("watchdog"));
    }

    pub fn dispose(&self) {
        Self::cancel_timer(&mut self.state());
    }

    async fn run(&self, reason: String) -> Option<Arc<T>> {
        let label = &self.inner.options.label;
        let attempt = self.state().failures + 1;
        self.log(
            Level::Info,
            &format!("[HubSupervisor] {label} starting ({reason}, attempt {attempt})"),
            None,
        );
        let hub = match (self.inner.options.start)(reason.clone()).await {
            Ok(hub) => hub,
            Err(error) => return self.fail("start failed", Some(&error)),
        };
        // An exit between ready and here was ignored by on_exit (the hub was not current yet).
        if !(self.inner.options.is_alive)(&hub) {
            return self.fail("exited right after reporting ready", None);
        }
        let mut state = self.state();
        state.in_flight = None;
        state.hub = Some(hub.clone());
        state.ready_since = self.now();
        state.next_attempt_at = 0.0;
        let failures = state.failures;
        drop(state);
        self.log(
            Level::Info,
            &format!("[HubSupervisor] {label} up ({reason}; failures so far {failures})"),
            None,
        );
        Some(hub)
    }

    fn fail(&self, what: &str, error: Option<&str>) -> Option<Arc<T>> {
        let mut state = self.state();
        state.failures += 1;
        let delay = self.delay(state.failures);
        state.next_attempt_at = self.now() + delay as f64;
        // Cleared before arm(), which refuses while a start is in flight.
        state.in_flight = None;
        let failures = state.failures;
        self.arm(&mut state);
        drop(state);
        self.log(
            Level::Error,
            &format!(
                "[HubSupervisor] {} {what} (failure {failures}); retrying in {}s",
                self.inner.options.label,
                delay as f64 / 1000.0
            ),
            error,
        );
        None
    }

    /// Clears the current hub and updates the backoff. False when `hub` is not current.
    fn mark_gone(&self, state: &mut State<T>, hub: &Arc<T>, reason: &str) -> bool {
        if !state.hub.as_ref().is_some_and(|h| same(h, hub)) {
            return false;
        }
        state.hub = None;
        // That start's outcome decides the backoff.
        if state.in_flight.is_some() {
            return true;
        }
        let label = &self.inner.options.label;
        let up = self.now() - state.ready_since;
        let seconds = (up / 1000.0).round();
        if up >= self.inner.options.stable_uptime_ms {
            state.failures = 0;
            state.next_attempt_at = 0.0;
            self.log(
                Level::Warn,
                &format!("[HubSupervisor] {label} exited after {seconds}s up ({reason}) - restarting now"),
                None,
            );
        } else {
            state.failures += 1;
            let delay = self.delay(state.failures);
            state.next_attempt_at = self.now() + delay as f64;
            self.log(
                Level::Warn,
                &format!(
                    "[HubSupervisor] {label} exited {seconds}s after ready ({reason}; failure {}) - restart in {}s",
                    state.failures,
                    delay as f64 / 1000.0
                ),
                None,
            );
        }
        true
    }

    fn delay(&self, failures: u32) -> u64 {
        let backoff = &self.inner.options.backoff_ms;
        let index = (failures.max(1) as usize - 1).min(backoff.len() - 1);
        backoff[index]
    }

    fn arm(&self, state: &mut State<T>) {
        if state.timer.is_some() || state.in_flight.is_some() || state.hub.is_some() {
            return;
        }
        let ms = (state.next_attempt_at - self.now()).max(0.0) as u64;
        let this = self.clone();
        state.timer = Some((self.inner.options.set_timer)(
            Box::new(move || {
                this.state().timer = None;
                drop(this.ensure("retry"));
            }),
            ms,
        ));
    }

    fn cancel_timer(state: &mut State<T>) {
        if let Some(timer) = state.timer.take() {
            timer.cancel();
        }
    }
}
