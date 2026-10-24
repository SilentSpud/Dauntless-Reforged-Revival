#![allow(clippy::unwrap_used, clippy::expect_used)] // tests

//! The Node Director's unit tests, ported: `portPool.test.ts`, `memoryAdmission.test.ts`,
//! `hubOccupancy.test.ts` and `hubSupervisor.test.ts` (the hunt resolver's are in `hunt.rs`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use director::hub::{self, HubSupervisor, Options, Timer};
use director::memory::{self, MemoryAdmission};
use director::occupancy::{IdleStopPolicy, RuntimeLogOccupancy};
use director::port_pool::{Lease, PortPool};
use futures_util::FutureExt;
use service_kit::env::Env;

// ---- portPool.test.ts

fn pool(ports: &[u16]) -> PortPool {
    let mut pool = PortPool::new();
    for port in ports {
        pool.add(*port);
    }
    pool
}

#[test]
fn a_late_release_from_an_old_owner_cannot_free_the_new_owners_port() {
    let mut pool = pool(&[8780]);
    assert_eq!(pool.take("A"), Some(8780));
    assert!(pool.release(8780, "A"));
    assert_eq!(pool.take("B"), Some(8780));
    assert!(!pool.release(8780, "A"));
    assert_eq!(pool.take("C"), None);
    assert_eq!(pool.owner_of(8780), Some("B"));
    assert!(pool.release(8780, "B"));
}

#[test]
fn release_is_idempotent() {
    let mut pool = pool(&[8780, 8781, 8782]);
    let port = pool.take("A").unwrap();
    assert!(pool.release(port, "A"));
    assert!(!pool.release(port, "A"));
    assert_eq!(pool.free_count(), 3);
    let taken = [pool.take("x"), pool.take("y"), pool.take("z")];
    assert_eq!(taken.iter().collect::<std::collections::HashSet<_>>().len(), 3);
    assert_eq!(pool.take("w"), None);
}

#[test]
fn add_ignores_a_port_that_is_already_free_or_held() {
    let mut pool = pool(&[8780, 8781]);
    pool.add(8780);
    assert_eq!(pool.free_count(), 2);
    let port = pool.take("A").unwrap();
    pool.add(port);
    assert_eq!((pool.free_count(), pool.held_count()), (1, 1));
}

#[test]
fn releasing_a_port_never_taken_changes_nothing() {
    let mut pool = pool(&[8780]);
    assert!(!pool.release(8780, "A"));
    assert!(!pool.release(9999, "A"));
    assert_eq!((pool.free_count(), pool.held_count()), (1, 0));
}

#[test]
fn fifo_a_just_released_port_is_reused_last() {
    let mut pool = pool(&[8780, 8781, 8782]);
    let first = pool.take("A").unwrap();
    pool.release(first, "A");
    assert_ne!(pool.take("B"), Some(first));
}

#[test]
fn readiness_timeout_race_no_port_is_held_by_two_owners() {
    let mut pool = pool(&[8780, 8781, 8782]);
    let a = pool.take("A").unwrap();
    let b = pool.take("B").unwrap();
    assert_ne!(a, b);
    assert!(pool.release(a, "A"));
    let c = pool.take("C").unwrap();
    let d = pool.take("D").unwrap();
    assert_eq!([b, c, d].iter().collect::<std::collections::HashSet<_>>().len(), 3);
    assert!([a, b, c, d].iter().all(|p| pool.owner_of(*p) != Some("A")));
    assert_eq!(pool.held_count() + pool.free_count(), 3);
}

