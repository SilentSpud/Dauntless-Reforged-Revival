//! The game servers (`src/controllers/gameservers.ts` and `watchdog.ts`): the two persistent hubs,
//! Ramsgate on the last port of the range and the Training Grounds on the one before, each under its
//! own [`HubSupervisor`], and the hunts below them, one process per hunt.
//!
//! What the Node Director learned and this keeps:
//! - a process exists before the game listens: nothing is advertised until the runtime prints its
//!   readiness marker with this launch's id (`state: ready`);
//! - a cached "ready" is not proof a hub still serves (1.14.7 FENCING 2026-10-04): every answer
//!   checks the process is alive, and a hub's reference is cleared before its restart;
//! - hunt ports are leased per launch id ([`PortPool`]), so a late release cannot free a newer
//!   hunt's port;
//! - hunt cold starts are staggered (SECONDS_TO_WAIT_BETWEEN_GAMESERVER_STARTUP) and the two hubs'
//!   cold starts never overlap: a concurrent Ramsgate + dojo start crashed Ramsgate's
//!   FAsyncLoadingThread;
//! - empty hunts are never reaped by wall clock: the runtime ends a hunt nobody is connected to.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use futures_util::FutureExt;
use serde_json::{Map, Value, json};
use service_kit::{js, jsjson};
use tokio::time::Instant;

use crate::config::Config;
use crate::hub::{self, HubSupervisor};
use crate::hunt::{self, HuntResolver, ResolverOptions};
use crate::log;
use crate::memory::MemoryAdmission;
use crate::occupancy::{IdleStopPolicy, RuntimeLogOccupancy};
use crate::port_pool::PortPool;
use crate::process::{self, Exit, Proc};

const RAMSGATE_MAP_PATH: &str = "/Game/Maps/ramsgate/ramsgate_01_persistent";
const TRAINING_DOJO_MAP_PATH: &str = "/Game/Maps/islands/dojo/training_dojo_persistent";
pub const TRAINING_DOJO_HUNT_ID: &str = "ShatteredIsles_TrainingDojo";
const HUB_HUNT_QUEUE_WAIT_CAP: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ExpectedPlayer {
    /// As the request sent it (the reuse path adds what it is given).
    pub uid: Value,
    pub hunt_id: String,
}

#[derive(Debug)]
struct Live {
    state: State,
    ready_time: Option<DateTime<Utc>>,
    expected_players: Option<Vec<ExpectedPlayer>>,
}

#[derive(Debug)]
pub struct Gameserver {
    pub launch_id: String,
    pub port: u16,
    pub map: String,
    pub behemoth: Option<String>,
    pub matchmaker_hunt_id: Option<String>,
    pub is_ramsgate: bool,
    pub is_training_dojo: bool,
    pub start_time: DateTime<Utc>,
    pub proc: Arc<Proc>,
    live: Mutex<Live>,
}

impl Gameserver {
    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn state(&self) -> State {
        self.live().state
    }

    pub fn ready_time(&self) -> Option<DateTime<Utc>> {
        self.live().ready_time
    }

    pub fn alive(&self) -> bool {
        self.proc.alive()
    }

    pub fn is_hunt(&self) -> bool {
        !self.is_ramsgate && !self.is_training_dojo
    }

    fn ready_and_alive(&self) -> bool {
        self.state() == State::Ready && self.alive()
    }
}

/// Why a start was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// MAX_CONCURRENT_HUNT_SERVERS reached.
    NoCapacity(String),
    /// GAMESERVER_MEMORY_GUARD refused it.
    Memory(String),
    /// The process failed, exited or timed out before it was ready (or a hub is not ready).
    Startup(String),
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCapacity(m) | Self::Memory(m) | Self::Startup(m) | Self::Other(m) => f.write_str(m),
        }
    }
}

/// Where a client connects: `{host, port}` (no `host` when MY_IP is unset, as JSON.stringify drops
/// undefined).
pub fn details(host: Option<&str>, port: u16) -> Value {
    let mut map = Map::new();
    if let Some(host) = host {
        map.insert("host".into(), json!(host));
    }
    map.insert("port".into(), json!(port));
    Value::Object(map)
}

#[derive(Debug, Clone)]
struct MissingHunt {
    count: u64,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

struct Shared {
    gameservers: Vec<Arc<Gameserver>>,
    ports: PortPool,
    /// When the hunt stagger queue is free again (its tail).
    launch_free_at: Instant,
    missing: Vec<(String, MissingHunt)>,
    training_hub_wanted: bool,
    training_occupancy: Option<RuntimeLogOccupancy>,
    training_idle: IdleStopPolicy,
    training_last_request_at: i64,
    /// The Training Grounds being stopped for idleness, and when the stop was sent.
    training_idle_stopping: Option<(Arc<Gameserver>, i64)>,
    procs: HashMap<u32, Arc<Proc>>,
}

pub struct Director {
    pub config: Config,
    pub resolver: HuntResolver,
    pub ramsgate: HubSupervisor<Gameserver>,
    pub training: HubSupervisor<Gameserver>,
    memory: MemoryAdmission,
    hub_gate: tokio::sync::Mutex<()>,
    training_idle_stop_ms: i64,
    shared: Mutex<Shared>,
    persistent_ready: AtomicBool,
    stopping: AtomicBool,
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn iso(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// TRAINING_IDLE_STOP_MINUTES: 1-1440 minutes; unset or 0 keeps the Training Grounds running.
fn training_idle_stop_ms(raw: Option<&str>) -> i64 {
    let raw = js::trim(raw.unwrap_or(""));
    if raw.is_empty() {
        return 0;
    }
    match raw.parse::<i64>() {
        Ok(minutes) if raw.bytes().all(|b| b.is_ascii_digit()) && minutes <= 1440 => minutes * 60_000,
        _ => {
            log::warn(&format!(
                "[TrainingOnDemand] TRAINING_IDLE_STOP_MINUTES='{raw}' is not a whole number from 0 to 1440; the Training Grounds keep running"
            ));
            0
        }
    }
}

/// `GameArgs` is a travel URL: `map?Key=Value?Key=Value`.
pub fn arg_value<'a>(game_args: &'a str, key: &str) -> Option<&'a str> {
    game_args.split('?').skip(1).find_map(|arg| {
        arg.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .or((arg == key).then_some(""))
    })
}

/// `value.trim().length > 0` on a request value: `Ok(None)` when absent or blank, an error where
/// JavaScript would throw (`.trim` of a number).
pub fn nonblank(value: Option<&Value>) -> Result<Option<&str>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok((!js::trim(text).is_empty()).then_some(text.as_str())),
        Some(_) => Err("HuntId.trim is not a function".to_owned()),
    }
}

