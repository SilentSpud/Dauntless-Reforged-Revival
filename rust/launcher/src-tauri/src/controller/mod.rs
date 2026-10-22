//! The launcher's state machine (row L38): `src/main/controller.ts`. It owns every piece of state; the
//! page draws snapshots and sends intents through the commands.
//!
//! Two server modes exist (see `core::invite`):
//!  - private (invite v1): the host's PC over Tailscale, plain HTTP to host:61000.
//!  - public (invite v2): a server on a public IP. Everything goes over TLS to its gateway, pinned to the
//!    invite's certificate fingerprint. While the game runs, a local relay on 127.0.0.1:61000 carries the
//!    game's own traffic to the gateway (`net::relay`).
//!
//! The state sits behind one mutex that is only held between awaits, never across one: each locked stretch
//! is what one synchronous stretch of the TypeScript controller was on Node's event loop, so the same
//! interleavings are possible and no others. Checks that TypeScript made and acted on in one stretch
//! (`busy`, `task`) are made and acted on under one lock here too.
//!
//! Offline play is off in 0.2.0 (row L42): the commands answer `offline_unavailable` without asking the
//! controller.

#[cfg(feature = "offline")]
pub mod offline;
pub mod platform;
pub mod stored;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use protocol::invite::{Invite, ServerMode};
use protocol::status::ServerStatus;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use self::platform::{Platform, Prepared};
use self::stored::{Stored, StoredServer, set_backup_offered, set_server, set_text, set_username};
use crate::bridge::types::{
    ActionResult, Background, Branding, ConnectProblem, ErrorCode, ExposureMode, ExternalTarget, HuntRegion,
    InviteCheck, Language, LauncherError, NewsItem, NoticeCode, Phase, RegisterOutcome, SavedServerInfo, ServerInfo,
    Settings, SettingsPatch, Snapshot, SnapshotAccount, SnapshotApp, SnapshotConnect, SnapshotGame, SnapshotInstall,
    TaskKind, TaskProgress,
};
use crate::core::game_language::game_locale;
use crate::core::invite::{is_loopback_host, parse_invite};
use crate::core::manifest::{GameManifest, ManifestFile, compare_manifests, resolve_inside};
use crate::core::pins::{DISK_MARGIN_BYTES, EXE_NAME, XMPP_PORT};
use crate::core::status::limited_view;
use crate::core::username::{UsernameCheck, check_username, extract_account_key};
use crate::net::downloader::{DownloadErrorCode, DownloadJob, DownloadOptions};
use crate::net::hostapi::{
    RegisterResult, StatusResult, check_device_signals, end_playing, fetch_branding_image, fetch_branding_index,
    fetch_content_manifest, fetch_news, fetch_server_status, fetch_user_info, probe_content, register_account,
    save_hunt_region,
};
use crate::net::http::Endpoint;
use crate::net::pinned::PinnedTarget;
use crate::net::relay::{LogLevel, Relay, RelayError, RelayOptions};
use crate::net::verify::{Abort, VerifiedCache, VerifyError, VerifyOptions, remove_part_files, verify_install};
use crate::platform::client_mods::{dll_status, install_client_mods, install_pinned_dlls, win64_dir};
use crate::platform::dialogs::{FolderPurpose, KeepOpen};
use crate::platform::game_config::{ApplyConfig, apply_game_config};
use crate::platform::game_folder::{GameFolder, locate_existing_game};
use crate::platform::keystore::{BackupFile, Encryptor, KeyStore, KeyStoreError, backup_file_text};
use crate::platform::launch::{
    GameProcess, LaunchRuntime, LaunchSettings, Spawn, build_launch_args, describe_launch, game_exit_detail,
};
use crate::platform::links::{PROJECT_URL, fixed_link_url, is_allowed_external_url};
use crate::platform::log;
use crate::platform::settings::{Platform as HostPlatform, SettingsStore};
use crate::platform::system::free_bytes;

pub type ControllerHandle = Arc<Controller>;

/// What the app knows before the controller starts (the fixed half of `Platform` in TypeScript).
pub struct Config {
    pub user_data: PathBuf,
    /// Where the automatic account key backups go (`Documents/Dauntless Revived/Account Keys`)
    pub documents: Option<PathBuf>,
    pub resources_dir: PathBuf,
    pub default_install_dir: PathBuf,
    pub host: HostPlatform,
    pub app_version: String,
    pub packaged: bool,
    /// The language a first start picks
    pub language: Language,
    /// The operating system's locale ("es-ES"), for the game language "auto"
    pub system_locale: String,
    pub manifest: Option<GameManifest>,
    pub game_config_dir: PathBuf,
    /// Public mode: where the local relay listens. The game server tells every client to use
    /// 127.0.0.1:61000, so only tests and rehearsals change it.
    pub relay_port: u16,
    /// The file checked by hash before every launch: (relative path, SHA-256). Tests use a fake game.
    pub exe_pin: (String, String),
    pub status_poll: Duration,
    pub encryptor: Arc<dyn Encryptor>,
    pub legacy_encryptor: Option<Arc<dyn Encryptor>>,
    /// A stand-in for the game (tests)
    pub spawn: Option<Spawn>,
    /// Offline play's stack and saves, where this build and PC have them (L43)
    #[cfg(feature = "offline")]
    pub offline: Option<offline::OfflineSetup>,
}

#[derive(Debug, Clone, Default)]
struct InstallInfo {
    present: usize,
    missing: Vec<ManifestFile>,
    part_files: usize,
    dlls_ok: bool,
}