#[test]
fn a_lease_is_reclaimed_when_its_exit_is_lost_and_its_pid_is_gone() {
    let mut pool = pool(&[8780, 8781]);
    let port = pool.take("A").unwrap();
    assert!(pool.attach_pid(port, "A", Some(4242)));
    assert!(!pool.attach_pid(port, "B", Some(1)), "only the owner can attach a pid");
    assert_eq!(pool.pid_of(port), Some(4242));
    // Still being stopped: unlisted owner, live pid -> stays held.
    assert_eq!(pool.reclaim(|_| false, |_| true), vec![]);
    // A listed owner is never reclaimed here, even with a dead pid.
    assert_eq!(pool.reclaim(|o| o == "A", |_| false), vec![]);
    let lease = Lease {
        port,
        owner: "A".into(),
        pid: Some(4242),
    };
    assert_eq!(pool.reclaim(|_| false, |_| false), vec![lease]);
    assert_eq!(pool.owner_of(port), None);
    assert_eq!(pool.free_count(), 2);
    let (reused, again) = (pool.take("B").unwrap(), pool.take("C").unwrap());
    assert!(reused == port || again == port);
    assert!(!pool.release(port, "A"), "a late exit after the reclaim is refused");
    assert_eq!(pool.held_count(), 2);
}

#[test]
fn a_lease_without_a_pid_is_reclaimable_once_unlisted() {
    let mut pool = pool(&[8780]);
    let port = pool.take("A").unwrap();
    let lease = Lease {
        port,
        owner: "A".into(),
        pid: None,
    };
    assert_eq!(pool.reclaim(|_| false, |_| true), vec![lease]);
    assert_eq!(pool.free_count(), 1);
}

// ---- memoryAdmission.test.ts

const MB: u64 = 1024 * 1024;

fn guard(available_mb: Option<u64>, clock: Arc<Mutex<i64>>) -> MemoryAdmission {
    MemoryAdmission::with(
        memory::Options {
            enabled: true,
            min_free_mb: 1024,
            hunt_mb: 1536,
            hold_ms: 120_000,
        },
        Box::new(move || available_mb.map(|mb| mb * MB)),
        Box::new(move || *clock.lock().unwrap()),
    )
}

#[test]
fn a_hunt_starts_only_when_one_more_fits_and_the_floor_stays_free() {
    assert!(guard(Some(2560), Arc::default()).reserve().is_ok());
    assert!(guard(Some(2559), Arc::default()).reserve().is_err());
}

#[test]
fn reservations_add_up_lapse_and_can_be_given_back() {
    let clock: Arc<Mutex<i64>> = Arc::default();
    let g = guard(Some(5000), clock.clone());
    g.reserve().unwrap();
    let second = g.reserve().unwrap();
    assert_eq!(g.status().reserved_mb, 3072);
    let refused = g.reserve().unwrap_err();
    assert_eq!(
        refused.to_string(),
        "Not enough free memory for another hunt server (5000 MB available, 3072 MB reserved for hunts still starting, 2560 MB needed)"
    );
    g.give_back(second);
    assert!(g.reserve().is_ok());
    *clock.lock().unwrap() = 120_000;
    assert_eq!(g.status().reserved_mb, 0);
}

#[test]
fn off_or_unreadable_memory_never_blocks_a_hunt() {
    let off = MemoryAdmission::with(
        memory::Options {
            enabled: false,
            min_free_mb: 1024,
            hunt_mb: 1536,
            hold_ms: 1,
        },
        Box::new(|| Some(0)),
        Box::new(|| 0),
    );
    assert!(off.reserve().is_ok());
    assert!(guard(None, Arc::default()).reserve().is_ok());
}

#[test]
fn settings_come_from_the_environment_and_a_bad_value_keeps_its_default() {
    let (off, _) = memory::options_from_env(&Env::from_pairs([]));
    assert!(!off.enabled);
    let (on, problems) = memory::options_from_env(&Env::from_pairs([
        ("GAMESERVER_MEMORY_GUARD", "1"),
        ("GAMESERVER_MIN_FREE_MB", "512"),
        ("GAMESERVER_HUNT_MEMORY_MB", "2048"),
    ]));
    assert!(on.enabled && problems.is_empty());
    assert_eq!(on.min_free_mb + on.hunt_mb, 2560);
    let (bad, problems) = memory::options_from_env(&Env::from_pairs([
        ("GAMESERVER_MEMORY_GUARD", "1"),
        ("GAMESERVER_MIN_FREE_MB", "lots"),
    ]));
    assert_eq!(
        problems,
        ["GAMESERVER_MIN_FREE_MB='lots' is not a whole number from 0 to 1048576; using 1024"]
    );
    assert_eq!(bad.min_free_mb + bad.hunt_mb, 1024 + 1536);
}