/// `value.length`, where it has one.
pub fn length(value: Option<&Value>) -> Option<usize> {
    match value {
        Some(Value::Array(items)) => Some(items.len()),
        Some(Value::String(text)) => Some(text.encode_utf16().count()),
        _ => None,
    }
}

fn label_of(behemoth: Option<&str>, is_ramsgate: bool, is_training_dojo: bool) -> String {
    if is_ramsgate {
        return "ramsgate".into();
    }
    if is_training_dojo {
        return "training_dojo".into();
    }
    match behemoth {
        Some(path) => {
            let last = path.rsplit('/').next().unwrap_or(path);
            last.split('.').next().unwrap_or(last).to_owned()
        }
        None => "hunt".into(),
    }
}

fn arena_week(matchmaker_hunt_id: Option<&str>) -> Option<String> {
    let id = matchmaker_hunt_id?;
    let rest = id.strip_prefix("Arena_MatchmakerHunt_")?;
    let (level, week) = rest.split_once('_')?;
    (matches!(level, "Easy" | "Hard" | "Elite") && week.len() == 3 && week.bytes().all(|b| b.is_ascii_digit()))
        .then(|| week.to_owned())
}

fn players_arg(players: Option<&[ExpectedPlayer]>) -> String {
    match players {
        Some(players) => {
            let pairs: Vec<(String, &str)> = players
                .iter()
                .map(|p| (js::string(Some(&p.uid)), p.hunt_id.as_str()))
                .collect();
            hunt::expected_player_args(pairs.iter().map(|(u, h)| (u.as_str(), *h)))
        }
        None => "NO_EXPECTED_PLAYERS".into(),
    }
}