#[derive(Default)]
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
    content_probe: Option<bool>,
    has_key: bool,
    key_rejected: bool,
    /// Bumped when a ServerStatus answer already on its way may no longer be shown: logout, a new key, a
    /// forgotten or different server. See `fetch_status`.
    status_gen: u64,
    busy: bool,
    install: InstallInfo,
    free_space: Option<u64>,
    task: Option<TaskProgress>,
    abort: Option<Abort>,
    job: Option<Arc<DownloadJob>>,
    last_error: Option<LauncherError>,
    notice: Option<NoticeCode>,
    update_ready: bool,
    poll: Option<JoinHandle<()>>,
    news: Option<Vec<NewsItem>>,
    news_checked_at: Option<Instant>,
    news_loading: bool,
    branding: Option<Branding>,
    relay: Option<Relay>,
    /// Who to tell the server the session ended for, when the game exits
    end_playing: Option<EndPlaying>,
    shutting_down: bool,
    #[cfg(feature = "offline")]
    off: offline::OfflineState,
}

/// What a game's exit ends: an online session, or an offline one whose local stack stops with it.
enum EndPlaying {
    Online(Endpoint, String),
    #[cfg(feature = "offline")]
    Offline(Endpoint, String),
}

impl State {
    fn s(&self) -> &Stored {
        &self.s
    }