#[test]
fn reads_linux_memavailable_and_falls_back_to_the_free_memory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("meminfo");
    std::fs::write(
        &file,
        "MemTotal:       16384000 kB\nMemFree:          512000 kB\nMemAvailable:    8192000 kB\n",
    )
    .unwrap();
    assert_eq!(memory::read_available_bytes(&file, true), Some(8_192_000 * 1024));
    let fallback = memory::read_available_bytes(&dir.path().join("missing"), true);
    assert!(fallback.is_none_or(|b| b > 0));
}

// ---- hubOccupancy.test.ts

fn edge(before: i64, after: i64, pid: u32) -> String {
    format!("[12:00:00.000] pid={pid} [NetConnEdge] tick=42 connCount {before} -> {after} connMax=16 NetDriver=0x1\n")
}

fn append(file: &std::path::Path, text: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn follows_the_connection_edges_from_where_the_log_ended_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mysticparadox_dll_port8776.log");
    std::fs::write(&file, edge(-2, 0, 100) + &edge(0, 3, 100)).unwrap();
    let mut occupancy = RuntimeLogOccupancy::new(file.clone());
    occupancy.begin();
    assert_eq!(occupancy.poll(), None, "nothing from this process yet");
    append(
        &file,
        &("[12:00:00.000] pid=292 [FlagCache] flag-file cache ON\n".to_owned() + &edge(-2, 0, 292)),
    );
    assert_eq!(occupancy.poll(), Some(0));
    append(&file, &(edge(0, 1, 292) + &edge(1, 2, 292)));
    assert_eq!(occupancy.poll(), Some(2));
    append(
        &file,
        &(edge(2, 1, 292) + "[12:00:01.000] pid=292 [NetConnEdge] tick=43 connC"),
    );
    assert_eq!(occupancy.poll(), Some(1));
    append(&file, "ount 1 -> 0 connMax=16 NetDriver=0x1\n");
    assert_eq!(occupancy.poll(), Some(0), "the split line is read whole");
    assert_eq!(occupancy.poll(), Some(0), "no new bytes: the last count stays");
}

#[test]
fn a_missing_log_is_unknown_and_a_truncated_one_is_read_from_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mysticparadox_dll_port8776.log");
    let mut occupancy = RuntimeLogOccupancy::new(file.clone());
    occupancy.begin();
    assert_eq!(occupancy.poll(), None);
    std::fs::write(&file, "x".repeat(200) + "\n" + &edge(0, 2, 1)).unwrap();
    assert_eq!(occupancy.poll(), Some(2));
    std::fs::write(&file, edge(-2, 0, 1)).unwrap();
    assert_eq!(occupancy.poll(), Some(0));
}

#[test]
fn stops_only_after_the_idle_time_from_the_last_of_empty_ready_and_request() {
    let minute = 60_000;
    let mut policy = IdleStopPolicy::new(5 * minute);
    assert!(!policy.should_stop(0, Some(0), 0, 0), "just went empty");
    assert!(!policy.should_stop(4 * minute, Some(0), 0, 0));
    assert!(policy.should_stop(5 * minute, Some(0), 0, 0));
    policy.reset();
    assert!(!policy.should_stop(10 * minute, Some(0), 0, 0));
    assert!(!policy.should_stop(16 * minute, Some(0), 0, 12 * minute));
    assert!(policy.should_stop(17 * minute, Some(0), 0, 12 * minute));
    policy.reset();
    assert!(!policy.should_stop(20 * minute, Some(1), 0, 0), "a player is in");
    assert!(!policy.should_stop(30 * minute, None, 0, 0), "unknown never stops");
    assert!(
        !policy.should_stop(31 * minute, Some(0), 0, 0),
        "empty again: the clock starts now"
    );
    assert!(policy.should_stop(36 * minute, Some(0), 0, 0));
    assert!(!IdleStopPolicy::new(0).should_stop(1_000_000_000, Some(0), 0, 0));
}