impl Director {
    pub fn new(config: Config, tables: hunt::Tables, memory: MemoryAdmission) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Director>| {
            let end = config.port_range_end;
            let idle_ms = if config.training_hub_at_boot {
                0
            } else {
                training_idle_stop_ms(config.training_idle_stop_minutes.as_deref())
            };
            let occupancy =
                (!config.training_hub_at_boot && idle_ms > 0 && !config.runtime_log_dir.is_empty()).then(|| {
                    RuntimeLogOccupancy::new(
                        std::path::Path::new(&config.runtime_log_dir)
                            .join(format!("mysticparadox_dll_port{}.log", end.saturating_sub(1))),
                    )
                });
            if !config.training_hub_at_boot && idle_ms > 0 && occupancy.is_none() {
                log::warn(
                    "[TrainingOnDemand] TRAINING_IDLE_STOP_MINUTES is set but GAMESERVER_RUNTIME_LOG_DIR is not: the Training Grounds keep running",
                );
            }
            let idle = IdleStopPolicy::new(if occupancy.is_some() { idle_ms } else { 0 });
            let missing = weak.clone();
            let mut options = ResolverOptions::new(tables);
            options.log = Box::new(log::HuntLog);
            options.trials_rotation_minutes =
                hunt::parse_trials_rotation_minutes(config.trials_rotation_minutes.as_deref(), &log::HuntLog);
            options.on_missing_hunt = Box::new(move |id| {
                if let Some(director) = missing.upgrade() {
                    director.record_missing_hunt(id);
                }
            });
            let ramsgate = Self::hub("ramsgate", weak.clone(), true);
            let training = Self::hub("training_dojo", weak.clone(), false);
            Director {
                resolver: HuntResolver::new(options),
                ramsgate,
                training,
                memory,
                hub_gate: tokio::sync::Mutex::new(()),
                training_idle_stop_ms: idle_ms,
                shared: Mutex::new(Shared {
                    gameservers: Vec::new(),
                    ports: PortPool::new(),
                    launch_free_at: Instant::now(),
                    missing: Vec::new(),
                    training_hub_wanted: config.training_hub_at_boot,
                    training_occupancy: occupancy,
                    training_idle: idle,
                    training_last_request_at: 0,
                    training_idle_stopping: None,
                    procs: HashMap::new(),
                }),
                persistent_ready: AtomicBool::new(false),
                stopping: AtomicBool::new(false),
                config,
            }
        })
    }

    fn hub(label: &'static str, weak: Weak<Director>, is_ramsgate: bool) -> HubSupervisor<Gameserver> {
        let start: hub::StartFn<Gameserver> = Arc::new(move |reason: String| {
            let weak = weak.clone();
            async move {
                let director = weak.upgrade().ok_or_else(|| "the Director stopped".to_owned())?;
                // Both hubs' cold starts go through one gate, never at once.
                let _gate = director.hub_gate.lock().await;
                if reason != "boot" {
                    director.wait_for_hunt_launch_queue(label).await;
                }
                if !is_ramsgate {
                    // Director patch 0033: only what this process writes to the runtime log counts.
                    let mut shared = director.shared();
                    if let Some(occupancy) = &mut shared.training_occupancy {
                        occupancy.begin();
                    }
                    shared.training_idle.reset();
                }
                let map = if is_ramsgate {
                    RAMSGATE_MAP_PATH
                } else {
                    TRAINING_DOJO_MAP_PATH
                };
                director
                    .start_server(map, None, None, None, is_ramsgate, !is_ramsgate)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        });
        let mut options = hub::Options::new(label, start, Arc::new(Gameserver::alive));
        options.log = Arc::new(|level, message, error| {
            let level = match level {
                hub::Level::Info => service_kit::linelog::Level::Info,
                hub::Level::Warn => service_kit::linelog::Level::Warn,
                hub::Level::Error => service_kit::linelog::Level::Error,
            };
            match error {
                Some(error) => log::with_error(level, message, error),
                None => service_kit::linelog::log(level, message, json!({})),
            }
        });
        HubSupervisor::new(options)
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn persistent_hubs_ready(&self) -> bool {
        self.persistent_ready.load(Ordering::SeqCst)
    }

    pub fn set_persistent_hubs_ready(&self, ready: bool) {
        self.persistent_ready.store(ready, Ordering::SeqCst);
    }

    pub fn gameservers(&self) -> Vec<Arc<Gameserver>> {
        self.shared().gameservers.clone()
    }

    fn my_ip(&self) -> Option<&str> {
        self.config.my_ip.as_deref()
    }

    fn count_hunt_servers(shared: &Shared) -> usize {
        shared.gameservers.iter().filter(|s| s.is_hunt()).count()
    }

    // ---- starting a server

    /// Hub restarts wait for the hunt stagger tail as it stands now (only hunts already queued),
    /// capped so a burst of matchmaking requests cannot keep a down hub down.
    async fn wait_for_hunt_launch_queue(&self, label: &str) {
        let tail = self.shared().launch_free_at;
        let started = Instant::now();
        let capped = tail > started + HUB_HUNT_QUEUE_WAIT_CAP;
        tokio::time::sleep_until(tail.min(started + HUB_HUNT_QUEUE_WAIT_CAP)).await;
        let waited = started.elapsed().as_millis();
        if capped {
            log::warn(&format!(
                "[HubSupervisor] {label} hunt launch queue still busy after {waited} ms - cold-starting anyway"
            ));
        } else if waited >= 100 {
            log::info(&format!(
                "[HubSupervisor] {label} waited {waited} ms for the hunt launch queue before cold start"
            ));
        }
    }

    fn stagger(&self) -> Duration {
        let ms = self.config.seconds_between_startups * 1000.0;
        // setTimeout: anything that is not a positive number waits about a millisecond.
        if ms.is_finite() && ms >= 1.0 {
            Duration::from_millis(ms as u64)
        } else {
            Duration::from_millis(1)
        }
    }

    fn ready_timeout(&self) -> Duration {
        let ms = self.config.ready_timeout_ms;
        if ms.is_finite() && (1.0..=2_147_483_647.0).contains(&ms) {
            Duration::from_millis(ms as u64)
        } else {
            Duration::from_millis(1)
        }
    }

    pub async fn start_server(
        self: &Arc<Self>,
        map: &str,
        behemoth: Option<String>,
        matchmaker_hunt_id: Option<String>,
        expected_players: Option<Vec<ExpectedPlayer>>,
        is_ramsgate: bool,
        is_training_dojo: bool,
    ) -> Result<Arc<Gameserver>, StartError> {
        let is_hunt = !is_ramsgate && !is_training_dojo;
        // Only hunts are staggered: the hubs have nothing to stagger against.
        if is_hunt {
            let slot = {
                let mut shared = self.shared();
                let slot = shared.launch_free_at.max(Instant::now());
                shared.launch_free_at = slot + self.stagger();
                slot
            };
            tokio::time::sleep_until(slot).await;
        }
        let id = crate::random::uuid_v4();
        let mut reservation = None;
        let port = if is_ramsgate {
            self.config.port_range_end
        } else if is_training_dojo {
            self.config.port_range_end.saturating_sub(1)
        } else {
            // The cap is checked here, after the stagger serialized the starts, so a burst cannot race past it.
            let count = Self::count_hunt_servers(&self.shared());
            let cap = self.config.max_concurrent_hunt_servers;
            if count as f64 >= cap {
                return Err(StartError::NoCapacity(format!(
                    "Hunt server capacity reached ({count}/{})",
                    jsjson::number(cap)
                )));
            }
            // Director patch 0022: refused like a full cap, before any port or process is taken.
            match self.memory.reserve() {
                Ok(r) => reservation = Some(r),
                Err(error) => {
                    log::warn(&format!("[MemoryGuard] refusing a new hunt ({count} running): {error}"));
                    return Err(StartError::Memory(error.to_string()));
                }
            }
            match self.shared().ports.take(&id) {
                Some(port) => port,
                None => {
                    self.give_back(reservation);
                    return Err(StartError::Other("No free ports left!".into()));
                }
            }
        };
        let label = label_of(behemoth.as_deref(), is_ramsgate, is_training_dojo);
        let fail_spawn = |this: &Self, message: String| {
            if is_hunt {
                this.shared().ports.release(port, &id);
            }
            this.give_back(reservation);
            StartError::Other(message)
        };

        let log_path = process::log_path(&self.config.gameserver_log_dir, port, &label);
        let mut log_file = match std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(file) => file,
            Err(error) => return Err(fail_spawn(self, format!("{}: {error}", log_path.display()))),
        };
        let players_text = players_arg(expected_players.as_deref());
        {
            use std::io::Write;
            let _ = write!(
                log_file,
                "=== Gameserver launch {} (local time) ===\nport={port} map={map} behemoth={} mmHunt={} players={players_text}\n\n",
                Local::now().format("%-m/%-d/%Y, %-I:%M:%S %p"),
                behemoth.as_deref().unwrap_or("NO_BEHEMOTH"),
                matchmaker_hunt_id.as_deref().unwrap_or("NO_MM_HUNTID"),
            );
        }

        // An arena hunt keeps the week it was launched for; the hubs follow the rotating schedule.
        let mut env = self.config.child_env.clone();
        env.retain(|(k, _)| k != "MYSTICPARADOX_GAMESERVER_LAUNCH_ID" && k != "MYSTICPARADOX_TRIALS_WEEK");
        env.push(("MYSTICPARADOX_GAMESERVER_LAUNCH_ID".into(), id.clone()));
        if let Some(week) = arena_week(matchmaker_hunt_id.as_deref()) {
            env.push(("MYSTICPARADOX_TRIALS_WEEK".into(), week));
        }
        let (Some(program), Some(key)) = (
            self.config.gameserver_binary.clone(),
            self.config.metagame_api_key.clone(),
        ) else {
            return Err(fail_spawn(
                self,
                "GAMESERVER_BINARY_PATH and METAGAME_API_KEY must be set to start a game server".into(),
            ));
        };
        let args = launch_args(&Launch {
            key: &key,
            port,
            map,
            behemoth: behemoth.as_deref(),
            matchmaker_hunt_id: matchmaker_hunt_id.as_deref(),
            players: &players_text,
            my_ip: self.my_ip(),
            log_cmds: &self.config.log_cmds,
        });

        let weak = Arc::downgrade(self);
        let exit_id = id.clone();
        let exit_label = label.clone();
        let pid_slot: Arc<Mutex<Option<u32>>> = Arc::default();
        let exit_pid = pid_slot.clone();
        let on_exit: process::OnExit = Box::new(move |exit, log_lines| {
            let pid = *exit_pid.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let message = format!(
                "Gameserver process exited port={port} label={exit_label} pid={} code={} signal={}",
                pid.map_or_else(|| "undefined".to_owned(), |p| p.to_string()),
                exit.code_text(),
                exit.signal_text()
            );
            log::warn(&message);
            let _ = log_lines.send(
                format!(
                    "
=== {message} ===
"
                )
                .into_bytes(),
            );
            if let Some(director) = weak.upgrade() {
                director.on_process_exit(&exit_id, port, is_hunt, &exit);
            }
        });
        let spec = process::Spec {
            program,
            args,
            env,
            windows_hide: self.config.windows_hide,
            log: log_file,
            launch_id: id.clone(),
            port,
        };
        let spawned = match process::spawn(spec, on_exit) {
            Ok(spawned) => spawned,
            Err(error) => return Err(fail_spawn(self, error)),
        };
        let pid = spawned.proc.pid;
        *pid_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = pid;
        let pid_text = pid.map_or_else(|| "unknown".to_owned(), |p| p.to_string());
        {
            let mut shared = self.shared();
            if is_hunt {
                shared.ports.attach_pid(port, &id, pid);
            }
            if let Some(pid) = pid {
                shared.procs.insert(pid, spawned.proc.clone());
            }
        }
        let _ = spawned.log.send(format!("pid={pid_text}\n\n").into_bytes());
        log::info(&format!(
            "{label} spawned on port {port} (pid={pid_text}) -> {}",
            log_path.display()
        ));

        let server = Arc::new(Gameserver {
            launch_id: id.clone(),
            port,
            map: map.to_owned(),
            behemoth,
            matchmaker_hunt_id,
            is_ramsgate,
            is_training_dojo,
            start_time: Utc::now(),
            proc: spawned.proc.clone(),
            live: Mutex::new(Live {
                state: State::Starting,
                ready_time: None,
                expected_players,
            }),
        });
        self.shared().gameservers.push(server.clone());

        match self.wait_ready(spawned.ready, spawned.exit, port, &label).await {
            Ok(()) => {
                let now = Utc::now();
                {
                    let mut live = server.live();
                    live.state = State::Ready;
                    live.ready_time = Some(now);
                }
                log::info(&format!(
                    "{label} ready on {}:{port} after {}ms",
                    self.my_ip().unwrap_or("undefined"),
                    (now - server.start_time).num_milliseconds()
                ));
                Ok(server)
            }
            Err(message) => {
                server.live().state = State::Failed;
                self.shared().gameservers.retain(|s| !Arc::ptr_eq(s, &server));
                self.give_back(reservation);
                // A hunt's port goes back only once its process is gone: a timed-out child still
                // alive is stopped, and its exit returns the port (owner-checked).
                if server.alive() {
                    let _ = server.proc.terminate();
                } else if is_hunt {
                    self.shared().ports.release(port, &id);
                }
                Err(StartError::Startup(message))
            }
        }
    }

    fn give_back(&self, reservation: Option<crate::memory::Reservation>) {
        if let Some(reservation) = reservation {
            self.memory.give_back(reservation);
        }
    }

    async fn wait_ready(
        &self,
        ready: tokio::sync::oneshot::Receiver<()>,
        mut exit: tokio::sync::watch::Receiver<Option<Exit>>,
        port: u16,
        label: &str,
    ) -> Result<(), String> {
        let exited = async {
            loop {
                if let Some(exit) = exit.borrow().clone() {
                    return exit;
                }
                if exit.changed().await.is_err() {
                    return Exit {
                        code: None,
                        signal: None,
                    };
                }
            }
        };
        tokio::select! {
            biased;
            result = ready => match result {
                Ok(()) => Ok(()),
                // The reader ended without a marker: the process closed its output; its exit follows.
                Err(_) => {
                    let exit = exit.wait_for(Option::is_some).await.ok().and_then(|e| e.clone())
                        .unwrap_or(Exit { code: None, signal: None });
                    Err(exited_message(label, port, &exit))
                }
            },
            exit = exited => Err(exited_message(label, port, &exit)),
            () = tokio::time::sleep(self.ready_timeout()) => Err(format!(
                "{label} port {port} did not report ready within {}ms",
                jsjson::number(self.config.ready_timeout_ms)
            )),
        }
    }

    fn on_process_exit(self: &Arc<Self>, launch_id: &str, port: u16, is_hunt: bool, exit: &Exit) {
        // A hunt is reclaimed at once (the process is confirmed gone; the release is owner-checked).
        // A hub's exit reaches its supervisor at once; HUB_PROMPT_RESTART=0 leaves it to the watchdog.
        if is_hunt {
            let mut shared = self.shared();
            shared.gameservers.retain(|s| s.launch_id != launch_id);
            shared.ports.release(port, launch_id);
        } else if self.config.hub_prompt_restart {
            let server = self
                .shared()
                .gameservers
                .iter()
                .find(|s| s.launch_id == launch_id)
                .cloned();
            if let Some(server) = server {
                self.cleanup_server(
                    &server,
                    &format!("exit code={} signal={}", exit.code_text(), exit.signal_text()),
                );
            }
        }
    }

    // ---- reclaiming

    /// Drops a server that is gone and hands a hub to its supervisor. Idempotent: the exit event,
    /// the getters and the watchdog may all report the same server.
    pub fn cleanup_server(self: &Arc<Self>, server: &Arc<Gameserver>, reason: &str) {
        if self.stopping.load(Ordering::SeqCst) {
            self.shared().gameservers.retain(|s| !Arc::ptr_eq(s, server));
            return;
        }
        let listed = {
            let mut shared = self.shared();
            let listed = shared.gameservers.iter().any(|s| Arc::ptr_eq(s, server));
            if listed {
                shared.gameservers.retain(|s| !Arc::ptr_eq(s, server));
            }
            listed
        };
        // Director patch 0033: stopped because nobody was in them; the next request starts them again.
        if server.is_training_dojo {
            let stopping = {
                let mut shared = self.shared();
                let stopping = shared
                    .training_idle_stopping
                    .as_ref()
                    .is_some_and(|(s, _)| Arc::ptr_eq(s, server));
                if stopping {
                    shared.training_idle_stopping = None;
                    shared.training_idle.reset();
                }
                stopping
            };
            if stopping {
                if self.training.release(server, reason) {
                    self.shared().training_hub_wanted = false;
                    log::info(&format!(
                        "[TrainingOnDemand] the Training Grounds stopped (no player for {} min); they start again on the next request",
                        jsjson::number(self.training_idle_stop_ms as f64 / 60_000.0)
                    ));
                }
                return;
            }
        }
        if server.is_ramsgate || server.is_training_dojo {
            let hub = if server.is_ramsgate {
                &self.ramsgate
            } else {
                &self.training
            };
            if hub.current().is_some_and(|c| Arc::ptr_eq(&c, server)) {
                log::warn(&if server.is_ramsgate {
                    format!("RAMSGATE HAS FALLEN ({reason})! Restarting!")
                } else {
                    format!("Training Dojo down ({reason})! Restarting!")
                });
                hub.on_exit(server, reason);
            } else if listed {
                // Never current: it exited during its own readiness wait, whose failure drives the retry.
                log::info(&format!(
                    "[HubSupervisor] {} pid={} exited during startup ({reason}); its start attempt handles the retry",
                    if server.is_ramsgate {
                        "ramsgate"
                    } else {
                        "training_dojo"
                    },
                    server
                        .proc
                        .pid
                        .map_or_else(|| "undefined".to_owned(), |p| p.to_string())
                ));
            }
            return;
        }
        // Owner-checked: a no-op when the exit already returned the port (or a newer hunt holds it).
        self.shared().ports.release(server.port, &server.launch_id);
    }

    /// The watchdog's fallback for a hunt whose exit never arrived after it left the list: its lease
    /// goes back once its process is gone.
    pub fn reclaim_orphaned_port_leases(&self) {
        let reclaimed = {
            let mut guard = self.shared();
            let shared = &mut *guard;
            let owners: Vec<String> = shared.gameservers.iter().map(|s| s.launch_id.clone()).collect();
            let procs = &shared.procs;
            shared.ports.reclaim(
                |owner| owners.iter().any(|o| o == owner),
                |pid| procs.get(&pid).is_some_and(|p| p.alive()),
            )
        };
        for lease in reclaimed {
            log::warn(&format!(
                "[PortPool] reclaimed port {} from launch {} (pid={} gone, no exit event)",
                lease.port,
                lease.owner,
                lease.pid.map_or_else(|| "none".to_owned(), |p| p.to_string())
            ));
        }
    }

    pub fn tick_persistent_hubs(self: &Arc<Self>) {
        if self.config.ramsgate_hub_enabled {
            self.ramsgate.tick();
        }
        // An on-demand Training hub is left alone until someone asked for it.
        if self.shared().training_hub_wanted {
            self.training.tick();
        }
        self.tick_training_idle_stop();
    }

    /// Director patch 0033: stops the on-demand Training Grounds once nobody has been in them for
    /// TRAINING_IDLE_STOP_MINUTES (checked once a minute). Their exit then releases the hub.
    fn tick_training_idle_stop(self: &Arc<Self>) {
        let now = now_ms();
        let mut shared = self.shared();
        if shared.training_occupancy.is_none() {
            return;
        }
        if let Some((server, at)) = shared.training_idle_stopping.clone() {
            if server.alive() && now - at >= 120_000 {
                log::warn(&format!(
                    "[TrainingOnDemand] the Training Grounds (pid={}) did not stop within 2 min; leaving them running",
                    server
                        .proc
                        .pid
                        .map_or_else(|| "undefined".to_owned(), |p| p.to_string())
                ));
                shared.training_idle_stopping = None;
                shared.training_idle.reset();
            }
            // Gone: the exit is on its way.
            return;
        }
        let current = self.training.current();
        let Some(server) = current.filter(|s| shared.training_hub_wanted && s.ready_and_alive()) else {
            shared.training_idle.reset();
            return;
        };
        let Some(ready_at) = server.ready_time().map(|t| t.timestamp_millis()) else {
            shared.training_idle.reset();
            return;
        };
        let count = shared.training_occupancy.as_mut().and_then(RuntimeLogOccupancy::poll);
        let last_request = shared.training_last_request_at;
        if !shared.training_idle.should_stop(now, count, ready_at, last_request) {
            return;
        }
        shared.training_idle_stopping = Some((server.clone(), now));
        let pid = server
            .proc
            .pid
            .map_or_else(|| "undefined".to_owned(), |p| p.to_string());
        log::info(&format!(
            "[TrainingOnDemand] no player in the Training Grounds for {} min - stopping them (pid={pid})",
            jsjson::number(self.training_idle_stop_ms as f64 / 60_000.0)
        ));
        if let Err(error) = server.proc.terminate() {
            log::with_error(
                service_kit::linelog::Level::Warn,
                "[TrainingOnDemand] could not stop the Training Grounds",
                &error,
            );
            shared.training_idle_stopping = None;
        }
    }

    /// One watchdog pass (`src/controllers/watchdog.ts`), once a minute.
    pub async fn run_watchdog(self: &Arc<Self>) {
        log::info("Running Gameserver Watchdog!");
        for server in self.gameservers() {
            if !server.alive() {
                log::warn(&format!("Cleaning up dead gameserver on port {}", server.port));
                self.cleanup_server(&server, "watchdog: process gone");
            }
        }
        self.reclaim_orphaned_port_leases();
        self.tick_persistent_hubs();
        // Keeps content gaps visible: a missing hunt row is otherwise one error per client retry.
        self.log_missing_hunt_requests();
        self.forget_exited_procs();
    }

    fn forget_exited_procs(&self) {
        let mut guard = self.shared();
        let shared = &mut *guard;
        let held: Vec<u32> = shared
            .gameservers
            .iter()
            .filter_map(|s| s.proc.pid)
            .chain(shared.ports.held_pids())
            .collect();
        shared.procs.retain(|pid, p| p.alive() || held.contains(pid));
    }

    // ---- answers

    /// The hubs' live status for matchmaking revalidation: null unless ready and still alive.
    pub fn persistent_hub_status(&self) -> Value {
        let describe = |server: Option<Arc<Gameserver>>| match server {
            Some(s) if s.ready_and_alive() => {
                let mut map = Map::new();
                map.insert("ready".into(), json!(true));
                if let Some(host) = self.my_ip() {
                    map.insert("host".into(), json!(host));
                }
                map.insert("port".into(), json!(s.port));
                if let Some(pid) = s.proc.pid {
                    map.insert("processId".into(), json!(pid));
                }
                Value::Object(map)
            }
            _ => Value::Null,
        };
        json!({
            "ramsgate": describe(self.ramsgate.current()),
            "trainingDojo": describe(self.training.current()),
        })
    }

    /// Running game servers for the launcher's server panel (through the backend's ServerStatus).
    /// Ready and live ones only; no process ids, addresses or launch ids.
    pub fn instances_for_status(&self) -> Value {
        let max = self.config.public_hunt_max_players;
        Value::Array(
            self.gameservers()
                .iter()
                .filter(|s| s.ready_and_alive())
                .map(|s| {
                    let kind = if s.is_ramsgate {
                        "city"
                    } else if s.is_training_dojo {
                        "dojo"
                    } else {
                        "hunt"
                    };
                    let live = s.live();
                    let players = live.expected_players.as_ref();
                    let max_players = if s.is_hunt() {
                        if max.is_finite() { json!(max) } else { Value::Null }
                    } else {
                        json!(0)
                    };
                    json!({
                        "id": format!("{kind}-{}", s.port),
                        "kind": kind,
                        "map": s.map,
                        "behemoth": s.behemoth,
                        "players": players.map_or(0, Vec::len),
                        "maxPlayers": jsjson::to_js(&max_players),
                        "playerIds": players.map_or_else(Vec::new, |p| p.iter().map(|p| p.uid.clone()).collect()),
                        "startedAt": iso(live.ready_time.unwrap_or(s.start_time)),
                    })
                })
                .collect(),
        )
    }

    /// A hub's address when it is ready and alive. A dead hub is reported to its supervisor, a
    /// missing one asked for; the caller gets "not ready" either way.
    fn hub_connection_details(self: &Arc<Self>, training: bool, name: &str) -> Result<Value, StartError> {
        let hub = if training { &self.training } else { &self.ramsgate };
        let server = hub.current();
        if let Some(server) = &server
            && server.ready_and_alive()
        {
            return Ok(details(self.my_ip(), server.port));
        }
        if self.config.hub_prompt_restart {
            match &server {
                Some(server) if !server.alive() => self.cleanup_server(server, "getter: process gone"),
                None => drop(hub.ensure("request")),
                Some(_) => {}
            }
        }
        Err(StartError::Startup(format!("{name} is not ready")))
    }

    pub fn ramsgate_connection_details(self: &Arc<Self>) -> Result<Value, StartError> {
        if !self.config.ramsgate_hub_enabled {
            return Err(StartError::Startup(
                "the dedicated Ramsgate hub is switched off (RAMSGATE_HUB=off)".into(),
            ));
        }
        self.hub_connection_details(false, "Ramsgate")
    }

    /// The matchmaker's Training getter. An on-demand hub that is not up yet is started here and the
    /// request waits for it; a failed start still answers "not ready".
    pub async fn training_connection_details_for_request(self: &Arc<Self>) -> Result<Value, StartError> {
        if self.config.training_hub_at_boot {
            return self.hub_connection_details(true, "Training Dojo");
        }
        self.shared().training_last_request_at = now_ms();
        // A request during an idle stop waits for that process to go, then starts a fresh one.
        let until = Instant::now() + Duration::from_secs(30);
        while self.shared().training_idle_stopping.is_some() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let first = {
            let mut shared = self.shared();
            let first = !shared.training_hub_wanted;
            shared.training_hub_wanted = true;
            first
        };
        if first {
            log::info(
                "[TrainingHub] on-demand Training hub requested for the first time (TRAINING_HUB=off) - starting it",
            );
        }
        if !self.training.current().is_some_and(|s| s.ready_and_alive()) {
            self.training.ensure("request").await;
        }
        self.hub_connection_details(true, "Training Dojo")
    }

    /// Public hunt server reuse (2026-07-19, ENABLE_PUBLIC_HUNT_REUSE=1): a live, ready, non-full
    /// server already running this player hunt id takes the joiners. Keyed on the player hunt id, not
    /// the matchmaker row (picked at random, which would split a party). The roster only grows.
    pub fn try_reuse_shared_hunt_server(&self, player_hunt_id: &str, joiners: Option<&Value>) -> Option<Value> {
        if !self.config.enable_public_hunt_reuse || js::trim(player_hunt_id).is_empty() {
            return None;
        }
        let max = self.config.public_hunt_max_players;
        let candidate = self.gameservers().into_iter().find(|s| {
            if s.state() != State::Ready || !s.is_hunt() {
                return false;
            }
            let live = s.live();
            live.expected_players.as_ref().is_some_and(|players| {
                !players.is_empty() && players[0].hunt_id == player_hunt_id && (players.len() as f64) < max
            })
        })?;
        let uids: Vec<Value> = match joiners {
            Some(Value::Array(items)) => items.clone(),
            // for...of over a string walks its characters
            Some(Value::String(text)) => text.chars().map(|c| json!(c.to_string())).collect(),
            _ => Vec::new(),
        };
        let roster = {
            let mut live = candidate.live();
            let players = live.expected_players.get_or_insert_with(Vec::new);
            for uid in uids {
                if !players.iter().any(|p| p.uid == uid) {
                    players.push(ExpectedPlayer {
                        uid,
                        hunt_id: player_hunt_id.to_owned(),
                    });
                }
            }
            players.len()
        };
        log::info(&format!(
            "[PublicHuntReuse] joining existing server port={} pid={} huntId={player_hunt_id} roster={roster}/{}",
            candidate.port,
            candidate
                .proc
                .pid
                .map_or_else(|| "undefined".to_owned(), |p| p.to_string()),
            jsjson::number(max)
        ));
        Some(details(self.my_ip(), candidate.port))
    }

    /// An ISLAND request with travel arguments: the map, `?MonsterClass=` and `?HuntID=` from them.
    pub async fn startup_with_args(
        self: &Arc<Self>,
        game_args: &str,
        hunt_id: Option<&Value>,
        expected: Option<&Value>,
    ) -> Result<Value, StartError> {
        let map = game_args.split('?').next().unwrap_or("");
        let behemoth = arg_value(game_args, "MonsterClass").map(str::to_owned);
        let matchmaker_hunt_id = match arg_value(game_args, "HuntID").filter(|v| !js::trim(v).is_empty()) {
            Some(id) => Some(id.to_owned()),
            None => match nonblank(hunt_id).map_err(StartError::Other)? {
                Some(hunt) => Some(self.resolver.matchmaker_hunt_id_for(hunt).map_err(StartError::Other)?),
                None => None,
            },
        };
        let players = expected_players(expected, hunt_id).map_err(StartError::Other)?;
        let server = self
            .start_server(map, behemoth, matchmaker_hunt_id, players, false, false)
            .await?;
        Ok(details(self.my_ip(), server.port))
    }

    /// A player hunt by its id and party: the manifest, then a server for it.
    pub async fn startup_with_hunt_id(self: &Arc<Self>, hunt_id: &str, expected: &Value) -> Result<Value, StartError> {
        let manifest = self
            .resolver
            .resolve_manifest(Some(&json!(hunt_id)), Some(expected))
            .map_err(StartError::Other)?;
        let text = |name: &str| manifest.get(name).map(|v| js::string(Some(v))).unwrap_or_default();
        let behemoth = Some(text("behemothPath")).filter(|b| b != "NO_BEHEMOTH");
        let players = expected
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|uid| ExpectedPlayer {
                        uid: uid.clone(),
                        hunt_id: hunt_id.to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let server = self
            .start_server(
                &text("mapPath"),
                behemoth,
                Some(text("matchmakerHuntId")),
                Some(players),
                false,
                false,
            )
            .await?;
        Ok(details(self.my_ip(), server.port))
    }

    // ---- hunt-table coverage

    fn record_missing_hunt(&self, player_hunt_id: &str) {
        let now = Utc::now();
        let mut shared = self.shared();
        if let Some((_, entry)) = shared.missing.iter_mut().find(|(id, _)| id == player_hunt_id) {
            entry.count += 1;
            entry.last_seen = now;
            return;
        }
        shared.missing.push((
            player_hunt_id.to_owned(),
            MissingHunt {
                count: 1,
                first_seen: now,
                last_seen: now,
            },
        ));
        drop(shared);
        // First sighting only: the client retries hard, and one actionable line beats fifty.
        log::error(&format!(
            "[HuntTableGap] Client requested HuntId '{player_hunt_id}' which is absent from player_hunts_table. This hunt CANNOT start. The vendored tables predate this content — re-export with CatalogExporter EXPORT_HUNTS=1 and import the new rows."
        ));
    }

    /// The missing hunt ids, most requested first.
    pub fn missing_hunt_requests(&self) -> Vec<Value> {
        let mut entries = self.shared().missing.clone();
        entries.sort_by_key(|e| std::cmp::Reverse(e.1.count));
        entries
            .into_iter()
            .map(|(id, info)| {
                json!({
                    "huntId": id,
                    "count": info.count,
                    "firstSeen": iso(info.first_seen),
                    "lastSeen": iso(info.last_seen),
                })
            })
            .collect()
    }

    pub fn log_missing_hunt_requests(&self) {
        let entries = self.missing_hunt_requests();
        if entries.is_empty() {
            return;
        }
        let summary = entries
            .iter()
            .map(|e| format!("{} x{}", js::string(e.get("huntId")), js::string(e.get("count"))))
            .collect::<Vec<_>>()
            .join(", ");
        log::warn(&format!(
            "[HuntTableGap] {} hunt id(s) requested but missing from player_hunts_table: {summary}",
            entries.len()
        ));
    }

    pub fn validate_hunt_tables(&self) -> hunt::TableAudit {
        let summary = hunt::validate_tables(&self.resolver);
        let coverage = &summary.coverage;
        log::info(&format!(
            "[HuntTableAudit] playerRows={} directRows={} tagRoutedRows={} tagRoutedRefs={} matchmakerRows={} (+{} aux tables) problems={}",
            summary.player_rows,
            summary.direct_player_rows,
            summary.tag_routed_player_rows,
            summary.tag_routed_references,
            summary.matchmaker_rows,
            self.resolver.tables.matchmaker_sources.len().saturating_sub(1),
            summary.problems.len()
        ));
        log::info(&format!(
            "[HuntLaunchCoverage] resolvable={}/{} referencedMatchmakerRows={} uniqueMaps={} gameModes={} unresolved={}",
            coverage.resolvable_player_hunts,
            coverage.total_player_hunts,
            coverage.referenced_matchmaker_rows,
            coverage.unique_map_assets.len(),
            coverage.game_mode_overrides.len(),
            coverage.unresolved_player_hunts.len()
        ));
        if !coverage.unresolved_player_hunts.is_empty() {
            log::warn(&format!(
                "[HuntLaunchCoverage] unsupported player hunt rows: {}",
                coverage.unresolved_player_hunts.join(", ")
            ));
        }
        let ids: Vec<&str> = self.resolver.tables.player_rows.keys().map(String::as_str).collect();
        for line in hunt::family_coverage(&ids) {
            log::info(&line);
        }
        for problem in summary.problems.iter().take(20) {
            log::warn(&format!("[HuntTableAudit] {problem}"));
        }
        if summary.problems.len() > 20 {
            log::warn(&format!(
                "[HuntTableAudit] {} additional problems omitted",
                summary.problems.len() - 20
            ));
        }
        summary
    }

    /// At start: the audit, the hunt ports, then the hubs one after the other, Ramsgate first. A
    /// failed hub start is reported, not fatal: the healthy sibling keeps serving and the supervisor
    /// retries with backoff.
    pub async fn startup(self: &Arc<Self>) {
        self.validate_hunt_tables();
        {
            let mut shared = self.shared();
            let (begin, end) = (self.config.port_range_begin, self.config.port_range_end);
            for port in begin..=end.saturating_sub(2) {
                if end >= 2 {
                    shared.ports.add(port);
                }
            }
        }
        let mut failures = Vec::new();
        if self.config.ramsgate_hub_enabled {
            if self.ramsgate.ensure("boot").await.is_none() {
                failures.push("ramsgate");
            }
        } else {
            log::info(
                "[RamsgateHub] RAMSGATE_HUB=off - no dedicated Ramsgate hub (realms run their own on the host's PC)",
            );
        }
        if self.config.training_hub_at_boot {
            if self.training.ensure("boot").await.is_none() {
                failures.push("training_dojo");
            }
        } else {
            log::info(
                "[TrainingHub] TRAINING_HUB=off - the dedicated Training hub starts on its first request (fallback for hosted Training)",
            );
        }
        if !failures.is_empty() {
            log::warn(&format!(
                "Persistent hub startup reported failures ({}) - continuing with whichever hubs are up; retrying with backoff until up",
                failures.join("; ")
            ));
        }
    }

    /// Stops every game server and restarts none (the tests). The service itself leaves them to
    /// the host kit, as the Node Director did: its job object or process group ends them.
    pub fn stop_all(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.dispose();
        for server in self.gameservers() {
            let _ = server.proc.terminate();
        }
    }

    pub fn dispose(&self) {
        self.ramsgate.dispose();
        self.training.dispose();
    }
}

fn exited_message(label: &str, port: u16, exit: &Exit) -> String {
    format!(
        "{label} port {port} exited before ready (code={}, signal={})",
        exit.code_text(),
        exit.signal_text()
    )
}

/// `GetExpectedPlayers`: the party with the player hunt id, or `None` without one of them.
fn expected_players(ids: Option<&Value>, hunt_id: Option<&Value>) -> Result<Option<Vec<ExpectedPlayer>>, String> {
    if matches!(ids, None | Some(Value::Null)) || length(ids) == Some(0) {
        return Ok(None);
    }
    let Some(hunt) = nonblank(hunt_id)? else {
        return Ok(None);
    };
    match ids {
        Some(Value::Array(items)) => Ok(Some(
            items
                .iter()
                .map(|uid| ExpectedPlayer {
                    uid: uid.clone(),
                    hunt_id: hunt.to_owned(),
                })
                .collect(),
        )),
        _ => Err("PlayerIds.map is not a function".to_owned()),
    }
}

/// What one game server is started with.
pub struct Launch<'a> {
    pub key: &'a str,
    pub port: u16,
    pub map: &'a str,
    pub behemoth: Option<&'a str>,
    pub matchmaker_hunt_id: Option<&'a str>,
    /// `uid:huntId,...` or NO_EXPECTED_PLAYERS.
    pub players: &'a str,
    pub my_ip: Option<&'a str>,
    pub log_cmds: &'a str,
}