    fn reset_server(&mut self) {
        self.reachable = None;
        self.status = None;
        self.status_unsupported = false;
        self.content_probe = None;
        self.connect_problem = None;
        self.news = None;
        self.branding = None;
        self.key_rejected = false;
        self.failed_polls = 0;
        self.status_gen += 1;
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

fn hunt_region_code(region: HuntRegion) -> &'static str {
    match region {
        HuntRegion::Auto => "auto",
        HuntRegion::Main => "main",
        HuntRegion::Ger => "ger",
        HuntRegion::Us => "us",
    }
}

fn blank(kind: TaskKind, total_bytes: u64, files_total: usize) -> TaskProgress {
    TaskProgress {
        kind,
        paused: false,
        total_bytes,
        done_bytes: 0,
        files_total: files_total as u32,
        files_done: 0,
        bytes_per_second: 0.0,
        eta_seconds: None,
        current_file: None,
    }
}

/// An error from a file operation, as the install task reports it.
fn io_code(error: &std::io::Error) -> ErrorCode {
    match error.kind() {
        std::io::ErrorKind::StorageFull => ErrorCode::DiskSpace,
        std::io::ErrorKind::PermissionDenied => ErrorCode::FolderInvalid,
        _ => ErrorCode::Unknown,
    }
}

pub struct Controller {
    p: Arc<dyn Platform>,
    c: Config,
    settings: tokio::sync::Mutex<SettingsStore>,
    keys: KeyStore,
    game: GameProcess,
    verified: Arc<tokio::sync::Mutex<VerifiedCache>>,
    state: Mutex<State>,
    /// News and art load in the background; shutdown waits for them.
    background: Mutex<Vec<JoinHandle<()>>>,
    /// Where background work runs: sync commands run on the main thread, outside any runtime
    rt: tokio::runtime::Handle,
    me: Weak<Controller>,
}

impl Controller {
    pub fn new(platform: Arc<dyn Platform>, config: Config) -> ControllerHandle {
        let settings = SettingsStore::load(&config.user_data, config.language, config.host);
        let stored = Stored::of(settings.get());
        let keys = KeyStore::new(
            config.user_data.join("keys"),
            config.encryptor.clone(),
            config.legacy_encryptor.clone(),
            config
                .documents
                .as_ref()
                .map(|documents| documents.join("Dauntless Revived").join("Account Keys")),
        );
        let verified = VerifiedCache::load(config.user_data.join("verified-files.json"));
        let game = GameProcess::with_spawn(config.spawn.clone());
        let rt = tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
            let tauri::async_runtime::RuntimeHandle::Tokio(handle) = tauri::async_runtime::handle();
            handle
        });
        let controller = Arc::new_cyclic(|me: &Weak<Controller>| Controller {
            p: platform,
            c: config,
            settings: tokio::sync::Mutex::new(settings),
            keys,
            game,
            verified: Arc::new(tokio::sync::Mutex::new(verified)),
            state: Mutex::new(State {
                s: stored,
                ..State::default()
            }),
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
        let end = {
            let mut st = self.st();
            if let Some(code) = code.filter(|code| *code != 0) {
                st.last_error = Some(err(ErrorCode::LaunchFailed, Some(game_exit_detail(code))));
            }
            st.end_playing.take()
        };
        match end {
            Some(EndPlaying::Online(endpoint, key)) => {
                self.rt.spawn(async move {
                    let _ = end_playing(&endpoint, &key).await;
                });
            }
            #[cfg(feature = "offline")]
            Some(EndPlaying::Offline(endpoint, key)) => {
                if let Some(this) = self.me.upgrade() {
                    self.rt
                        .spawn(async move { this.offline_game_ended(endpoint, key).await });
                }
            }
            None => {}
        }
        if let Some(this) = self.me.upgrade() {
            self.rt.spawn(async move {
                this.stop_relay().await;
                this.publish();
            });
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

    pub fn install_dir(&self) -> String {
        Self::install_dir_of(&self.st(), &self.c)
    }

    fn install_dir_of(st: &State, c: &Config) -> String {
        st.s()
            .install_dir
            .clone()
            .unwrap_or_else(|| c.default_install_dir.to_string_lossy().into_owned())
    }

    /// True while the game runs through the local relay or on the offline services: closing the launcher
    /// would cut the game off.
    pub fn keep_open(&self) -> Option<KeepOpen> {
        #[cfg(feature = "offline")]
        if self.offline_game_active() {
            return Some(KeepOpen::Offline);
        }
        self.st().relay.is_some().then_some(KeepOpen::Relay)
    }

    pub fn language(&self) -> Language {
        self.st().s().language
    }

    /// Private mode: the content server's own port on the host. Public mode: the gateway itself, which
    /// serves /content/* from the content server behind it.
    fn content_endpoint_of(st: &State) -> Option<Endpoint> {
        let sv = st.s().server.as_ref()?;
        if sv.public() {
            return (st.content_probe == Some(true)).then(|| sv.endpoint());
        }
        let port = st.status.as_ref()?.content_port?;
        Some(Endpoint {
            host: sv.host.clone(),
            port,
            pin: None,
        })
    }

    fn content_endpoint(&self) -> Option<Endpoint> {
        Self::content_endpoint_of(&self.st())
    }

    /// Changes the settings file and the view of it.
    async fn update_settings(&self, change: impl FnOnce(&mut Map<String, Value>)) {
        let mut store = self.settings.lock().await;
        if let Err(error) = store.update(change).await {
            log::error(&format!("settings not saved: {}", log::describe_error(&error)));
        }
        self.st().s = Stored::of(store.get());
    }

    pub async fn init(&self) {
        self.refresh_key_flag();
        self.inspect_install().await;
        self.publish();
        if self.server().is_some()
            && let Some(this) = self.me.upgrade()
        {
            self.rt.spawn(async move {
                this.connect().await;
            });
        }
    }

    fn compute_phase(&self, st: &State) -> Phase {
        let s = st.s();
        if s.server.is_none() {
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
        if self.c.manifest.is_none() || st.install.present == 0 && st.install.part_files == 0 {
            return Phase::Install;
        }
        if !st.install.missing.is_empty()
            || !st.install.dlls_ok
            || s.verified_dir.as_deref() != Some(Self::install_dir_of(st, &self.c).as_str())
        {
            return Phase::Update;
        }
        Phase::Ready
    }

    fn settings_of(s: &Stored) -> Settings {
        Settings {
            hunt_region: s.hunt_region,
            graphics: s.graphics,
            exposure: s.exposure,
            windowed: s.windowed,
            language: s.language,
            game_language: s.game_language,
            show_console: s.show_console,
            streamer_mode: Some(s.streamer_mode),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.st();
        let phase = self.compute_phase(&st);
        let s = st.s();
        let sv = s.server.as_ref();
        let sid = sv.map(StoredServer::id);
        let install_dir = Self::install_dir_of(&st, &self.c);
        let missing_bytes: u64 = st.install.missing.iter().map(|file| file.size).sum();
        let (vc_runtime_missing, direct_x_missing) = self.p.missing_runtimes();
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
            install: SnapshotInstall {
                installed: matches!(phase, Phase::Ready | Phase::Running),
                missing_files: st.install.missing.len() as u32,
                verified: s.verified_dir.as_deref() == Some(install_dir.as_str()),
                dlls_ok: st.install.dlls_ok,
                free_bytes: st.free_space,
                required_bytes: missing_bytes + DISK_MARGIN_BYTES,
                total_bytes: self.c.manifest.as_ref().map_or(0, |manifest| manifest.total_bytes),
                vc_runtime_missing,
                direct_x_missing: Some(direct_x_missing),
                content_available: Self::content_endpoint_of(&st).is_some(),
                default_dir: self.c.default_install_dir.to_string_lossy().into_owned(),
                dir: install_dir,
            },
            task: st.task.clone(),
            game: SnapshotGame {
                running: self.game.running(),
                relay_port: st.relay.as_ref().map(Relay::port),
            },
            settings: Self::settings_of(s),
            app: SnapshotApp {
                version: self.c.app_version.clone(),
                packaged: self.c.packaged,
                update_ready: st.update_ready,
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

    /// An update waits to be installed (`setUpdateReady`).
    pub fn set_update_ready(&self) {
        self.st().update_ready = true;
        self.publish();
    }

    /// `installUpdate`'s condition: an update waits, and nothing would be cut off by the restart.
    pub fn may_install_update(&self) -> bool {
        let st = self.st();
        st.update_ready && st.task.is_none() && !self.game.running()
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

    /// Public invites: is there a key for this host:port that belongs to another certificate? The page
    /// asks before it offers to join, so that it can warn instead of offering a one-click join.
    pub fn check_invite(&self, text: &str) -> InviteCheck {
        match parse_invite(text) {
            Ok(invite) => {
                let change = self.certificate_change(&StoredServer::from_invite(&invite, None));
                InviteCheck::Ok {
                    certificate_changed: change.is_some(),
                    previous_fingerprint: change.flatten(),
                }
            }
            Err(error) => InviteCheck::Err(err(ErrorCode::InviteInvalidFormat, Some(error.as_str().to_owned()))),
        }
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

    fn is_busy(st: &State) -> bool {
        st.busy || st.task.is_some()
    }

    /// Joins the server from an invite. When a public invite carries another certificate than the one this
    /// PC's key for that host:port belongs to, nothing is changed or sent (error cert_changed, detail = the
    /// saved fingerprint) until the user confirms the new certificate.
    pub async fn submit_invite(&self, text: &str, accept_new_certificate: bool) -> ActionResult {
        if Self::is_busy(&self.st()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let invite: Invite = match parse_invite(text) {
            Ok(invite) => invite,
            Err(error) => {
                return ActionResult::Err(self.fail(ErrorCode::InviteInvalidFormat, Some(error.as_str().to_owned())));
            }
        };
        let code = Some(invite.code.clone());
        result(
            self.join(StoredServer::from_invite(&invite, code), accept_new_certificate)
                .await,
        )
    }

    /// Back to a server joined before, with what its invite said then: no invite to paste again.
    pub async fn switch_server(&self, id: &str) -> ActionResult {
        if Self::is_busy(&self.st()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let saved = self.s().saved_servers.into_iter().find(|saved| saved.id() == id);
        match saved {
            Some(saved) => result(self.join(saved, false).await),
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

    async fn join(&self, invite: StoredServer, accept_new_certificate: bool) -> Outcome {
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
                if sv.public() {
                    let probe = probe_content(endpoint).await;
                    self.st().content_probe = Some(probe);
                }
                self.check_account().await;
                self.inspect_install().await;
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

    /// ServerStatus with `status_key`. Stale when `status_gen` moved while it was on its way (the user logged
    /// out, a new key was stored, the server was forgotten or changed): the answer was asked for under the
    /// old state, so a full list in it must not be shown any more.
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
        let extras = {
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
                    Some(if st.branding.is_none() {
                        Extras::All
                    } else {
                        Extras::News
                    })
                }
                StatusResult::Unsupported => {
                    st.status_unsupported = true;
                    st.failed_polls = 0;
                    None
                }
                StatusResult::CertMismatch => {
                    st.reachable = Some(false);
                    st.connect_problem = Some(ConnectProblem::CertMismatch);
                    offline(&mut st);
                    log::error("the server's certificate changed and no longer matches the invite");
                    None
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
                    None
                }
            }
        };
        if let Some(extras) = extras {
            self.in_background(extras);
        }
        self.publish();
    }

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

    async fn refresh_news(&self) {
        let (endpoint, generation, fallback) = {
            let mut st = self.st();
            let Some(endpoint) = Self::content_endpoint_of(&st) else {
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
        let Some(endpoint) = self.content_endpoint() else {
            return;
        };
        self.refresh_news().await;
        if self.st().branding.is_none() {
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
                    url: format!("dr-art://bg/{id}"),
                    credit,
                });
            }
            self.st().branding = Some(Branding {
                backgrounds,
                accent: index.accent,
            });
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
        if Self::is_busy(&self.st()) {
            return RegisterOutcome::Err(err(ErrorCode::Busy, None));
        }
        let check = check_username(username);
        if check != UsernameCheck::Ok {
            return RegisterOutcome::Err(self.fail(ErrorCode::UsernameInvalid, Some(check.as_str().to_owned())));
        }
        let Some(sv) = self.server() else {
            return RegisterOutcome::Err(err(ErrorCode::ServerUnreachable, None));
        };
        if !self.begin_busy(|st| !Self::is_busy(st)) {
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
        self.inspect_install().await;
        // Now a registered player: ask again, with the key, so the player list shows at once
        self.poll_status().await;
        RegisterOutcome::Ok {
            username: username.to_owned(),
        }
    }

    pub async fn use_existing_key(&self, text: &str) -> ActionResult {
        if Self::is_busy(&self.st()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        let Some(key) = extract_account_key(text).map(str::to_owned) else {
            return ActionResult::Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        if !self.begin_busy(|st| !Self::is_busy(st)) {
            return ActionResult::err(ErrorCode::Busy);
        }
        let outcome = self.adopt_key(&key).await;
        self.end_busy();
        result(outcome)
    }

    pub async fn import_key_file(&self) -> ActionResult {
        if Self::is_busy(&self.st()) {
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
        self.inspect_install().await;
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
            Some(username) => format!("Dauntless Revived key - {username}.txt"),
            None => "Dauntless Revived key.txt".to_owned(),
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

    // ------------------------------------------------------------------ install folder

    pub async fn inspect_install(&self) {
        let dir = PathBuf::from(self.install_dir());
        let files = self.c.manifest.as_ref().map(|manifest| manifest.files.clone());
        let (free, install) =
            tokio::task::spawn_blocking(move || (free_bytes(&dir), files.map(|files| inspect(&dir, &files))))
                .await
                .unwrap_or_default();
        let mut st = self.st();
        st.free_space = free;
        st.install = install.unwrap_or_default();
    }

    fn may_change_folder(&self) -> bool {
        let st = self.st();
        st.task.is_none() && !st.busy && !self.game.running()
    }

    pub async fn choose_install_dir(&self) -> ActionResult {
        if !self.may_change_folder() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let start = PathBuf::from(self.install_dir());
        let Some(picked) = self
            .p
            .choose_folder(self.language(), FolderPurpose::Install, start)
            .await
        else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let picked = picked.to_string_lossy().into_owned();
        // A folder that already holds the game (BaseGame144, its Dauntless folder, or Archon or Win64 inside
        // it) is used as that game: no second copy is downloaded next to it
        let host = self.c.host;
        let existing = {
            let picked = picked.clone();
            tokio::task::spawn_blocking(move || locate_existing_game(&picked, host))
                .await
                .unwrap_or(GameFolder::InvalidPath)
        };
        if !self.may_change_folder() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let found = matches!(existing, GameFolder::Found(_));
        let mut dir = match existing {
            GameFolder::Found(root) => root,
            // A game found there whose root is too long is refused, not given a second copy in a
            // subfolder (which would be longer still)
            GameFolder::TooLong => return ActionResult::Err(self.fail(ErrorCode::FolderInvalid, None)),
            _ => picked.clone(),
        };
        let absolute = match host {
            HostPlatform::Windows => {
                let bytes = dir.as_bytes();
                bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && (bytes[2] == b'\\' || bytes[2] == b'/')
            }
            HostPlatform::Linux => dir.starts_with('/'),
        };
        if !absolute || crate::core::text::js_length(&dir) > 150 {
            return ActionResult::Err(self.fail(ErrorCode::FolderInvalid, None));
        }
        // Otherwise an empty folder is used as is, and anything else gets a subfolder
        if !found {
            let mut entries = Vec::new();
            if let Ok(mut read) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = read.next_entry().await {
                    entries.push(entry.file_name().to_string_lossy().to_lowercase());
                }
            }
            if !entries.is_empty() && !entries.iter().any(|entry| entry == "archon") {
                let separator = if host == HostPlatform::Windows { '\\' } else { '/' };
                dir = format!("{}{separator}DauntlessRevived", dir.trim_end_matches(['\\', '/']));
            }
        }
        let default = self.c.default_install_dir.to_string_lossy().into_owned();
        self.update_settings(|map| set_text(map, "installDir", (dir != default).then_some(dir.as_str())))
            .await;
        self.inspect_install().await;
        self.publish();
        ActionResult::Ok
    }

    pub async fn use_existing_game_folder(&self) -> ActionResult {
        if !self.may_change_folder() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let start = PathBuf::from(self.install_dir());
        let Some(picked) = self
            .p
            .choose_folder(self.language(), FolderPurpose::Existing, start)
            .await
        else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        self.use_existing_game_path(&picked.to_string_lossy()).await
    }

    /// "I already have the game files", from a pasted path or a picked folder (Vvoidddd, #8). The game is
    /// used where it is: Repair checks every file against the manifest, fetches only what is missing or
    /// different, and installs the pinned DLLs.
    pub async fn use_existing_game_path(&self, input: &str) -> ActionResult {
        if !self.may_change_folder() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let host = self.c.host;
        let found = {
            let input = input.to_owned();
            tokio::task::spawn_blocking(move || locate_existing_game(&input, host))
                .await
                .unwrap_or(GameFolder::InvalidPath)
        };
        // Unlike the folder dialog, a pasted path leaves the window usable while the folder is looked at
        if !self.may_change_folder() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let root = match found {
            GameFolder::Found(root) => root,
            GameFolder::NotFound => return ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None)),
            _ => return ActionResult::Err(self.fail(ErrorCode::FolderInvalid, None)),
        };
        self.update_settings(|map| {
            set_text(map, "installDir", Some(&root));
            set_text(map, "verifiedDir", None);
        })
        .await;
        self.inspect_install().await;
        self.repair().await
    }

    pub fn open_game_folder(&self) {
        let dir = PathBuf::from(self.install_dir());
        if dir.exists() {
            self.p.open_folder(&dir);
        }
    }

    // ------------------------------------------------------------------ install / repair

    pub async fn start_install(&self) -> ActionResult {
        self.run_task(false).await
    }

    pub async fn repair(&self) -> ActionResult {
        self.run_task(true).await
    }

    fn download_job(&self) -> Option<Arc<DownloadJob>> {
        let st = self.st();
        st.task
            .as_ref()
            .is_some_and(|task| task.kind == TaskKind::Download)
            .then(|| st.job.clone())
            .flatten()
    }

    pub fn pause_task(&self) {
        if let Some(job) = self.download_job() {
            job.pause();
        }
    }

    pub fn resume_task(&self) {
        if let Some(job) = self.download_job() {
            job.resume();
        }
    }

    pub fn cancel_task(&self) {
        let (job, abort) = {
            let st = self.st();
            (st.job.clone(), st.abort.clone())
        };
        if let Some(job) = job {
            job.cancel();
        }
        if let Some(abort) = abort {
            abort.abort();
        }
    }

    fn set_task(&self, task: TaskProgress) {
        self.p.emit_progress(&task);
        self.st().task = Some(task);
    }

    /// UE4SS and the client mods (`platform::client_mods`). Never fatal: the game runs without them.
    async fn install_mods(&self, dir: &Path) {
        let resources = self.c.resources_dir.clone();
        let dir = dir.to_path_buf();
        match tokio::task::spawn_blocking(move || install_client_mods(&resources, &dir)).await {
            Ok(Ok(Some(done))) => log::info(&format!(
                "client mods: {} installed, {} up to date, {} kept, {} removed, {} switched off",
                done.installed, done.up_to_date, done.kept, done.removed, done.disabled
            )),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => log::warn(&format!("client mods not installed: {error}")),
            Err(error) => log::warn(&format!("client mods not installed: {error}")),
        }
    }

    async fn run_task(&self, repair: bool) -> ActionResult {
        if Self::is_busy(&self.st()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let Some(manifest) = self.c.manifest.clone() else {
            return ActionResult::Err(self.fail(ErrorCode::ManifestUnavailable, None));
        };
        let Some(sv) = self.server() else {
            return ActionResult::Err(self.fail(ErrorCode::ServerUnreachable, None));
        };
        let abort = Abort::default();
        let full_hash = {
            let mut st = self.st();
            if Self::is_busy(&st) {
                return ActionResult::err(ErrorCode::Busy);
            }
            st.last_error = None;
            st.notice = None;
            st.abort = Some(abort.clone());
            let dir = Self::install_dir_of(&st, &self.c);
            // Files this launcher already verified in this folder are trusted by size; anything else
            // (Repair, a folder picked by hand) is hashed
            let full_hash = repair || st.s().verified_dir.as_deref() != Some(dir.as_str());
            let total = if full_hash {
                manifest.total_bytes
            } else {
                manifest.files.len() as u64
            };
            st.task = Some(blank(TaskKind::Verify, total, manifest.files.len()));
            full_hash
        };
        let dir = PathBuf::from(self.install_dir());
        let mode = if repair { "repair" } else { "install" };
        let outcome = self.run_task_now(repair, full_hash, &manifest, &sv, &dir, abort).await;
        if let Err(error) = self.verified.lock().await.save().await {
            log::warn(&format!(
                "verified-files cache not saved: {}",
                log::describe_error(&error)
            ));
        }
        {
            let mut st = self.st();
            st.job = None;
            st.abort = None;
            st.task = None;
        }
        self.inspect_install().await;
        self.publish();
        match outcome {
            Ok(()) => {
                log::info(&format!("{mode} finished in {}", dir.display()));
                ActionResult::Ok
            }
            Err(error) => ActionResult::Err(error),
        }
    }

    async fn run_task_now(
        &self,
        repair: bool,
        full_hash: bool,
        manifest: &GameManifest,
        sv: &StoredServer,
        dir: &Path,
        abort: Abort,
    ) -> Outcome {
        let mode = if repair { "repair" } else { "install" };
        let io_failed = |error: std::io::Error| {
            log::error(&format!("{mode} failed: {}", log::describe_error(&error)));
            self.fail(io_code(&error), None)
        };
        tokio::fs::create_dir_all(dir).await.map_err(io_failed)?;
        let dir_text = dir.to_string_lossy().into_owned();
        if let Some(task) = self.st().task.clone() {
            self.p.emit_progress(&task);
        }
        self.publish();

        // 1. Which files are needed?
        let trusted = if repair {
            None
        } else {
            let cache = Arc::new(self.verified.lock().await.clone());
            let dir_text = dir_text.clone();
            Some(Arc::new(move |file: &ManifestFile, size: u64, mtime: f64| {
                cache.is_trusted(&dir_text, file, size, mtime)
            }) as crate::net::verify::Trusted)
        };
        let me = self.me.clone();
        let report = verify_install(
            dir,
            &manifest.files,
            VerifyOptions {
                full_hash,
                trusted,
                abort: abort.clone(),
                on_progress: Some(Arc::new(move |progress| {
                    if let Some(this) = me.upgrade() {
                        this.set_task(TaskProgress {
                            done_bytes: progress.done_bytes,
                            files_done: progress.files_done as u32,
                            current_file: progress.current_file,
                            ..blank(TaskKind::Verify, progress.total_bytes, progress.files_total)
                        });
                    }
                })),
                concurrency: None,
            },
        )
        .await;
        let report = match report {
            Ok(report) => report,
            Err(VerifyError::Aborted) => {
                log::info(&format!("{mode} cancelled"));
                return Err(err(ErrorCode::Cancelled, None));
            }
            Err(VerifyError::Io(error)) => return Err(io_failed(error)),
        };
        let needed: Vec<ManifestFile> = report.bad.iter().map(|(file, _)| file.clone()).collect();
        {
            let mut cache = self.verified.lock().await;
            for file in &needed {
                cache.forget(&dir_text, file);
            }
        }
        if !needed.is_empty() {
            log::info(&format!("{} file(s) to fetch ({mode})", needed.len()));
        }

        // 2. Download what is missing or damaged
        if !needed.is_empty() {
            self.download(manifest, sv, dir, needed).await?;
        }

        // 3. The pinned DLLs
        self.set_task(blank(TaskKind::Dlls, 2, 2));
        self.publish();
        let resources = self.c.resources_dir.clone();
        let target = dir.to_path_buf();
        match tokio::task::spawn_blocking(move || install_pinned_dlls(&resources, &target)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::error(&format!("DLL install failed: {}: {}", error.file, error.message));
                return Err(self.fail(ErrorCode::DllFailed, Some(error.file)));
            }
            Err(error) => {
                log::error(&format!("{mode} failed: {error}"));
                return Err(self.fail(ErrorCode::Unknown, None));
            }
        }
        self.install_mods(dir).await;
        remove_part_files(dir, &manifest.files).await;
        self.update_settings(|map| set_text(map, "verifiedDir", Some(&dir_text)))
            .await;
        self.st().notice = Some(if repair {
            NoticeCode::RepairDone
        } else {
            NoticeCode::InstallDone
        });
        Ok(())
    }

    async fn download(
        &self,
        manifest: &GameManifest,
        sv: &StoredServer,
        dir: &Path,
        needed: Vec<ManifestFile>,
    ) -> Outcome {
        if sv.public() && self.st().content_probe != Some(true) {
            let probe = probe_content(&sv.endpoint()).await;
            self.st().content_probe = Some(probe);
        }
        let Some(content) = self.content_endpoint() else {
            return Err(self.fail(ErrorCode::NoContentServer, None));
        };
        let Some(key) = self.keys.load(&sv.slot()).ok().flatten() else {
            return Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        let Some(server_manifest) = fetch_content_manifest(&content).await else {
            return Err(self.fail(ErrorCode::ManifestUnavailable, None));
        };
        if let Err(difference) = compare_manifests(manifest, &server_manifest) {
            log::error(&format!("content server manifest refused: {difference}"));
            return Err(self.fail(ErrorCode::ManifestMismatch, Some(difference)));
        }

        let free = free_bytes(dir);
        let mut on_disk = 0;
        for file in &needed {
            if let Ok(full) = resolve_inside(dir, &file.path) {
                let mut part = full.into_os_string();
                part.push(".part");
                if let Ok(metadata) = tokio::fs::metadata(&part).await {
                    on_disk += metadata.len().min(file.size);
                }
            }
        }
        let total: u64 = needed.iter().map(|file| file.size).sum();
        let required = total - on_disk + DISK_MARGIN_BYTES;
        if free.is_some_and(|free| free < required) {
            return Err(self.fail(ErrorCode::DiskSpace, Some(required.to_string())));
        }

        let download_base_url = match server_manifest.get("downloadBaseUrl") {
            None => None,
            Some(Value::String(url)) => Some(url.clone()),
            Some(_) => {
                log::error("download failed: Invalid content download URL");
                return Err(self.fail(ErrorCode::Unknown, None));
            }
        };
        let files_total = needed.len();
        let mut options = DownloadOptions::new(content, key, dir.to_path_buf(), needed);
        options.download_base_url = download_base_url;
        let me = self.me.clone();
        options.on_progress = Some(Arc::new(move |progress| {
            if let Some(this) = me.upgrade() {
                this.set_task(TaskProgress {
                    kind: TaskKind::Download,
                    paused: progress.paused,
                    total_bytes: progress.total_bytes,
                    done_bytes: progress.done_bytes,
                    files_total: progress.files_total as u32,
                    files_done: progress.files_done as u32,
                    bytes_per_second: progress.bytes_per_second,
                    eta_seconds: progress.eta_seconds,
                    current_file: progress.current_file,
                });
            }
        }));
        options.on_retry = Some(Arc::new(|file, attempt, reason| {
            log::warn(&format!("retry {attempt} for {}: {reason}", file.path));
        }));
        let cache = self.verified.clone();
        let verified_dir = dir.to_path_buf();
        let rt = self.rt.clone();
        options.on_file_verified = Some(Arc::new(move |file| {
            let (cache, dir, file) = (cache.clone(), verified_dir.clone(), file.clone());
            rt.spawn(async move {
                let mut cache = cache.lock().await;
                cache.record(&dir, &file).await;
                let _ = cache.save().await;
            });
        }));
        let job = match DownloadJob::new(options) {
            Ok(job) => Arc::new(job),
            Err(error) => {
                log::error(&format!("download failed: {}", error.message));
                return Err(self.fail(ErrorCode::Unknown, None));
            }
        };
        self.st().job = Some(job.clone());
        self.set_task(blank(TaskKind::Download, total, files_total));
        self.publish();
        let outcome = job.run().await;
        self.st().job = None;
        match outcome {
            Ok(()) => {
                log::info(&format!("downloaded and verified {files_total} file(s)"));
                Ok(())
            }
            Err(error) if error.code == DownloadErrorCode::Cancelled => {
                log::info("download cancelled");
                Err(err(ErrorCode::Cancelled, None))
            }
            Err(error) => {
                log::error(&format!(
                    "download stopped: {:?} {} {}",
                    error.code,
                    error.file.as_deref().unwrap_or(""),
                    error.message
                ));
                let code = match error.code {
                    DownloadErrorCode::KeyRejected => ErrorCode::KeyRejected,
                    DownloadErrorCode::FileMissingOnServer => ErrorCode::FileMissingOnServer,
                    DownloadErrorCode::FileDifferentOnServer => ErrorCode::FileDifferentOnServer,
                    DownloadErrorCode::DiskSpace => ErrorCode::DiskSpace,
                    DownloadErrorCode::DiskError => ErrorCode::FolderInvalid,
                    DownloadErrorCode::CertMismatch => ErrorCode::CertMismatch,
                    DownloadErrorCode::DownloadFailed | DownloadErrorCode::Cancelled => ErrorCode::DownloadFailed,
                };
                {
                    let mut st = self.st();
                    if error.code == DownloadErrorCode::KeyRejected {
                        st.key_rejected = true;
                    }
                    if error.code == DownloadErrorCode::CertMismatch {
                        st.reachable = Some(false);
                        st.connect_problem = Some(ConnectProblem::CertMismatch);
                    }
                }
                Err(self.fail(code, error.file))
            }
        }
    }

    // ------------------------------------------------------------------ play

    pub async fn play(&self) -> ActionResult {
        if self.st().shutting_down || Self::is_busy(&self.st()) {
            return ActionResult::err(ErrorCode::Busy);
        }
        #[cfg(feature = "offline")]
        if self.st().off.blocks_online() {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let Some(sv) = self.server() else {
            return ActionResult::Err(self.fail(ErrorCode::ServerUnreachable, None));
        };
        if !self.begin_busy(|st| !Self::is_busy(st) && !st.shutting_down) {
            return ActionResult::err(ErrorCode::Busy);
        }
        let outcome = self.play_now(&sv).await;
        self.end_busy();
        result(outcome)
    }

    async fn play_now(&self, sv: &StoredServer) -> Outcome {
        self.check_game_launch_environment().await?;
        let Some(key) = self.keys.load(&sv.slot()).ok().flatten() else {
            return Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        let endpoint = sv.endpoint();
        if let Err((code, detail)) = fetch_user_info(&endpoint, &key).await {
            self.st().key_rejected = matches!(code, ErrorCode::KeyRejected | ErrorCode::AccountBanned);
            return Err(self.fail(code, detail));
        }
        let signals = self.p.collect_device_signals().await;
        if let Err((code, detail)) = check_device_signals(&endpoint, &key, &signals).await {
            return Err(self.fail(code, detail));
        }
        self.st().key_rejected = false;
        let region = match self.s().hunt_region {
            None | Some(HuntRegion::Auto) => crate::net::autoregion::choose_closest_region(&endpoint, &key, None).await,
            Some(region) => Some(hunt_region_code(region)),
        };
        if let Some(region) = region {
            match save_hunt_region(&endpoint, &key, region).await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(self.fail(
                        ErrorCode::ServerError,
                        Some(
                            "The selected hunt region is unavailable. Choose Main in Settings or try again later."
                                .into(),
                        ),
                    ));
                }
                Err(_) => return Err(self.fail(ErrorCode::ServerUnreachable, None)),
            }
        }
        self.prepare_game_install().await?;

        // Where the game connects. Public mode: the local relay, started before the game
        let (mut game_host, mut game_port, mut xmpp_port) = (sv.host.clone(), sv.port, XMPP_PORT);
        if sv.public() {
            let port = self.start_relay(sv).await?;
            game_host = "127.0.0.1".into();
            game_port = port;
            xmpp_port = port;
        }
        let suffix = if sv.public() {
            format!(" (relay to {}:{})", sv.host, sv.port)
        } else {
            String::new()
        };
        let launched = self
            .launch_game(
                &game_host,
                game_port,
                xmpp_port,
                &key,
                EndPlaying::Online(endpoint, key.clone()),
                &suffix,
            )
            .await;
        if launched.is_err() {
            self.stop_relay().await;
        }
        launched
    }

    async fn check_game_launch_environment(&self) -> Outcome {
        let (vc, dx) = self.p.missing_runtimes();
        let missing: Vec<String> = vc.into_iter().chain(dx).collect();
        if !missing.is_empty() {
            return Err(self.fail(ErrorCode::WindowsRuntimeMissing, Some(missing.join(", "))));
        }
        if !self.p.find_running_clients().await.is_empty() {
            return Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        Ok(())
    }

    async fn prepare_game_install(&self) -> Outcome {
        let dir = PathBuf::from(self.install_dir());
        self.inspect_install().await;
        let (missing, dlls_ok) = {
            let st = self.st();
            (st.install.missing.len(), st.install.dlls_ok)
        };
        if missing > 0 {
            return Err(self.fail(ErrorCode::GameFilesInvalid, Some(missing.to_string())));
        }
        // Same checks as play.ps1 on every launch: the exe and both DLLs by hash
        let (relative, sha256) = self.c.exe_pin.clone();
        let exe = resolve_inside(&dir, &relative).ok();
        let hash = match exe {
            Some(exe) => tokio::task::spawn_blocking(move || crate::platform::client_mods::hash_file(&exe).ok())
                .await
                .ok()
                .flatten(),
            None => None,
        };
        if hash.as_deref() != Some(sha256.as_str()) {
            return Err(self.fail(ErrorCode::GameFilesInvalid, Some(relative)));
        }
        if !dlls_ok {
            let resources = self.c.resources_dir.clone();
            let target = dir.clone();
            let installed = tokio::task::spawn_blocking(move || install_pinned_dlls(&resources, &target)).await;
            if !matches!(installed, Ok(Ok(()))) {
                let reason = match installed {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                    Ok(Ok(())) => String::new(),
                };
                log::error(&format!("DLL repair before launch failed: {reason}"));
                return Err(self.fail(ErrorCode::DllFailed, None));
            }
        }
        self.install_mods(&dir).await;
        Ok(())
    }

    async fn launch_game(
        &self,
        game_host: &str,
        game_port: u16,
        xmpp_port: u16,
        key: &str,
        end: EndPlaying,
        suffix: &str,
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
            host: game_host,
            xmpp_port,
            graphics: s.graphics,
            exposure: s.exposure,
            config_dir,
            safe_window: s.windowed,
        };
        let display_repaired = match apply_game_config(&config) {
            Ok((_, repaired)) => repaired,
            Err(error) => {
                log::error(&format!(
                    "could not write the game config: {}",
                    log::describe_error(&error)
                ));
                return Err(self.fail(ErrorCode::ConfigFailed, None));
            }
        };
        if s.exposure != ExposureMode::Game {
            log::info("game config: auto exposure basic");
        }
        let args = build_launch_args(&LaunchSettings {
            host: game_host,
            port: game_port,
            key,
            windowed: s.windowed || display_repaired,
            game_locale: Some(game_locale(s.game_language, &self.c.system_locale)),
        });
        let args = match args {
            Ok(args) => args,
            Err(reason) => {
                log::error(&format!("launch failed: {reason}"));
                return Err(self.fail(ErrorCode::LaunchFailed, None));
            }
        };
        let via = prepared
            .as_ref()
            .map(|prepared| format!(" via {}", prepared.runtime_name))
            .unwrap_or_default();
        log::info(&format!("starting {}{via}{suffix}", describe_launch(EXE_NAME, &args)));
        self.st().end_playing = Some(end);
        let runtime: Option<&LaunchRuntime> = prepared.as_ref().map(|prepared| &prepared.runtime);
        let env: &[(&str, &str)] = if s.show_console {
            &[("DR_SHOW_CONSOLE", "1")]
        } else {
            &[]
        };
        let win64 = win64_dir(Path::new(&self.install_dir()));
        if let Err(error) = self.game.start(&win64, &args, runtime, env) {
            self.st().end_playing = None;
            log::error(&format!("launch failed: {}", log::describe_error(&error)));
            return Err(self.fail(ErrorCode::LaunchFailed, None));
        }
        Ok(())
    }

    /// Starts the local relay for a public server; its port.
    async fn start_relay(&self, sv: &StoredServer) -> Result<u16, LauncherError> {
        self.stop_relay().await;
        let Some(fingerprint) = sv.fp.clone().filter(|_| sv.public()) else {
            return Err(err(ErrorCode::RelayFailed, None));
        };
        let port = self.c.relay_port;
        let mut options = RelayOptions::new(PinnedTarget {
            host: sv.host.clone(),
            port: sv.port,
            fingerprint,
        });
        options.port = port;
        // Windows launches the game directly. Wine/Proton's wrapper PID is not its socket owner
        if self.c.host == HostPlatform::Windows {
            let game = self.game.clone();
            options.authorize = Some(crate::net::relay_owner::owner_check(
                crate::net::relay_owner::read_netstat(),
                true,
                Arc::new(move || game.pid()),
            ));
        }
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
            let (requests, upgrades, failures) = (relay.requests(), relay.upgrades(), relay.failures());
            relay.stop().await;
            log::info(&format!(
                "relay stopped ({requests} requests, {upgrades} upgrades, {failures} failures)"
            ));
        }
    }

    // ------------------------------------------------------------------ settings / links

    pub async fn set_settings(&self, patch: SettingsPatch) -> Settings {
        self.update_settings(|map| {
            let mut put = |field: &str, value: Option<Value>| {
                if let Some(value) = value {
                    map.insert(field.into(), value);
                }
            };
            put("graphics", patch.graphics.map(|graphics| Value::from(graphics.value())));
            put(
                "exposure",
                patch.exposure.and_then(|exposure| serde_json::to_value(exposure).ok()),
            );
            put("windowed", patch.windowed.map(Value::Bool));
            put("streamerMode", patch.streamer_mode.map(Value::Bool));
            put("showConsole", patch.show_console.map(Value::Bool));
            put(
                "huntRegion",
                patch.hunt_region.map(|region| Value::from(hunt_region_code(region))),
            );
            put("language", patch.language.map(|language| Value::from(language.code())));
            put(
                "gameLanguage",
                patch
                    .game_language
                    .and_then(|language| serde_json::to_value(language).ok()),
            );
        })
        .await;
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

    pub async fn shutdown(&self) {
        self.st().shutting_down = true;
        let background: Vec<JoinHandle<()>> = lock(&self.background).drain(..).collect();
        for task in background {
            let _ = task.await;
        }
        self.set_status_polling(false);
        self.cancel_task();
        #[cfg(feature = "offline")]
        self.shutdown_offline().await;
        self.stop_relay().await;
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

/// Which manifest files are in place by size (`inspectInstall`).
fn inspect(dir: &Path, files: &[ManifestFile]) -> InstallInfo {
    let mut info = InstallInfo::default();
    for file in files {
        let full = resolve_inside(dir, &file.path).ok();
        let present = full
            .as_ref()
            .and_then(|full| std::fs::metadata(full).ok())
            .is_some_and(|metadata| metadata.is_file() && metadata.len() == file.size);
        if present {
            info.present += 1;
            continue;
        }
        if let Some(full) = full {
            let mut part = full.into_os_string();
            part.push(".part");
            if Path::new(&part).exists() {
                info.part_files += 1;
            }
        }
        info.missing.push(file.clone());
    }
    info.dlls_ok = info.present > 0 && dll_status(dir).iter().all(|(_, ok)| *ok);
    info
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
