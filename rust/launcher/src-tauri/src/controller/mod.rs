//! The launcher's state machine (row L38; Reforged's `src/main/controller.ts`). It owns every piece of
//! state; the page draws snapshots and sends intents through the commands.
//!
//! RA2's skeleton: every method a command forwards to exists with its final signature and answers as a
//! stub (`crate::stub`), so the commands never change again. The methods are split by the row that
//! fills them, so no two rows share a file:
//!
//! - this file: join, connect, account, status, news and art, settings, links, play and shutdown (RA13,
//!   ported from DR's `controller/mod.rs` at e1c5fec and Reforged's `controller.ts`);
//! - `game.rs`: the game folder (RA13, on RA9's `platform::game_folder`);
//! - `runtime.rs`: the server's game runtime (RA13, on RA7's `net::runtime_files`);
//! - `host.rs`: the Host page (RA12, on `platform::hostkit`, `hostserver`, `requirements`, `injector`).
//!
//! Until RA13, the snapshot is the one a first start shows (no server: paste an invite), with the
//! default settings.

pub mod game;
pub mod host;
#[allow(dead_code, reason = "RA13's controller uses the platform")]
pub mod platform;
pub mod runtime;
#[allow(dead_code, reason = "RA13's controller reads the settings through it")]
pub mod stored;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use self::platform::Platform;
use crate::bridge::types::{
    ActionResult, Branding, ExternalTarget, GameInfo, GameLanguage, GameProblem, GraphicsPreset, InviteCheck, Language,
    LauncherError, NewsItem, NoticeCode, Phase, RegisterOutcome, RuntimeInfo, RuntimeProfile, RuntimeState, Settings,
    SettingsPatch, Snapshot, SnapshotAccount, SnapshotApp, SnapshotConnect, SnapshotRunning,
};
use crate::platform::dialogs::KeepOpen;
use crate::platform::keystore::Encryptor;
use crate::platform::log;
use crate::platform::settings::Platform as HostPlatform;
use crate::stub::{NotImplemented, not_implemented};

pub type ControllerHandle = Arc<Controller>;

/// The row that fills this file's stubs.
const ROW: &str = "RA13";

/// What the app knows before the controller starts (the fixed half of `Platform` in TypeScript).
#[allow(dead_code, reason = "RA13's controller reads the rest")]
pub struct Config {
    pub user_data: PathBuf,
    /// Where the automatic account key backups go (`Documents/Dauntless Reforged Revival/Account Keys`)
    pub documents: Option<PathBuf>,
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

#[derive(Default)]
struct State {
    last_error: Option<LauncherError>,
    notice: Option<NoticeCode>,
    update_ready: bool,
    shutting_down: bool,
}

/// A poisoned lock only means a panic elsewhere while holding it; the data is still whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `process.platform`, as the page reads `app.platform`.
fn platform_name() -> &'static str {
    if cfg!(windows) { "win32" } else { "linux" }
}

pub struct Controller {
    p: Arc<dyn Platform>,
    c: Config,
    state: Mutex<State>,
}

impl Controller {
    pub fn new(platform: Arc<dyn Platform>, config: Config) -> ControllerHandle {
        Arc::new(Controller {
            p: platform,
            c: config,
            state: Mutex::new(State::default()),
        })
    }