/// The game server's arguments, in the order the runtime reads them: the gameserver key, the port,
/// the map, the Behemoth, the matchmaker row, the party, the address it advertises, then the switches.
pub fn launch_args(launch: &Launch<'_>) -> Vec<String> {
    let mut args = vec![
        launch.key.to_owned(),
        launch.port.to_string(),
        launch.map.to_owned(),
        launch.behemoth.unwrap_or("NO_BEHEMOTH").to_owned(),
        launch.matchmaker_hunt_id.unwrap_or("NO_MM_HUNTID").to_owned(),
        launch.players.to_owned(),
        format!("{}:{}", launch.my_ip.unwrap_or("undefined"), launch.port),
    ];
    args.extend(standard_args(launch.log_cmds));
    args
}

/// Every game server's switches after its own arguments.
///
/// -nosound: a dedicated hub must not initialise audio (it played the launch audio on the host,
/// 1.14.7 2026-10-03). -LogCmds: LogOnline stays at Verbose, not VeryVerbose, which formatted stale
/// connection names every tick on long-lived Ramsgate sessions and faulted (2026-07-21); LogLoad Off,
/// as a headless client build floods it.
pub fn standard_args(log_cmds: &str) -> Vec<String> {
    [
        "-EpicPortal",
        "-server",
        "-nullrhi",
        "-warp",
        "-nosound",
        "-RepDriverEnable",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(format!("-LogCmds=\"{log_cmds}\"")))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_travel_arguments_as_the_node_director() {
        let args = "/Game/Maps/x/y?MonsterClass=/Game/Monsters/a/b_bp.b_bp_C?HuntID=MM_1?Flag";
        assert_eq!(arg_value(args, "MonsterClass"), Some("/Game/Monsters/a/b_bp.b_bp_C"));
        assert_eq!(arg_value(args, "HuntID"), Some("MM_1"));
        assert_eq!(arg_value(args, "Flag"), Some(""));
        assert_eq!(arg_value(args, "Missing"), None);
        assert_eq!(arg_value("a?K=v=w", "K"), Some("v=w"));
        assert_eq!(
            label_of(Some("/Game/Monsters/fulgar/fulgar_bp.fulgar_bp_C"), false, false),
            "fulgar_bp"
        );
        assert_eq!(label_of(None, false, false), "hunt");
        assert_eq!(
            arena_week(Some("Arena_MatchmakerHunt_Hard_042")).as_deref(),
            Some("042")
        );
        assert_eq!(arena_week(Some("Arena_MatchmakerHunt_Test_042")), None);
    }

    #[test]
    fn the_standard_switches_end_with_the_log_categories() {
        let args = standard_args("LogLoad Off");
        assert_eq!(args.last().map(String::as_str), Some("-LogCmds=\"LogLoad Off\""));
        assert!(args.contains(&"-nosound".to_owned()));
    }
}