// ---- hubSupervisor.test.ts

#[derive(Debug)]
struct FakeHub {
    id: usize,
    alive: AtomicBool,
}

fn fake(id: usize, alive: bool) -> Arc<FakeHub> {
    Arc::new(FakeHub {
        id,
        alive: AtomicBool::new(alive),
    })
}

enum Outcome {
    Ok,
    Fail,
    Hub(Arc<FakeHub>),
    Later(tokio::sync::oneshot::Receiver<Result<Arc<FakeHub>, String>>),
}

struct FakeTimer {
    fire: Option<Box<dyn FnOnce() + Send>>,
    ms: u64,
    cancelled: Arc<AtomicBool>,
}

struct Cancel(Arc<AtomicBool>);

impl Timer for Cancel {
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Harness {
    supervisor: HubSupervisor<FakeHub>,
    timers: Arc<Mutex<Vec<FakeTimer>>>,
    clock: Arc<Mutex<f64>>,
    starts: Arc<AtomicUsize>,
}

impl Harness {
    fn new(outcomes: Vec<Outcome>) -> Self {
        Self::with(outcomes, |_| {})
    }

    fn with(outcomes: Vec<Outcome>, tweak: impl FnOnce(&mut Options<FakeHub>)) -> Self {
        let outcomes = Arc::new(Mutex::new(VecDeque::from(outcomes)));
        let starts = Arc::new(AtomicUsize::new(0));
        let next_id = Arc::new(AtomicUsize::new(1));
        let clock = Arc::new(Mutex::new(1_000_000.0));
        let timers: Arc<Mutex<Vec<FakeTimer>>> = Arc::default();
        let counted = starts.clone();
        let start: hub::StartFn<FakeHub> = Arc::new(move |_reason| {
            counted.fetch_add(1, Ordering::SeqCst);
            let outcome = outcomes.lock().unwrap().pop_front().unwrap_or(Outcome::Ok);
            let next_id = next_id.clone();
            async move {
                match outcome {
                    Outcome::Ok => Ok(fake(next_id.fetch_add(1, Ordering::SeqCst), true)),
                    Outcome::Fail => Err("did not report ready".to_owned()),
                    Outcome::Hub(hub) => Ok(hub),
                    Outcome::Later(rx) => rx.await.unwrap_or_else(|_| Err("dropped".into())),
                }
            }
            .boxed()
        });
        let mut options = Options::new(
            "test_hub",
            start,
            Arc::new(|h: &FakeHub| h.alive.load(Ordering::SeqCst)),
        );
        let now = clock.clone();
        options.now = Arc::new(move || *now.lock().unwrap());
        let list = timers.clone();
        options.set_timer = Arc::new(move |fire, ms| {
            let cancelled = Arc::new(AtomicBool::new(false));
            list.lock().unwrap().push(FakeTimer {
                fire: Some(fire),
                ms,
                cancelled: cancelled.clone(),
            });
            Box::new(Cancel(cancelled))
        });
        tweak(&mut options);
        Self {
            supervisor: HubSupervisor::new(options),
            timers,
            clock,
            starts,
        }
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn advance(&self, ms: f64) {
        *self.clock.lock().unwrap() += ms;
    }

    fn live(&self) -> Vec<u64> {
        self.timers
            .lock()
            .unwrap()
            .iter()
            .filter(|t| !t.cancelled.load(Ordering::SeqCst))
            .map(|t| t.ms)
            .collect()
    }

    /// Fires the newest live timer after moving the clock to its due time.
    async fn fire(&self) {
        let fire = {
            let mut timers = self.timers.lock().unwrap();
            let timer = timers
                .iter_mut()
                .rev()
                .find(|t| !t.cancelled.load(Ordering::SeqCst))
                .expect("an armed retry timer");
            timer.cancelled.store(true, Ordering::SeqCst);
            *self.clock.lock().unwrap() += timer.ms as f64;
            timer.fire.take().unwrap()
        };
        fire();
        flush().await;
    }
}

async fn flush() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_failed_start_arms_the_retry_and_the_retry_starts_the_hub() {
    let h = Harness::new(vec![Outcome::Fail, Outcome::Ok]);
    assert!(h.supervisor.ensure("boot").await.is_none());
    assert_eq!(h.starts(), 1);
    assert!(!h.supervisor.pending());
    assert_eq!(h.live(), [5_000], "the first design armed no timer here");
    assert!(h.supervisor.retry_armed());
    h.fire().await;
    assert_eq!(h.starts(), 2);
    assert!(h.supervisor.current().is_some_and(|c| c.alive.load(Ordering::SeqCst)));
    assert!(h.live().is_empty());
}

#[tokio::test]
async fn failed_starts_back_off_and_stay_capped_at_300_s() {
    let h = Harness::new((0..8).map(|_| Outcome::Fail).collect());
    h.supervisor.ensure("boot").await;
    let mut delays = vec![h.live()[0]];
    for _ in 0..7 {
        h.fire().await;
        delays.push(h.live()[0]);
    }
    assert_eq!(
        delays,
        [5_000, 15_000, 30_000, 60_000, 120_000, 300_000, 300_000, 300_000]
    );
    assert_eq!(h.starts(), 8);
}

#[tokio::test]
async fn a_failed_restart_after_a_crash_is_retried_by_its_timer() {
    let h = Harness::new(vec![Outcome::Ok, Outcome::Fail, Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(200_000.0);
    hub.alive.store(false, Ordering::SeqCst);
    h.supervisor.on_exit(&hub, "exit code=3");
    flush().await;
    assert_eq!(h.starts(), 2, "a stable exit restarts at once");
    assert!(h.supervisor.current().is_none());
    assert_eq!(h.live().len(), 1);
    h.fire().await;
    assert_eq!(h.starts(), 3);
    assert!(h.supervisor.current().is_some());
}

#[tokio::test]
async fn concurrent_ensure_callers_share_one_start() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let h = Harness::new(vec![Outcome::Later(rx)]);
    let a = h.supervisor.ensure("boot");
    let b = h.supervisor.ensure("request");
    let c = h.supervisor.ensure("watchdog");
    h.supervisor.tick();
    flush().await;
    assert_eq!(h.starts(), 1);
    assert!(h.supervisor.pending());
    let hub = fake(42, true);
    tx.send(Ok(hub.clone())).unwrap();
    for got in [a.await, b.await, c.await] {
        assert!(Arc::ptr_eq(&got.unwrap(), &hub));
    }
    assert_eq!(h.starts(), 1);
    assert!(!h.supervisor.pending());
}

#[tokio::test]
async fn an_exit_after_a_stable_run_restarts_at_once_without_a_timer() {
    let h = Harness::new(vec![Outcome::Ok, Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(120_000.0);
    hub.alive.store(false, Ordering::SeqCst);
    h.supervisor.on_exit(&hub, "exit code=0");
    flush().await;
    assert_eq!(h.starts(), 2);
    assert!(h.timers.lock().unwrap().is_empty());
    assert!(!h.supervisor.current().is_some_and(|c| Arc::ptr_eq(&c, &hub)));
    assert_eq!(h.supervisor.failure_count(), 0);
}

#[tokio::test]
async fn a_crash_loop_backs_off_and_requests_cannot_skip_it() {
    let h = Harness::new(vec![Outcome::Ok, Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(10_000.0);
    hub.alive.store(false, Ordering::SeqCst);
    h.supervisor.on_exit(&hub, "exit code=3");
    flush().await;
    assert_eq!(h.starts(), 1, "no immediate start after an early exit");
    assert_eq!(h.live(), [5_000]);
    h.advance(1_000.0);
    assert!(h.supervisor.ensure("request").await.is_none());
    assert_eq!(h.starts(), 1);
    assert_eq!(h.live().len(), 1, "the request reuses the armed timer");
    h.fire().await;
    assert_eq!(h.starts(), 2);
    assert!(h.supervisor.current().is_some());
}

#[tokio::test]
async fn an_exit_of_a_hub_that_is_not_current_is_ignored() {
    let h = Harness::new(vec![Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.supervisor
        .on_exit(&fake(999, false), "late exit of a killed failed start");
    flush().await;
    assert_eq!(h.starts(), 1);
    assert!(Arc::ptr_eq(&h.supervisor.current().unwrap(), &hub));
    assert!(h.timers.lock().unwrap().is_empty());
}

#[tokio::test]
async fn watchdog_and_exit_event_in_one_pass_restart_once() {
    let h = Harness::new(vec![Outcome::Ok, Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(300_000.0);
    hub.alive.store(false, Ordering::SeqCst);
    h.supervisor.tick();
    h.supervisor.on_exit(&hub, "exit code=3");
    flush().await;
    assert_eq!(h.starts(), 2);
    assert!(!h.supervisor.current().is_some_and(|c| Arc::ptr_eq(&c, &hub)));
}

#[tokio::test]
async fn tick_retries_a_missing_hub_once_due_and_never_doubles_a_start() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let h = Harness::new(vec![Outcome::Fail, Outcome::Later(rx)]);
    h.supervisor.ensure("boot").await;
    h.supervisor.tick();
    flush().await;
    assert_eq!(h.starts(), 1, "backoff not due yet");
    h.advance(5_000.0);
    h.supervisor.tick();
    flush().await;
    assert_eq!(h.starts(), 2);
    h.supervisor.tick();
    flush().await;
    assert_eq!(h.starts(), 2, "a start is in flight");
    tx.send(Ok(fake(7, true))).unwrap();
    flush().await;
    assert_eq!(h.supervisor.current().map(|c| c.id), Some(7));
}

#[tokio::test]
async fn an_exit_reported_while_a_start_is_in_flight_adds_no_failure() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let h = Harness::new(vec![Outcome::Ok, Outcome::Later(rx)]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(300_000.0);
    hub.alive.store(false, Ordering::SeqCst);
    drop(h.supervisor.ensure("request"));
    flush().await;
    assert_eq!(h.starts(), 2);
    h.supervisor.on_exit(&hub, "late exit event");
    assert_eq!(h.supervisor.failure_count(), 0);
    tx.send(Ok(fake(8, true))).unwrap();
    flush().await;
    assert_eq!(h.supervisor.current().map(|c| c.id), Some(8));
    assert_eq!(h.supervisor.failure_count(), 0);
}

#[tokio::test]
async fn a_hub_dead_when_its_start_resolves_is_a_failed_start() {
    let h = Harness::new(vec![Outcome::Hub(fake(5, false)), Outcome::Ok]);
    assert!(h.supervisor.ensure("boot").await.is_none());
    assert!(h.supervisor.current().is_none());
    assert_eq!(h.live().len(), 1);
    h.fire().await;
    assert!(h.supervisor.current().is_some_and(|c| c.alive.load(Ordering::SeqCst)));
}

#[tokio::test]
async fn two_supervisors_sharing_a_gate_never_start_at_once() {
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let order: Arc<Mutex<Vec<String>>> = Arc::default();
    let make = |label: &'static str, rx: tokio::sync::oneshot::Receiver<Result<Arc<FakeHub>, String>>| {
        let rx = Arc::new(Mutex::new(Some(rx)));
        let (gate, order) = (gate.clone(), order.clone());
        let start: hub::StartFn<FakeHub> = Arc::new(move |_| {
            let (gate, order, rx) = (gate.clone(), order.clone(), rx.lock().unwrap().take());
            async move {
                let _held = gate.lock().await;
                order.lock().unwrap().push(format!("{label}:begin"));
                let result = match rx {
                    Some(rx) => rx.await.unwrap_or_else(|_| Err("dropped".into())),
                    None => Err("no outcome".into()),
                };
                order.lock().unwrap().push(format!("{label}:end"));
                result
            }
            .boxed()
        });
        let mut options = Options::new(label, start, Arc::new(|h: &FakeHub| h.alive.load(Ordering::SeqCst)));
        options.set_timer = Arc::new(|_, _| Box::new(Cancel(Arc::default())));
        HubSupervisor::new(options)
    };
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (second_tx, second_rx) = tokio::sync::oneshot::channel();
    let ramsgate = make("ramsgate", first_rx);
    let dojo = make("dojo", second_rx);
    let a = ramsgate.ensure("boot");
    let b = dojo.ensure("boot");
    flush().await;
    assert_eq!(*order.lock().unwrap(), ["ramsgate:begin"]);
    first_tx.send(Err("timeout".into())).unwrap();
    assert!(a.await.is_none());
    flush().await;
    assert_eq!(*order.lock().unwrap(), ["ramsgate:begin", "ramsgate:end", "dojo:begin"]);
    second_tx.send(Ok(fake(2, true))).unwrap();
    assert_eq!(b.await.map(|h| h.id), Some(2));
}

#[tokio::test]
async fn start_receives_the_ensure_reason() {
    let reasons: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = reasons.clone();
    let h = Harness::with(vec![], move |options| {
        options.start = Arc::new(move |reason| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(reason);
                let n = seen.lock().unwrap().len();
                Ok(fake(n, true))
            }
            .boxed()
        });
    });
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(hub::DEFAULT_STABLE_UPTIME_MS);
    h.supervisor.on_exit(&hub, "exit code=0");
    flush().await;
    assert_eq!(*reasons.lock().unwrap(), ["boot", "exit: exit code=0"]);
}

#[tokio::test]
async fn the_default_clock_is_monotonic() {
    let start: hub::StartFn<FakeHub> = Arc::new(|_| async { Err("did not report ready".to_owned()) }.boxed());
    let mut options = Options::new(
        "clock_hub",
        start,
        Arc::new(|h: &FakeHub| h.alive.load(Ordering::SeqCst)),
    );
    options.set_timer = Arc::new(|_, _| Box::new(Cancel(Arc::default())));
    let supervisor = HubSupervisor::new(options);
    let before = hub::monotonic_ms();
    supervisor.ensure("boot").await;
    let due = supervisor.next_attempt_due_at();
    let first = hub::DEFAULT_BACKOFF_MS[0] as f64;
    assert!(due >= before + first && due <= hub::monotonic_ms() + first, "due={due}");
    supervisor.dispose();
}

#[test]
fn hub_switches_unset_keeps_the_default_and_off_words_switch_off() {
    assert!(hub::switch_on(None, true));
    assert!(hub::switch_on(Some(""), true));
    assert!(!hub::switch_on(Some("  "), false));
    for off in ["0", "false", "no", "off", " OFF ", "False"] {
        assert!(!hub::switch_on(Some(off), true), "{off}");
    }
    for on in ["1", "on", "yes", "true", "anything"] {
        assert!(hub::switch_on(Some(on), false), "{on}");
    }
}

#[tokio::test]
async fn a_released_hub_is_not_restarted_and_the_next_request_starts_it_at_once() {
    let h = Harness::new(vec![Outcome::Ok, Outcome::Ok]);
    let hub = h.supervisor.ensure("boot").await.unwrap();
    h.advance(10_000.0);
    assert!(h.supervisor.release(&hub, "idle"));
    hub.alive.store(false, Ordering::SeqCst);
    h.supervisor.on_exit(&hub, "exit code=0");
    flush().await;
    assert_eq!(h.starts(), 1, "no restart");
    assert!(h.live().is_empty(), "no retry timer");
    assert!(h.supervisor.current().is_none());
    assert_eq!(h.supervisor.failure_count(), 0);
    assert!(
        h.supervisor.ensure("request").await.is_some(),
        "no backoff before the next start"
    );
    assert_eq!(h.starts(), 2);
    assert!(!h.supervisor.release(&hub, "idle"), "an old hub is not the current one");
}
