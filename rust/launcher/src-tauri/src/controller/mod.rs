//! The launcher's state machine (row L38; Reforged's `src/main/controller.ts`). It owns every piece of
//! state; the page draws snapshots and sends intents through the commands.
//!
//! Two server modes exist (see `core::invite`):
//!  - private (invite v1): the host's PC over Tailscale, plain HTTP to the host's gateway;
//!  - public (invite v2): a server on a public IP. Everything goes over TLS to its gateway, pinned to the
//!    invite's certificate fingerprint.
//!
//! Either way, while the game runs a local relay on 127.0.0.1:61100 carries the game's own traffic
//! (`net::relay`): the runtime sends it there because the launcher sets `REFORGED_BACKEND_URL`.
//!
//! The state sits behind one mutex that is only held between awaits, never across one: each locked
//! stretch is what one synchronous stretch of the TypeScript controller was on Node's event loop, so the
//! same interleavings are possible and no others (DR's design).
//!
//! The methods are split by the row that fills them, so no two rows share a file:
//!
//! - this file: join, connect, account, status, news and art, settings, links, play and shutdown (RA13,
//!   ported from DR's `controller/mod.rs` at e1c5fec and Reforged's `controller.ts`);
//! - `game.rs`: the game folder (RA13, on RA9's `platform::game_folder`);
//! - `runtime.rs`: the server's game runtime (RA13, on RA7's `net::runtime_files`);
//! - `host.rs`: the Host page (RA12, on `platform::hostkit`, `hostserver`, `requirements` and
//!   `loader_session`). It reads and sets `State::host_server_up`, `State::dumping` and
//!   `State::loader_session`, which this file acts on when the game exits, a runtime is installed and the
//!   launcher shuts down; PLAY asks it for the runtime loader's leftovers first (RD9).

pub mod game;
pub mod host;
pub mod platform;
pub mod runtime;
pub mod stored;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use protocol::invite::{Invite, ServerMode};
use protocol::manifest::RuntimeManifest;
use protocol::status::limited_view;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use self::platform::{Platform, Prepared};
use self::stored::{Stored, StoredServer, set_backup_offered, set_server, set_username};
use crate::bridge::types::{
    ActionResult, Background, Branding, ConnectProblem, ErrorCode, ExternalTarget, GameInfo, GameProblem, InviteCheck,
    Language, LauncherError, NewsItem, NoticeCode, Phase, RegisterOutcome, RuntimeInfo, RuntimeState, SavedServerInfo,
    ServerInfo, Settings, SettingsPatch, Snapshot, SnapshotAccount, SnapshotApp, SnapshotConnect, SnapshotRunning,
    TaskProgress,
};
use crate::core::game_language::game_locale;
use crate::core::invite::{InviteError, is_loopback_host, parse_invite};
use crate::core::pins::{EXE_NAME, WIN64_RELATIVE_DIR};
use crate::core::status::ServerStatus;
use crate::core::username::{UsernameCheck, check_username, extract_account_key};
use crate::net::hostapi::{
    RegisterResult, StatusResult, fetch_branding_image, fetch_branding_index, fetch_news, fetch_server_status,
    fetch_user_info, register_account,
};
use crate::net::http::Endpoint;
use crate::net::relay::{DEFAULT_QOS_PORT, LogLevel, Relay, RelayError, RelayOptions, RelayTarget};
use crate::net::relay_owner::game_owner_check;
use crate::net::runtime_files::{RuntimeErrorCode, install_local_runtime};
use crate::net::verify::{Abort, HashCache};
use crate::platform::dialogs::KeepOpen;
use crate::platform::game_config::{ApplyConfig, apply_game_config};
use crate::platform::game_folder::StoreEnv;
use crate::platform::hostserver::Probe;
use crate::platform::keystore::{BackupFile, Encryptor, KeySlot, KeyStore, KeyStoreError, backup_file_text};
use crate::platform::launch::{
    GameProcess, LaunchRuntime, LaunchSettings, STATUS_CONTROL_C_EXIT, Spawn, build_launch_args, describe_launch,
    game_exit_detail, launch_environment,
};
use crate::platform::links::{PROJECT_URL, fixed_link_url, is_allowed_external_url};
use crate::platform::loader_session::{LoaderSession, SessionKind};
use crate::platform::log;
use crate::platform::settings::{Platform as HostPlatform, SettingsStore};

pub type ControllerHandle = Arc<Controller>;

/// What the app knows before the controller starts (the fixed half of `Platform` in TypeScript).
pub struct Config {
    pub user_data: PathBuf,
    /// Where the automatic account key backups go (`Documents/Dauntless Reforged Revival/Account Keys`)
    pub documents: Option<PathBuf>,
    #[allow(dead_code, reason = "Reforged's; nothing reads the app's resources yet")]
    pub resources_dir: PathBuf,
    pub host: HostPlatform,
    pub app_version: String,
    pub packaged: bool,
    /// The language a first start picks
    pub language: Language,
    /// The operating system's locale ("es-ES"), for the game language "auto"
    pub system_locale: String,
    pub game_config_dir: PathBuf,
    /// Where the local relay listens (`REFORGED_RELAY_PORT` changes it for tests and rehearsals)
    pub relay_port: u16,
    /// The file checked by hash before every launch: (relative path, SHA-256). Tests use a fake game.
    pub exe_pin: (String, String),
    pub status_poll: Duration,
    pub encryptor: Arc<dyn Encryptor>,
    pub legacy_encryptor: Option<Arc<dyn Encryptor>>,
}

/// What the tests change about the world outside (Reforged's `Platform` test seams). The app uses the
/// defaults: the real game, the real environment, the stores' lists and a QoS listener on port 80.
pub struct Seams {
    /// A stand-in for the game
    pub spawn: Option<Spawn>,
    /// The environment the game's own starts from; None: the launcher's
    pub env: Option<Vec<(OsString, OsString)>>,
    /// Where the stores keep their lists; None: this PC's
    pub store_env: Option<StoreEnv>,
    /// The relay's extra QoS listener; None for none
    pub qos_port: Option<u16>,
    /// Does a Host page server answer on port 61000? None: look
    pub host_probe: Option<Probe>,
    /// Where Dumper-7's SDKs are; None: `C:\Dumper-7`
    pub dump_root: Option<PathBuf>,
}

impl Default for Seams {
    fn default() -> Self {
        Self {
            spawn: None,
            env: None,
            store_env: None,
            qos_port: Some(DEFAULT_QOS_PORT),
            host_probe: None,
            dump_root: None,
        }
    }
}

struct State {
    s: Stored,
    reachable: Option<bool>,
    checking: bool,
    connect_problem: Option<ConnectProblem>,
    tailscale_installed: bool,
    last_checked_at: Option<String>,
    failed_polls: u32,
    status: Option<ServerStatus>,
    status_unsupported: bool,
    has_key: bool,
    key_rejected: bool,
    /// Bumped when a ServerStatus answer already on its way may no longer be shown: logout, a new key, a
    /// forgotten or different server. See `fetch_status`. News and art loaded under an older value are
    /// dropped too.
    status_gen: u64,
    busy: bool,
    game: GameInfo,
    /// What the server offers (`/content/v1/runtime/manifest`), None when it offers none
    runtime_manifest: Option<RuntimeManifest>,
    runtime: RuntimeInfo,
    runtime_playable: bool,
    task: Option<TaskProgress>,
    /// Stops the executable check (`verify` task)
    abort: Option<Abort>,
    /// Stops the runtime download (`runtime` task)
    cancel: Option<CancellationToken>,
    last_error: Option<LauncherError>,
    notice: Option<NoticeCode>,
    update_ready: bool,
    poll: Option<JoinHandle<()>>,
    news: Option<Vec<NewsItem>>,
    news_checked_at: Option<Instant>,
    news_loading: bool,
    branding: Option<Branding>,
    relay: Option<Relay>,
    shutting_down: bool,
    /// The Host page's server runs (started here or elsewhere): a runtime replaced while it ran needs
    /// a restart (`runtime_installed_restart`). Kept current by `host.rs` (RA12).
    host_server_up: bool,
    /// The game was started for Dumper-7 (`host.rs`): its exit code is not a failed launch
    dumping: bool,
    /// What a Dumper-7 or CatalogExporter session placed beside the runtime loader (`host.rs`, RD9),
    /// removed when the game exits
    loader_session: Option<LoaderSession>,
}