    fn st(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// A stub's answer, logged so a developer sees which row a call is waiting for.
    fn stub(&self, what: &str, row: &'static str) -> NotImplemented {
        let stub = not_implemented(row);
        log::warn(&format!("{what}: {stub}"));
        stub
    }

    /// True while the game runs through the local relay: closing the launcher would cut the game off.
    pub fn keep_open(&self) -> Option<KeepOpen> {
        None
    }

    pub fn language(&self) -> Language {
        self.settings().language
    }

    pub async fn init(&self) {
        self.publish();
    }

    fn settings(&self) -> Settings {
        Settings {
            graphics: GraphicsPreset::DEFAULT,
            windowed: false,
            language: self.c.language,
            game_language: GameLanguage::Auto,
            runtime_profile: RuntimeProfile::Production,
            streamer_mode: false,
            host_tools: false,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.st();
        Snapshot {
            phase: Phase::Join,
            busy: false,
            server: None,
            saved_servers: Vec::new(),
            connect: SnapshotConnect {
                checking: false,
                problem: None,
                tailscale_installed: false,
                last_checked_at: None,
            },
            account: SnapshotAccount {
                username: None,
                has_key: false,
                offer_backup: false,
            },
            game: GameInfo {
                root: None,
                problem: Some(GameProblem::NotSet),
                version_text: None,
                version_ok: false,
                exe_verified: false,
                detected: Vec::new(),
            },
            runtime: RuntimeInfo {
                state: RuntimeState::Unavailable,
                version: None,
                files: Vec::new(),
                local_dir: None,
            },
            task: None,
            running: SnapshotRunning {
                game: false,
                relay_port: None,
            },
            settings: self.settings(),
            app: SnapshotApp {
                version: self.c.app_version.clone(),
                packaged: self.c.packaged,
                update_ready: st.update_ready,
                platform: platform_name().into(),
            },
            status: None,
            status_unsupported: false,
            last_error: st.last_error.clone(),
            notice: st.notice,
        }
    }

    fn publish(&self) {
        self.p.emit_snapshot(&self.snapshot());
    }

    pub fn set_update_ready(&self) {
        self.st().update_ready = true;
        self.publish();
    }

    /// `installUpdate`'s condition: an update waits, and nothing would be cut off by the restart.
    pub fn may_install_update(&self) -> bool {
        let st = self.st();
        st.update_ready && !st.shutting_down
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

    pub fn check_invite(&self, _text: &str) -> InviteCheck {
        InviteCheck::Err(self.stub("check_invite", ROW).into())
    }

    pub async fn submit_invite(
        &self,
        _text: &str,
        _accept_new_certificate: bool,
        _carry_account: bool,
    ) -> ActionResult {
        self.stub("submit_invite", ROW).into()
    }

    pub async fn switch_server(&self, _id: &str) -> ActionResult {
        self.stub("switch_server", ROW).into()
    }

    pub async fn remove_saved_server(&self, _id: &str) -> ActionResult {
        self.stub("remove_saved_server", ROW).into()
    }

    pub async fn forget_server(&self) -> ActionResult {
        self.stub("forget_server", ROW).into()
    }

    pub async fn connect(&self) -> ActionResult {
        self.stub("connect", ROW).into()
    }

    pub fn set_status_polling(&self, _on: bool) {
        self.stub("set_status_polling", ROW);
    }

    pub async fn poll_status(&self) {
        self.stub("poll_status", ROW);
    }

    pub fn news(&self) -> Vec<NewsItem> {
        self.stub("news", ROW);
        Vec::new()
    }

    pub fn branding(&self) -> Branding {
        self.stub("branding", ROW);
        Branding {
            backgrounds: Vec::new(),
            accent: None,
        }
    }

    // ------------------------------------------------------------------ account

    pub async fn register(&self, _username: &str) -> RegisterOutcome {
        RegisterOutcome::Err(self.stub("register", ROW).into())
    }

    pub async fn use_existing_key(&self, _text: &str) -> ActionResult {
        self.stub("use_existing_key", ROW).into()
    }

    pub async fn import_key_file(&self) -> ActionResult {
        self.stub("import_key_file", ROW).into()
    }

    pub async fn save_key_backup(&self) -> ActionResult {
        self.stub("save_key_backup", ROW).into()
    }

    pub async fn dismiss_backup_offer(&self) {
        self.stub("dismiss_backup_offer", ROW);
    }

    pub async fn logout(&self) -> ActionResult {
        self.stub("logout", ROW).into()
    }

    // ------------------------------------------------------------------ play

    pub fn cancel_task(&self) {
        self.stub("cancel_task", ROW);
    }

    pub async fn play(&self) -> ActionResult {
        if self.st().shutting_down {
            return ActionResult::err(crate::bridge::types::ErrorCode::Busy);
        }
        self.stub("play", ROW).into()
    }

    // ------------------------------------------------------------------ settings, links, folders

    /// The settings after the patch; a refused or unapplied patch changes nothing.
    pub async fn set_settings(&self, _patch: SettingsPatch) -> Settings {
        self.stub("set_settings", ROW);
        self.settings()
    }

    pub fn open_external(&self, _target: ExternalTarget) -> ActionResult {
        self.stub("open_external", ROW).into()
    }

    pub fn open_game_folder(&self) {
        self.stub("open_game_folder", ROW);
    }

    pub fn open_logs_folder(&self) {
        self.stub("open_logs_folder", ROW);
    }

    /// As the Electron launcher on window-all-closed: stop polling, the task and the relay, and wait for
    /// the background downloads (RD22).
    pub async fn shutdown(&self) {
        self.st().shutting_down = true;
    }
}