impl State {
    fn new(s: Stored) -> Self {
        Self {
            s,
            reachable: None,
            checking: false,
            connect_problem: None,
            tailscale_installed: false,
            last_checked_at: None,
            failed_polls: 0,
            status: None,
            status_unsupported: false,
            has_key: false,
            key_rejected: false,
            status_gen: 0,
            busy: false,
            game: no_game(),
            runtime_manifest: None,
            runtime: no_runtime(),
            runtime_playable: false,
            task: None,
            abort: None,
            cancel: None,
            last_error: None,
            notice: None,
            update_ready: false,
            poll: None,
            news: None,
            news_checked_at: None,
            news_loading: false,
            branding: None,
            relay: None,
            shutting_down: false,
            host_server_up: false,
            dumping: false,
            loader_session: None,
        }
    }

    fn s(&self) -> &Stored {
        &self.s
    }

    fn reset_server(&mut self) {
        self.reachable = None;
        self.status = None;
        self.status_unsupported = false;
        self.connect_problem = None;
        self.news = None;
        self.branding = None;
        self.key_rejected = false;
        self.failed_polls = 0;
        self.runtime_manifest = None;
        self.status_gen += 1;
    }

    fn is_busy(&self) -> bool {
        self.busy || self.task.is_some()
    }
}

/// The page's view of a game folder before it is looked at.
fn no_game() -> GameInfo {
    GameInfo {
        root: None,
        problem: Some(GameProblem::NotSet),
        version_text: None,
        version_ok: false,
        exe_verified: false,
        detected: Vec::new(),
    }
}

fn no_runtime() -> RuntimeInfo {
    RuntimeInfo {
        state: RuntimeState::Unavailable,
        version: None,
        files: Vec::new(),
        local_dir: None,
    }
}

fn err(code: ErrorCode, detail: Option<String>) -> LauncherError {
    LauncherError { code, detail }
}

fn result(outcome: Result<(), LauncherError>) -> ActionResult {
    match outcome {
        Ok(()) => ActionResult::Ok,
        Err(error) => ActionResult::Err(error),
    }
}

type Outcome = Result<(), LauncherError>;

/// A poisoned lock only means a panic elsewhere while holding it; the data is still whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `process.platform`, as the page reads `app.platform`.
fn platform_name(host: HostPlatform) -> &'static str {
    match host {
        HostPlatform::Windows => "win32",
        HostPlatform::Linux => "linux",
    }
}

/// A relative path with `/` under `root`.
fn under(root: &str, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(PathBuf::from(root), |path, part| path.join(part))
}

/// The key slot of a private server at `host:port`.
fn private_slot(host: &str, port: u16) -> KeySlot {
    KeySlot {
        host: host.to_owned(),
        port,
        public: false,
        fp: None,
    }
}

pub struct Controller {
    p: Arc<dyn Platform>,
    c: Config,
    seams: Seams,
    settings: tokio::sync::Mutex<SettingsStore>,
    keys: KeyStore,
    game: GameProcess,
    /// The Host page (`host.rs`)
    host: host::HostState,
    /// The executable's hash by size and time (`userData/file-hashes.json`)
    hashes: tokio::sync::Mutex<HashCache>,
    state: Mutex<State>,
    /// News and art load in the background; shutdown waits for them (RD22).
    background: Mutex<Vec<JoinHandle<()>>>,
    /// Where background work runs: sync commands run on the main thread, outside any runtime
    rt: tokio::runtime::Handle,
    me: Weak<Controller>,
}

impl Controller {
    pub fn new(platform: Arc<dyn Platform>, config: Config) -> ControllerHandle {
        Self::with_seams(platform, config, Seams::default())
    }

    pub fn with_seams(platform: Arc<dyn Platform>, config: Config, seams: Seams) -> ControllerHandle {
        let settings = SettingsStore::load(&config.user_data, config.language, config.host);
        let stored = Stored::of(settings.get());
        let keys = KeyStore::new(
            config.user_data.join("keys"),
            config.encryptor.clone(),
            config.legacy_encryptor.clone(),
            config
                .documents
                .as_ref()
                .map(|documents| documents.join("Dauntless Reforged Revival").join("Account Keys")),
        );
        let hashes = HashCache::load(config.user_data.join("file-hashes.json"));
        let game = GameProcess::with_spawn(seams.spawn.clone());
        let host = host::HostState::with(seams.host_probe.clone(), seams.dump_root.clone());
        let rt = tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
            let tauri::async_runtime::RuntimeHandle::Tokio(handle) = tauri::async_runtime::handle();
            handle
        });
        let controller = Arc::new_cyclic(|me: &Weak<Controller>| Controller {
            p: platform,
            c: config,
            seams,
            settings: tokio::sync::Mutex::new(settings),
            keys,
            game,
            host,
            hashes: tokio::sync::Mutex::new(hashes),
            state: Mutex::new(State::new(stored)),
            background: Mutex::new(Vec::new()),
            rt,
            me: me.clone(),
        });
        let me = Arc::downgrade(&controller);
        controller.game.on_change(move |running, code| {
            if let Some(this) = me.upgrade() {
                this.game_changed(running, code);
            }
        });
        controller.host_attach();
        controller
    }

    fn game_changed(&self, running: bool, code: Option<i32>) {
        if running {
            log::info("game started");
            self.publish();
            return;
        }
        log::info(&format!(
            "game exited (code {})",
            code.map_or_else(|| "none".to_owned(), |code| code.to_string())
        ));
        {
            let mut st = self.st();
            let dumped = std::mem::take(&mut st.dumping);
            // A game started for Dumper-7 is ended by its console window, and a console closed by the
            // player ends the game with STATUS_CONTROL_C_EXIT: neither is a failed launch
            if let Some(code) = code.filter(|code| *code != 0 && !dumped && *code as u32 != STATUS_CONTROL_C_EXIT) {
                st.last_error = Some(err(ErrorCode::LaunchFailed, Some(game_exit_detail(code))));
            }
        }
        if let Some(this) = self.me.upgrade() {
            self.rt.spawn(async move {
                this.restore_after_session().await;
                this.stop_relay().await;
                this.inspect_runtime().await;
                this.publish();
            });
        }
    }

    /// Ends the loader session the game ran with, if any (`restoreAfterDump`): the ini and the DLL go,
    /// a Dumper-7 session's runtime comes back.
    async fn restore_after_session(&self) {
        let session = self.st().loader_session.take();
        if let Some(session) = session {
            self.end_loader_session(session).await;
        }
    }

    // ------------------------------------------------------------------ basics

    fn st(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn s(&self) -> Stored {
        self.st().s().clone()
    }

    fn server(&self) -> Option<StoredServer> {
        self.st().s().server.clone()
    }

    /// True while the game runs through the local relay: closing the launcher would cut the game off.
    pub fn keep_open(&self) -> Option<KeepOpen> {
        self.st().relay.is_some().then_some(KeepOpen::Relay)
    }

    pub fn language(&self) -> Language {
        self.st().s().language
    }

    /// The game's `Win64` folder, where the runtime goes and the game starts.
    fn win64_dir_of(st: &State) -> Option<PathBuf> {
        st.game.root.as_deref().map(|root| under(root, WIN64_RELATIVE_DIR))
    }

    /// Changes the settings file and the view of it.
    async fn update_settings(&self, change: impl FnOnce(&mut Map<String, Value>)) {
        let mut store = self.settings.lock().await;
        if let Err(error) = store.update(change) {
            log::error(&format!("settings not saved: {}", log::describe_error(&error)));
        }
        self.st().s = Stored::of(store.get());
    }

    pub async fn init(&self) {
        self.refresh_key_flag();
        // Before the runtime is looked at: a Dumper-7 session a closed launcher left set it aside
        self.recover_loader_session().await;
        self.inspect_game(true).await;
        self.inspect_runtime().await;
        self.ensure_host_workspace(true).await;
        self.publish();
        let Some(this) = self.me.upgrade() else { return };
        if self.server().is_some() {
            let this = this.clone();
            self.rt.spawn(async move {
                this.connect().await;
            });
        }
        // A game found but never checked: check it now (the executable is hashed once per change)
        if self.st().game.problem == Some(GameProblem::Unverified) {
            self.rt.spawn(async move {
                this.verify_game().await;
            });
        }
    }

    fn compute_phase(&self, st: &State) -> Phase {
        if st.s().server.is_none() {
            return Phase::Join;
        }
        if st.task.is_some() {
            return Phase::Installing;
        }
        if self.game.running() {
            return Phase::Running;
        }
        if st.reachable != Some(true) {
            return Phase::Connect;
        }
        if !st.has_key || st.key_rejected {
            return Phase::Register;
        }
        if st.game.problem.is_some() {
            return Phase::Locate;
        }
        if !st.runtime_playable {
            return Phase::Runtime;
        }
        Phase::Ready
    }

    fn settings_of(s: &Stored) -> Settings {
        Settings {
            graphics: s.graphics,
            windowed: s.windowed,
            language: s.language,
            game_language: s.game_language,
            runtime_profile: s.runtime_profile,
            streamer_mode: s.streamer_mode,
            host_tools: s.host_tools,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.st();
        let phase = self.compute_phase(&st);
        let s = st.s();
        let sv = s.server.as_ref();
        let sid = sv.map(StoredServer::id);
        let has_key = st.has_key && !st.key_rejected;
        Snapshot {
            phase,
            busy: st.busy,
            saved_servers: s
                .saved_servers
                .iter()
                .map(|saved| {
                    let id = saved.id();
                    SavedServerInfo {
                        current: sid.as_deref() == Some(id.as_str()),
                        username: s.usernames.get(&id).cloned(),
                        id,
                        mode: saved.mode,
                        host: saved.host.clone(),
                        port: saved.port,
                        name: saved.name.clone(),
                        unsupported: saved.revived(),
                    }
                })
                .collect(),
            server: sv.map(|sv| ServerInfo {
                mode: sv.mode,
                host: sv.host.clone(),
                port: sv.port,
                name: sv.name.clone(),
                has_share: sv.share.is_some(),
                has_pending_invite: sv.code.is_some(),
                loopback: is_loopback_host(&sv.host),
                fingerprint: sv.fp.clone(),
            }),
            connect: SnapshotConnect {
                checking: st.checking,
                problem: st.connect_problem,
                tailscale_installed: st.tailscale_installed,
                last_checked_at: st.last_checked_at.clone(),
            },
            account: SnapshotAccount {
                username: sid.as_ref().and_then(|sid| s.usernames.get(sid).cloned()),
                has_key,
                offer_backup: sid
                    .as_ref()
                    .is_some_and(|sid| has_key && s.backup_offered.get(sid) != Some(&true)),
            },
            game: st.game.clone(),
            runtime: st.runtime.clone(),
            task: st.task.clone(),
            running: SnapshotRunning {
                game: self.game.running(),
                relay_port: st.relay.as_ref().map(Relay::port),
            },
            settings: Self::settings_of(s),
            app: SnapshotApp {
                version: self.c.app_version.clone(),
                packaged: self.c.packaged,
                update_ready: st.update_ready,
                platform: platform_name(self.c.host).into(),
            },
            status: st.status.clone(),
            status_unsupported: st.status_unsupported,
            last_error: st.last_error.clone(),
            notice: st.notice,
        }
    }

    fn publish(&self) {
        self.p.emit_snapshot(&self.snapshot());
    }

    /// Sets the error the page shows and returns it.
    fn fail(&self, code: ErrorCode, detail: Option<String>) -> LauncherError {
        let error = err(code, detail);
        self.st().last_error = Some(error.clone());
        self.publish();
        error
    }

    /// Marks the controller busy if `free` says it may start; false when it may not.
    fn begin_busy(&self, free: impl FnOnce(&State) -> bool) -> bool {
        {
            let mut st = self.st();
            if !free(&st) {
                return false;
            }
            st.busy = true;
            st.last_error = None;
        }
        self.publish();
        true
    }

    fn end_busy(&self) {
        self.st().busy = false;
        self.publish();
    }

    async fn with_busy<T>(&self, work: impl Future<Output = T>) -> T {
        {
            let mut st = self.st();
            st.busy = true;
            st.last_error = None;
        }
        self.publish();
        let out = work.await;
        self.end_busy();
        out
    }

    fn set_task(&self, task: TaskProgress) {
        self.p.emit_progress(&task);
        self.st().task = Some(task);
    }

    /// An update waits to be installed (`setUpdateReady`).
    pub fn set_update_ready(&self) {
        self.st().update_ready = true;
        self.publish();
    }

    /// `installUpdate`'s condition: an update waits, and nothing would be cut off by the restart.
    pub fn may_install_update(&self) -> bool {
        let st = self.st();
        st.update_ready && st.task.is_none() && !self.game.running() && !st.shutting_down
    }

    pub fn set_notice(&self, code: Option<NoticeCode>) {
        self.st().notice = code;
        self.publish();
    }

    pub fn dismiss_error(&self) {
        self.st().last_error = None;
        self.publish();
    }

    // ------------------------------------------------------------------ join / connect

    /// Public invites: is there a key for this host:port that belongs to another certificate? Private
    /// invites: is this a server this PC has an account on, at another address? The page asks before it
    /// offers to join, so that it can warn instead of offering a one-click join.
    pub fn check_invite(&self, text: &str) -> InviteCheck {
        match parse_invite(text) {
            Ok(invite) => {
                let server = StoredServer::from_invite(&invite, None);
                let change = self.certificate_change(&server);
                InviteCheck::Ok {
                    certificate_changed: change.is_some(),
                    previous_fingerprint: change.flatten(),
                    moved_from: self.moved_server(&server).map(|from| from.host),
                }
            }
            Err(error) => InviteCheck::Err(Self::invite_error(error)),
        }
    }

    /// A refused invite, as the page words it (`invite_err_<detail>`; `revived` for a Dauntless Revived
    /// invite, RD5).
    fn invite_error(error: InviteError) -> LauncherError {
        err(ErrorCode::InviteInvalidFormat, Some(error.as_str().to_owned()))
    }

    /// The fingerprint the stored key for this public host:port belongs to, when it is not the invite's
    /// (`Some(None)`: a key from before keys were tied to certificates).
    fn certificate_change(&self, server: &StoredServer) -> Option<Option<String>> {
        if !server.public() {
            return None;
        }
        self.keys
            .certificate_of(&server.slot())
            .filter(|bound| *bound != server.fp)
    }

    /// A private server that moved: its host changed Tailscale accounts, so the same server (same name,
    /// same port) is now at another address, where this PC has no key yet. The page offers to take the
    /// account along; its key goes to the new address only once the player confirmed.
    fn moved_server(&self, invite: &StoredServer) -> Option<StoredServer> {
        if invite.public() || self.keys.has(&private_slot(&invite.host, invite.port)) {
            return None;
        }
        let name = crate::core::text::js_trim(&invite.name);
        let s = self.s();
        // The same kind of server only: a Dauntless Revived server's account is not one of ours
        s.server
            .into_iter()
            .chain(s.saved_servers)
            .filter(|sv| {
                sv.game == invite.game
                    && !sv.public()
                    && sv.port == invite.port
                    && sv.host != invite.host
                    && crate::core::text::js_trim(&sv.name) == name
            })
            .find(|sv| self.keys.has(&private_slot(&sv.host, sv.port)))
    }

    async fn carry_account(&self, invite: &StoredServer) {
        let Some(from) = self.moved_server(invite) else {
            return;
        };
        let Some(key) = self.keys.load(&private_slot(&from.host, from.port)).ok().flatten() else {
            return;
        };
        if let Err(error) = self.keys.save(&private_slot(&invite.host, invite.port), &key) {
            log::error(&format!("could not store the key: {error:?}"));
            return;
        }
        let (old_id, new_id) = (from.id(), invite.id());
        self.update_settings(|map| {
            let names = map.get("usernames").and_then(Value::as_object);
            let old_name = names.and_then(|names| names.get(&old_id)).cloned();
            if let Some(Value::String(name)) = old_name
                && names.is_some_and(|names| !names.contains_key(&new_id))
            {
                set_username(map, &new_id, Some(&name));
            }
            // The old address is gone: the server is listed once, at its new one
            if let Some(Value::Array(saved)) = map.get_mut("savedServers") {
                saved.retain(|entry| {
                    !matches!(serde_json::from_value::<StoredServer>(entry.clone()), Ok(saved) if saved.id() == old_id)
                });
            }
        })
        .await;
        log::info(&format!(
            "the account for {} moved with its server from {}:{} to {}:{} (the player confirmed)",
            invite.name, from.host, from.port, invite.host, invite.port
        ));
    }

    /// Joins the server from an invite. When a public invite carries another certificate than the one this
    /// PC's key for that host:port belongs to, nothing is changed or sent (error cert_changed, detail = the
    /// saved fingerprint) until the user confirms the new certificate.
    pub async fn submit_invite(&self, text: &str, accept_new_certificate: bool, carry_account: bool) -> ActionResult {
        if self.st().is_busy() {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let invite: Invite = match parse_invite(text) {
            Ok(invite) => invite,
            Err(error) => {
                let error = Self::invite_error(error);
                return ActionResult::Err(self.fail(error.code, error.detail));
            }
        };
        let code = Some(invite.code.clone());
        result(
            self.join(
                StoredServer::from_invite(&invite, code),
                accept_new_certificate,
                carry_account,
            )
            .await,
        )
    }

    /// Back to a server joined before, with what its invite said then: no invite to paste again. A saved
    /// Dauntless Revived server is refused as its invite would be (RD5).
    pub async fn switch_server(&self, id: &str) -> ActionResult {
        if self.st().is_busy() {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let saved = self.s().saved_servers.into_iter().find(|saved| saved.id() == id);
        match saved {
            Some(saved) if saved.revived() => ActionResult::Err(self.fail(
                ErrorCode::InviteInvalidFormat,
                Some(InviteError::Revived.as_str().into()),
            )),
            Some(saved) => result(self.join(saved, false, false).await),
            None => ActionResult::Err(self.fail(ErrorCode::InviteInvalidFormat, None)),
        }
    }

    pub async fn remove_saved_server(&self, id: &str) -> ActionResult {
        // The current server stays in the list
        if self.server().is_some_and(|server| server.id() == id) {
            return ActionResult::err(ErrorCode::Busy);
        }
        self.update_settings(|map| {
            if let Some(Value::Array(saved)) = map.get_mut("savedServers") {
                saved.retain(|entry| {
                    !matches!(serde_json::from_value::<StoredServer>(entry.clone()), Ok(saved) if saved.id() == id)
                });
            }
        })
        .await;
        self.publish();
        ActionResult::Ok
    }

    async fn join(&self, invite: StoredServer, accept_new_certificate: bool, carry_account: bool) -> Outcome {
        if let Some(previous) = self.certificate_change(&invite) {
            if !accept_new_certificate {
                log::warn(&format!(
                    "invite for {}:{} has a new certificate; the stored key stays with the old one until the user confirms",
                    invite.host, invite.port
                ));
                return Err(err(ErrorCode::CertChanged, Some(previous.clone().unwrap_or_default())));
            }
            let fp = invite.fp.clone().unwrap_or_default();
            let slot = StoredServer {
                fp: previous.clone(),
                ..invite.clone()
            }
            .slot();
            if let Err(error) = self.keys.rebind(&slot, &fp) {
                log::error(&format!(
                    "the stored key could not be moved to the new certificate: {error:?}"
                ));
            }
            log::warn(&format!(
                "the user accepted a new certificate for {}:{} (was {}, now {fp})",
                invite.host,
                invite.port,
                previous.as_deref().unwrap_or("not recorded")
            ));
        }
        if carry_account {
            self.carry_account(&invite).await;
        }
        let same = self.server().is_some_and(|current| {
            current.mode == invite.mode
                && current.host == invite.host
                && current.port == invite.port
                && current.fp == invite.fp
        });
        // A friend who already has an account on this server does not need the code again
        let has_key_for_it = self.keys.has(&invite.slot());
        let stored = StoredServer {
            code: if has_key_for_it { None } else { invite.code.clone() },
            ..invite.clone()
        };
        self.update_settings(|map| set_server(map, Some(&stored))).await;
        if !same {
            self.st().reset_server();
        }
        let mode = if invite.public() { "public" } else { "private" };
        log::info(&format!("joined {mode} server {}:{}", invite.host, invite.port));
        self.refresh_key_flag();
        self.st().last_error = None;
        self.connect().await;
        Ok(())
    }

    pub async fn forget_server(&self) -> ActionResult {
        if self.st().task.is_some() || self.game.running() {
            return ActionResult::err(ErrorCode::Busy);
        }
        self.update_settings(|map| set_server(map, None)).await;
        {
            let mut st = self.st();
            st.reset_server();
            st.has_key = false;
        }
        self.inspect_runtime().await;
        self.publish();
        ActionResult::Ok
    }

    pub async fn connect(&self) -> ActionResult {
        let Some(sv) = self.server() else {
            return ActionResult::err(ErrorCode::ServerUnreachable);
        };
        let endpoint = sv.endpoint();
        {
            let mut st = self.st();
            if st.checking {
                return ActionResult::Ok;
            }
            st.checking = true;
        }
        self.publish();
        self.connect_now(&sv, &endpoint).await;
        self.st().checking = false;
        self.publish();
        ActionResult::Ok
    }

    async fn connect_now(&self, sv: &StoredServer, endpoint: &Endpoint) {
        let tailscale_installed = !sv.public() && self.p.find_tailscale();
        self.st().tailscale_installed = tailscale_installed;
        let (answer, stale) = self.fetch_status(endpoint).await;
        self.st().last_checked_at = Some(now_iso());
        match answer {
            StatusResult::Ok(_) | StatusResult::Unsupported => {
                let name = {
                    let mut st = self.st();
                    st.reachable = Some(true);
                    st.connect_problem = None;
                    st.failed_polls = 0;
                    st.status_unsupported = answer == StatusResult::Unsupported;
                    // Overtaken (logout, new key, other server): the server still answers, but never show a
                    // list that was asked for with a key the launcher no longer has
                    match answer {
                        StatusResult::Ok(status) => {
                            let name = (!stale).then(|| status.name.clone());
                            st.status = Some(if stale { limited_view(status) } else { status });
                            name
                        }
                        _ => {
                            st.status = None;
                            None
                        }
                    }
                };
                if let Some(name) = name {
                    self.remember_server_name(&name).await;
                }
                self.check_account().await;
                self.refresh_runtime_offer().await;
                self.in_background(Extras::All);
            }
            _ => {
                let problem = match answer {
                    StatusResult::BadAnswer => ConnectProblem::BadAnswer,
                    StatusResult::CertMismatch => ConnectProblem::CertMismatch,
                    _ if !sv.public() && !tailscale_installed && !is_loopback_host(&endpoint.host) => {
                        ConnectProblem::TailscaleMissing
                    }
                    _ => ConnectProblem::Unreachable,
                };
                {
                    let mut st = self.st();
                    st.reachable = Some(false);
                    st.connect_problem = Some(problem);
                }
                if problem == ConnectProblem::CertMismatch {
                    log::error(&format!(
                        "server {}:{} presented a certificate that does not match the invite",
                        endpoint.host, endpoint.port
                    ));
                } else {
                    let problem = serde_json::to_value(problem)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    log::warn(&format!(
                        "server {}:{} not reachable ({problem})",
                        endpoint.host, endpoint.port
                    ));
                }
            }
        }
    }

    async fn remember_server_name(&self, name: &str) {
        if self.server().is_some_and(|sv| sv.name != name) {
            self.update_settings(|map| stored::edit_server(map, "name", Value::String(name.into())))
                .await;
        }
    }

    fn refresh_key_flag(&self) {
        let has_key = self.server().is_some_and(|sv| self.keys.has(&sv.slot()));
        self.st().has_key = has_key;
    }

    /// The key sent with ServerStatus, so the server lists who is online (it shows the list to registered
    /// players only). Only this server's own key, which in public mode is bound to the invite's certificate
    /// and only ever goes over the connection pinned to it; none while the server refuses the key. Read
    /// from the key store for each request, never kept in memory.
    fn status_key(&self) -> Option<String> {
        let (sv, has_key, key_rejected) = {
            let st = self.st();
            (st.s().server.clone(), st.has_key, st.key_rejected)
        };
        if !has_key || key_rejected {
            return None;
        }
        self.keys.load(&sv?.slot()).ok().flatten()
    }

    /// ServerStatus with `status_key`. Stale when `status_gen` moved while it was on its way: the answer
    /// was asked for under the old state, so a full list in it must not be shown any more.
    async fn fetch_status(&self, endpoint: &Endpoint) -> (StatusResult, bool) {
        let generation = self.st().status_gen;
        let key = self.status_key();
        let answer = fetch_server_status(endpoint, key.as_deref(), None).await;
        (answer, generation != self.st().status_gen)
    }

    /// Confirms the stored key with GetUserInfo and refreshes the username.
    async fn check_account(&self) {
        let Some(sv) = self.server() else { return };
        let key = self.keys.load(&sv.slot()).ok().flatten();
        self.st().has_key = key.is_some();
        let Some(key) = key else { return };
        match fetch_user_info(&sv.endpoint(), &key).await {
            Ok(info) => {
                self.st().key_rejected = false;
                let id = sv.id();
                if self.s().usernames.get(&id) != Some(&info.username) {
                    self.update_settings(|map| set_username(map, &id, Some(&info.username)))
                        .await;
                }
            }
            Err((ErrorCode::AccountBanned, detail)) => {
                let mut st = self.st();
                st.key_rejected = true;
                st.last_error = Some(err(ErrorCode::AccountBanned, detail));
            }
            Err((ErrorCode::KeyRejected, _)) => {
                {
                    let mut st = self.st();
                    st.key_rejected = true;
                    st.last_error = Some(err(ErrorCode::KeyRejected, None));
                }
                log::warn("the server does not accept the stored key");
            }
            Err(_) => {}
        }
    }

    // ------------------------------------------------------------------ status polling, news, art

    /// Row L39: every `status_poll` while the page is shown (the page turns it off when hidden).
    pub fn set_status_polling(&self, on: bool) {
        let mut st = self.st();
        match (on, st.poll.is_some()) {
            (true, false) => {
                let me = self.me.clone();
                let every = self.c.status_poll;
                st.poll = Some(self.rt.spawn(async move {
                    // The first tick is at once
                    let mut interval = tokio::time::interval(every);
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        interval.tick().await;
                        let Some(this) = me.upgrade() else { return };
                        this.poll_status().await;
                    }
                }));
            }
            (false, true) => {
                if let Some(poll) = st.poll.take() {
                    poll.abort();
                }
            }
            _ => {}
        }
    }

    pub async fn poll_status(&self) {
        let Some(sv) = self.server() else { return };
        let endpoint = sv.endpoint();
        let (checking, reachable, problem) = {
            let st = self.st();
            (st.checking, st.reachable, st.connect_problem)
        };
        if checking {
            return;
        }
        if reachable != Some(true) {
            // Keep retrying quietly in the background while the guide is shown
            if problem != Some(ConnectProblem::CertMismatch) {
                self.connect().await;
            }
            return;
        }
        let (answer, stale) = self.fetch_status(&endpoint).await;
        // Overtaken: drop it; the next poll asks again under the new state
        if stale {
            return;
        }
        let running = self.game.running();
        let (extras, offer) = {
            let mut st = self.st();
            let offline = |st: &mut State| {
                if let Some(status) = st.status.as_mut() {
                    status.online = false;
                }
            };
            match answer {
                StatusResult::Ok(status) => {
                    st.status = Some(status);
                    st.status_unsupported = false;
                    st.failed_polls = 0;
                    let extras = if st.branding.is_none() {
                        Extras::All
                    } else {
                        Extras::News
                    };
                    // The host may have published a runtime meanwhile
                    let offer = !running
                        && st.task.is_none()
                        && (st.runtime.state == RuntimeState::Unavailable || st.runtime_manifest.is_none());
                    (Some(extras), offer)
                }
                StatusResult::Unsupported => {
                    st.status_unsupported = true;
                    st.failed_polls = 0;
                    (None, false)
                }
                StatusResult::CertMismatch => {
                    st.reachable = Some(false);
                    st.connect_problem = Some(ConnectProblem::CertMismatch);
                    offline(&mut st);
                    log::error("the server's certificate changed and no longer matches the invite");
                    (None, false)
                }
                _ => {
                    st.failed_polls += 1;
                    offline(&mut st);
                    // While the game runs, a busy moment on the server must not flip the launcher back to
                    // the connect guide
                    if st.failed_polls >= 2 && !running {
                        st.reachable = Some(false);
                        st.connect_problem = Some(ConnectProblem::Unreachable);
                    }
                    (None, false)
                }
            }
        };
        if let Some(extras) = extras {
            self.in_background(extras);
        }
        if offer {
            self.refresh_runtime_offer().await;
        }
        self.publish();
    }

    /// News and art load without holding up the caller; `shutdown` waits for them (RD22).
    fn in_background(&self, extras: Extras) {
        let Some(this) = self.me.upgrade() else { return };
        let task = self.rt.spawn(async move {
            match extras {
                Extras::All => this.load_extras().await,
                Extras::News => this.refresh_news().await,
            }
        });
        let mut background = lock(&self.background);
        background.retain(|task| !task.is_finished());
        background.push(task);
    }

    /// News and art come from the server's own gateway (`/content/v1/*`).
    async fn refresh_news(&self) {
        let (endpoint, generation, fallback) = {
            let mut st = self.st();
            let Some(endpoint) = st.s().server.as_ref().map(StoredServer::endpoint) else {
                return;
            };
            let fresh = st.news.is_some()
                && st
                    .news_checked_at
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(60));
            if st.news_loading || fresh {
                return;
            }
            st.news_loading = true;
            st.news_checked_at = Some(Instant::now());
            (endpoint, st.status_gen, st.news.clone().unwrap_or_default())
        };
        let items = fetch_news(&endpoint, fallback).await;
        let current = {
            let mut st = self.st();
            st.news_loading = false;
            let current = generation == st.status_gen;
            if current {
                st.news = Some(items);
            }
            current
        };
        if current {
            self.publish();
        }
    }

    async fn load_extras(&self) {
        let Some(endpoint) = self.server().map(|sv| sv.endpoint()) else {
            return;
        };
        self.refresh_news().await;
        let (missing, generation) = {
            let st = self.st();
            (st.branding.is_none(), st.status_gen)
        };
        if missing {
            let index = fetch_branding_index(&endpoint).await;
            let cache = self.c.user_data.join("art-cache");
            let mut backgrounds = Vec::new();
            if let Err(error) = tokio::fs::create_dir_all(&cache).await {
                log::warn(&format!(
                    "could not load the server's news or art: {}",
                    log::describe_error(&error)
                ));
                return;
            }
            for (path, credit) in index.backgrounds {
                let Some((kind, data)) = fetch_branding_image(&endpoint, &path).await else {
                    continue;
                };
                let id = hex(&Sha256::digest(&data));
                let file = cache.join(&id);
                if let Err(error) = tokio::fs::write(&file, &data).await {
                    log::warn(&format!(
                        "could not load the server's news or art: {}",
                        log::describe_error(&error)
                    ));
                    return;
                }
                self.p.put_art(&id, file, kind);
                backgrounds.push(Background {
                    url: format!("{}://bg/{id}", crate::platform::art::SCHEME),
                    credit,
                });
            }
            // The server changed while the pack loaded: it belongs to the old one
            let mut st = self.st();
            if generation == st.status_gen {
                st.branding = Some(Branding {
                    backgrounds,
                    accent: index.accent,
                });
            }
        }
        self.publish();
    }

    pub fn news(&self) -> Vec<NewsItem> {
        self.st().news.clone().unwrap_or_default()
    }

    pub fn branding(&self) -> Branding {
        self.st().branding.clone().unwrap_or(Branding {
            backgrounds: Vec::new(),
            accent: None,
        })
    }

    // ------------------------------------------------------------------ account

    pub async fn register(&self, username: &str) -> RegisterOutcome {
        if self.st().is_busy() {
            return RegisterOutcome::Err(err(ErrorCode::Busy, None));
        }
        let check = check_username(username);
        if check != UsernameCheck::Ok {
            return RegisterOutcome::Err(self.fail(ErrorCode::UsernameInvalid, Some(check.as_str().to_owned())));
        }
        let Some(sv) = self.server() else {
            return RegisterOutcome::Err(err(ErrorCode::ServerUnreachable, None));
        };
        if !self.begin_busy(|st| !st.is_busy()) {
            return RegisterOutcome::Err(err(ErrorCode::Busy, None));
        }
        let outcome = self.register_now(&sv, username).await;
        self.end_busy();
        outcome
    }

    async fn register_now(&self, sv: &StoredServer, username: &str) -> RegisterOutcome {
        let key = match register_account(&sv.endpoint(), username, sv.code.as_deref().unwrap_or("")).await {
            RegisterResult::Ok { key } => key,
            RegisterResult::Err { error, .. } => {
                let code = serde_json::to_value(error).unwrap_or_default();
                log::warn(&format!("registration refused: {}", code.as_str().unwrap_or("")));
                self.st().last_error = Some(err(error, None));
                return RegisterOutcome::Err(err(error, None));
            }
        };
        if let Err(error) = self.keys.save(&sv.slot(), &key) {
            log::error(&format!("could not store the key: {error:?}"));
            self.st().last_error = Some(err(ErrorCode::KeyStorageUnavailable, None));
            return RegisterOutcome::Err(err(ErrorCode::KeyStorageUnavailable, None));
        }
        let id = sv.id();
        self.update_settings(|map| {
            stored::edit_server(map, "code", Value::Null);
            set_username(map, &id, Some(username));
            set_backup_offered(map, &id, false);
        })
        .await;
        {
            let mut st = self.st();
            st.has_key = true;
            st.key_rejected = false;
            st.status_gen += 1;
        }
        log::info(&format!("registered as {username}"));
        self.check_account().await;
        // Now a registered player: ask again, with the key, so the player list shows at once
        self.poll_status().await;
        RegisterOutcome::Ok {
            username: username.to_owned(),
        }
    }

    pub async fn use_existing_key(&self, text: &str) -> ActionResult {
        if self.st().is_busy() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let Some(key) = extract_account_key(text).map(str::to_owned) else {
            return ActionResult::Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        if !self.begin_busy(|st| !st.is_busy()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        let outcome = self.adopt_key(&key).await;
        self.end_busy();
        result(outcome)
    }

    pub async fn import_key_file(&self) -> ActionResult {
        if self.st().is_busy() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let Some(file) = self.p.choose_open_file().await else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let text = match tokio::fs::metadata(&file).await {
            Ok(metadata) if metadata.len() <= 8192 => tokio::fs::read_to_string(&file).await.ok(),
            _ => None,
        };
        let Some(key) = text.as_deref().and_then(extract_account_key).map(str::to_owned) else {
            return ActionResult::Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        result(self.with_busy(self.adopt_key(&key)).await)
    }

    async fn adopt_key(&self, key: &str) -> Outcome {
        let Some(sv) = self.server() else {
            return Err(err(ErrorCode::ServerUnreachable, None));
        };
        let info = match fetch_user_info(&sv.endpoint(), key).await {
            Ok(info) => info,
            Err((code, detail)) => {
                let error = err(code, detail);
                self.st().last_error = Some(error.clone());
                return Err(error);
            }
        };
        if let Err(error) = self.keys.save(&sv.slot(), key) {
            let code = if error == KeyStoreError::Invalid {
                ErrorCode::KeyInvalid
            } else {
                ErrorCode::KeyStorageUnavailable
            };
            self.st().last_error = Some(err(code, None));
            return Err(err(code, None));
        }
        let id = sv.id();
        self.update_settings(|map| {
            set_username(map, &id, Some(&info.username));
            // They already have a copy
            set_backup_offered(map, &id, true);
            stored::edit_server(map, "code", Value::Null);
        })
        .await;
        {
            let mut st = self.st();
            st.has_key = true;
            st.key_rejected = false;
            st.status_gen += 1;
        }
        log::info(&format!("using an existing key for {}", info.username));
        self.poll_status().await;
        Ok(())
    }

    pub async fn save_key_backup(&self) -> ActionResult {
        let Some(sv) = self.server() else {
            return ActionResult::err(ErrorCode::ServerUnreachable);
        };
        let Some(key) = self.keys.load(&sv.slot()).ok().flatten() else {
            return ActionResult::Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        let id = sv.id();
        let username = self.s().usernames.get(&id).cloned();
        let name = match &username {
            Some(username) => format!("Dauntless Reforged Revival key - {username}.txt"),
            None => "Dauntless Reforged Revival key.txt".to_owned(),
        };
        let Some(file) = self.p.choose_save_file(name).await else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let text = backup_file_text(&BackupFile {
            key: &key,
            username: username.as_deref(),
            server_name: &sv.name,
            host: &sv.host,
            port: sv.port,
            fingerprint: sv.fp.as_deref(),
            date: &date,
        });
        if let Err(error) = write_private(&file, &text).await {
            log::error(&format!("backup not saved: {}", log::describe_error(&error)));
            return ActionResult::Err(self.fail(ErrorCode::Unknown, None));
        }
        log::info("key backup saved");
        self.update_settings(|map| set_backup_offered(map, &id, true)).await;
        self.set_notice(Some(NoticeCode::BackupSaved));
        ActionResult::Ok
    }

    pub async fn dismiss_backup_offer(&self) {
        let Some(sv) = self.server() else { return };
        let id = sv.id();
        self.update_settings(|map| set_backup_offered(map, &id, true)).await;
        self.publish();
    }

    pub async fn logout(&self) -> ActionResult {
        if self.st().task.is_some() || self.game.running() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let Some(sv) = self.server() else {
            return ActionResult::Ok;
        };
        self.keys.remove(&sv.slot());
        let id = sv.id();
        self.update_settings(|map| {
            set_username(map, &id, None);
            set_backup_offered(map, &id, false);
        })
        .await;
        {
            let mut st = self.st();
            st.has_key = false;
            st.key_rejected = false;
            // Without a key the server no longer lists who is online; stop showing the old list now, and
            // drop any status request still on its way with the old key (it would bring the list back)
            st.status_gen += 1;
            st.status = st.status.take().map(limited_view);
        }
        log::info("logged out; key removed");
        self.publish();
        ActionResult::Ok
    }

    // ------------------------------------------------------------------ play

    pub fn cancel_task(&self) {
        let (abort, cancel) = {
            let st = self.st();
            (st.abort.clone(), st.cancel.clone())
        };
        if let Some(abort) = abort {
            abort.abort();
        }
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }

    pub async fn play(&self) -> ActionResult {
        self.play_with(None).await
    }

    /// PLAY; with `exporter`, the game also loads CatalogExporter through the runtime loader's list
    /// (the Host page's export session, RD9).
    async fn play_with(&self, exporter: Option<PathBuf>) -> ActionResult {
        {
            let st = self.st();
            if st.shutting_down || st.is_busy() {
                return ActionResult::err(ErrorCode::Busy);
            }
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let Some(sv) = self.server() else {
            return ActionResult::Err(self.fail(ErrorCode::ServerUnreachable, None));
        };
        if !self.begin_busy(|st| !st.is_busy() && !st.shutting_down) {
            return ActionResult::err(ErrorCode::Busy);
        }
        let outcome = self.play_now(&sv, exporter).await;
        self.end_busy();
        result(outcome)
    }

    async fn play_now(&self, sv: &StoredServer, exporter: Option<PathBuf>) -> Outcome {
        // The runtime needs the Visual C++ runtime; a PC without it is warned in the log only, as in 1.x
        // (RD22): the game itself says what is missing
        let (missing_vc, _) = self.p.missing_runtimes();
        if !missing_vc.is_empty() {
            log::warn(&format!("Visual C++ runtime files missing: {}", missing_vc.join(", ")));
        }
        if !self.p.find_running_clients().await.is_empty() {
            return Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let Some(key) = self.keys.load(&sv.slot()).ok().flatten() else {
            return Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        let endpoint = sv.endpoint();
        let account = match fetch_user_info(&endpoint, &key).await {
            Ok(account) => account,
            Err((code, detail)) => {
                self.st().key_rejected = matches!(code, ErrorCode::KeyRejected | ErrorCode::AccountBanned);
                return Err(self.fail(code, detail));
            }
        };
        self.st().key_rejected = false;

        // The executable by hash on every launch, like Dauntless Revived's play.ps1
        let (root, dir) = {
            let st = self.st();
            (st.game.root.clone(), Self::win64_dir_of(&st))
        };
        let (Some(root), Some(dir)) = (root, dir) else {
            return Err(self.fail(ErrorCode::GameFolderNotFound, None));
        };
        let exe = under(&root, &self.c.exe_pin.0);
        let digest = {
            let mut hashes = self.hashes.lock().await;
            let digest = hashes.hash(&exe, Abort::default(), Arc::new(|_| {})).await.ok();
            let _ = hashes.save().await;
            digest
        };
        if digest.as_deref() != Some(self.c.exe_pin.1.as_str()) {
            self.inspect_game(false).await;
            let code = if digest.is_none() {
                ErrorCode::GameFolderNotFound
            } else {
                ErrorCode::GameWrongVersion
            };
            return Err(self.fail(code, None));
        }

        // Before anything in the folder changes. Nothing but the runtime rides along: no loader list, no
        // Host tools DLL beside the loader (RD9)
        let leftovers = self.loader_leftovers(&dir);
        if !leftovers.is_empty() {
            log::warn(&format!(
                "PLAY refused: beside the runtime loader: {}",
                leftovers.join(", ")
            ));
            return Err(self.fail(ErrorCode::LoaderLeftovers, Some(leftovers.join(", "))));
        }

        // The runtime: a local build, or the server's files checked again by hash
        if let Some(local) = self.s().local_runtime_dir
            && let Err(error) = install_local_runtime(Path::new(&local), &dir).await
        {
            log::error(&format!("local runtime not installed: {error}"));
            let code = if error.code == RuntimeErrorCode::InUse {
                ErrorCode::RuntimeInUse
            } else {
                ErrorCode::LocalRuntimeInvalid
            };
            return Err(self.fail(code, None));
        }
        self.inspect_runtime().await;
        let (playable, state) = {
            let st = self.st();
            (st.runtime_playable, st.runtime.state)
        };
        if !playable {
            let code = match state {
                RuntimeState::Untrusted => ErrorCode::RuntimeUntrusted,
                RuntimeState::Unavailable => ErrorCode::RuntimeUnavailable,
                _ => ErrorCode::RuntimeFailed,
            };
            return Err(self.fail(code, None));
        }

        if let Some(exporter) = exporter {
            let session = self.begin_loader_session(SessionKind::Export, &dir, &exporter)?;
            self.st().loader_session = Some(session);
        }

        let launched = match self.start_relay(sv).await {
            Ok(port) => self.launch_game(sv, &key, &account, &dir, port).await,
            Err(error) => Err(error),
        };
        if launched.is_err() {
            self.stop_relay().await;
            self.restore_after_session().await;
        }
        launched
    }

    async fn launch_game(
        &self,
        sv: &StoredServer,
        key: &str,
        account: &crate::net::hostapi::UserInfo,
        dir: &Path,
        port: u16,
    ) -> Outcome {
        let prepared = match self.p.prepare_game_launch().await {
            Prepared::Native => None,
            Prepared::Runtime(prepared) => Some(prepared),
            Prepared::Missing => return Err(self.fail(ErrorCode::CompatRuntimeMissing, None)),
        };
        let s = self.s();
        let config_dir = prepared.as_ref().map_or_else(
            || self.c.game_config_dir.clone(),
            |prepared| prepared.config_dir.clone(),
        );
        let config = ApplyConfig {
            host: "127.0.0.1",
            port,
            graphics: s.graphics,
            config_dir,
            safe_window: s.windowed,
            clear_phoenix_endpoints: true,
        };
        let applied = match apply_game_config(&config) {
            Ok(applied) => applied,
            Err(error) => {
                log::error(&format!(
                    "could not write the game config: {}",
                    log::describe_error(&error)
                ));
                return Err(self.fail(ErrorCode::ConfigFailed, None));
            }
        };
        if applied.phoenix_endpoints_removed > 0 {
            log::info(&format!(
                "removed {} endpoint overrides ([OnlineSubsystemPhoenix], written by Dauntless Revived's launcher) from Game.ini: they send Dauntless 1.14.7 to 127.0.0.1 (copy kept as Game.ini.before-reforged)",
                applied.phoenix_endpoints_removed
            ));
        }
        let args = build_launch_args(&LaunchSettings {
            key,
            user_id: &account.user_id,
            username: &account.username,
            windowed: s.windowed || applied.display_repaired,
            game_locale: Some(game_locale(s.game_language, &self.c.system_locale)),
        });
        let base = self.seams.env.clone().unwrap_or_else(|| std::env::vars_os().collect());
        let env = args.and_then(|args| {
            launch_environment(base, &format!("http://127.0.0.1:{port}"), s.runtime_profile).map(|env| (args, env))
        });
        let (args, env) = match env {
            Ok(launch) => launch,
            Err(reason) => {
                log::error(&format!("launch failed: {reason}"));
                return Err(self.fail(ErrorCode::LaunchFailed, None));
            }
        };
        let mode = if sv.public() { "public" } else { "private" };
        let via = prepared
            .as_ref()
            .map(|prepared| format!(" via {}", prepared.runtime_name))
            .unwrap_or_default();
        log::info(&format!(
            "starting {}{via} (relay to {}:{}, {mode})",
            describe_launch(EXE_NAME, &args),
            sv.host,
            sv.port
        ));
        let runtime: Option<&LaunchRuntime> = prepared.as_ref().map(|prepared| &prepared.runtime);
        if let Err(error) = self.game.start(dir, &args, runtime, &env) {
            log::error(&format!("launch failed: {}", log::describe_error(&error)));
            return Err(self.fail(ErrorCode::LaunchFailed, None));
        }
        Ok(())
    }

    /// Starts the local relay for the current server, in both modes (the runtime reaches the backend
    /// through it); its port.
    async fn start_relay(&self, sv: &StoredServer) -> Result<u16, LauncherError> {
        self.stop_relay().await;
        if sv.public() && sv.fp.is_none() {
            return Err(self.fail(ErrorCode::RelayFailed, None));
        }
        let port = self.c.relay_port;
        let mut options = RelayOptions::new(RelayTarget {
            host: sv.host.clone(),
            port: sv.port,
            fingerprint: if sv.public() { sv.fp.clone() } else { None },
        });
        options.port = port;
        options.qos_port = self.seams.qos_port;
        // Only the game this launcher started may use the relay, where its process can be found (RD21)
        let game = self.game.clone();
        options.authorize = game_owner_check(Arc::new(move || game.pid()));
        let me = self.me.clone();
        options.on_pin_error = Some(Arc::new(move |_| {
            if let Some(this) = me.upgrade() {
                this.st().last_error = Some(err(ErrorCode::CertMismatch, None));
                this.publish();
            }
        }));
        options.on_log = Some(Arc::new(|level, message| {
            let line = format!("relay: {message}");
            match level {
                LogLevel::Info => log::info(&line),
                LogLevel::Warn => log::warn(&line),
                LogLevel::Error => log::error(&line),
            }
        }));
        match Relay::start(options).await {
            Ok(relay) => {
                let port = relay.port();
                self.st().relay = Some(relay);
                Ok(port)
            }
            Err(error) => {
                log::error(&format!("relay could not start: {error}"));
                let code = if matches!(error, RelayError::PortBusy(_)) {
                    ErrorCode::RelayPortBusy
                } else {
                    ErrorCode::RelayFailed
                };
                Err(self.fail(code, Some(port.to_string())))
            }
        }
    }

    async fn stop_relay(&self) {
        let relay = self.st().relay.take();
        if let Some(relay) = relay {
            let (requests, upgrades, failures, qos) = (
                relay.requests(),
                relay.upgrades(),
                relay.failures(),
                relay.qos_answers(),
            );
            relay.stop().await;
            log::info(&format!(
                "relay stopped ({requests} requests, {upgrades} upgrades, {failures} failures, {qos} QoS answers)"
            ));
        }
    }

    // ------------------------------------------------------------------ settings, links, folders

    /// The settings after the patch; a refused patch (`commands::set_settings`) changes nothing.
    pub async fn set_settings(&self, patch: SettingsPatch) -> Settings {
        let host_tools_on = patch.host_tools == Some(true) && !self.s().host_tools;
        self.update_settings(|map| {
            let mut put = |field: &str, value: Option<Value>| {
                if let Some(value) = value {
                    map.insert(field.into(), value);
                }
            };
            put("graphics", patch.graphics.map(|graphics| Value::from(graphics.value())));
            put("windowed", patch.windowed.map(Value::Bool));
            put("streamerMode", patch.streamer_mode.map(Value::Bool));
            put("hostTools", patch.host_tools.map(Value::Bool));
            put("language", patch.language.map(|language| Value::from(language.code())));
            put(
                "gameLanguage",
                patch
                    .game_language
                    .and_then(|language| serde_json::to_value(language).ok()),
            );
            put(
                "runtimeProfile",
                patch
                    .runtime_profile
                    .and_then(|profile| serde_json::to_value(profile).ok()),
            );
        })
        .await;
        if host_tools_on {
            self.ensure_host_workspace(true).await;
        }
        self.publish();
        Self::settings_of(&self.s())
    }

    pub fn open_external(&self, target: ExternalTarget) -> ActionResult {
        let (sv, source) = {
            let st = self.st();
            (
                st.s().server.clone(),
                st.status.as_ref().and_then(|status| status.source_url.clone()),
            )
        };
        let url = match target {
            ExternalTarget::TailscaleShare => sv.filter(|sv| sv.mode == ServerMode::Private).and_then(|sv| sv.share),
            ExternalTarget::ServerSource => Some(source.clone().unwrap_or_else(|| PROJECT_URL.to_owned())),
            // Every other target opens its one fixed URL
            _ => fixed_link_url(target).map(str::to_owned),
        };
        match url {
            Some(url) if is_allowed_external_url(&url, source.as_deref()) => {
                self.p.open_url(&url, source.as_deref());
                ActionResult::Ok
            }
            _ => ActionResult::err(ErrorCode::Unknown),
        }
    }

    pub fn open_logs_folder(&self) {
        let dir = self.c.user_data.join("logs");
        if dir.exists() {
            self.p.open_folder(&dir);
        }
    }

    /// As the Electron launcher on window-all-closed, and DR's shutdown: no PLAY from here on, the Host
    /// page's server stops, the background downloads finish, then polling, the task and the relay stop
    /// (RD22).
    pub async fn shutdown(&self) {
        self.st().shutting_down = true;
        // The server the Host page started stops with the launcher (the page says so)
        self.host_shutdown().await;
        let background: Vec<JoinHandle<()>> = lock(&self.background).drain(..).collect();
        for task in background {
            let _ = task.await;
        }
        self.set_status_polling(false);
        self.cancel_task();
        self.stop_relay().await;
        if let Err(error) = self.hashes.lock().await.save().await {
            log::warn(&format!("file hashes not saved: {}", log::describe_error(&error)));
        }
    }
}

enum Extras {
    /// News and the art pack
    All,
    News,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A file only its owner may read (where the file system knows owners).
async fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    tokio::fs::write(path, text).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}
