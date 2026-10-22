//! The values that cross the bridge between the page and the backend: `src/shared/types.ts` of the
//! Electron launcher, field for field (row L6). The TypeScript declarations in
//! `src/shared/generated/types.ts` are generated from these (`generate.rs`), so the page and the
//! backend cannot drift apart.

use protocol::invite::ServerMode;
use protocol::status::ServerStatus;
use serde::{Deserialize, Serialize, Serializer};

/// The launcher's languages, made from the page's strings by build.rs (L14).
pub use crate::core::i18n::Language;

/// -1 = leave graphics to the in-game menu, 0..4 = Low, Medium, High, Epic, Cinematic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "i8", into = "i8")]
pub struct GraphicsPreset(i8);

impl GraphicsPreset {
    pub const GAME_MENU: Self = Self(-1);
    pub const DEFAULT: Self = Self(4);

    pub fn value(self) -> i8 {
        self.0
    }
}

impl TryFrom<i8> for GraphicsPreset {
    type Error = &'static str;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        if (-1..=4).contains(&value) {
            Ok(Self(value))
        } else {
            Err("a graphics preset is -1 to 4")
        }
    }
}

impl From<GraphicsPreset> for i8 {
    fn from(preset: GraphicsPreset) -> Self {
        preset.0
    }
}

/// Keep the game's scene-specific exposure by default; basic is an opt-in experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ExposureMode {
    Game,
    Basic,
}

/// The game's own text language, sent to it as -epiclocale; "auto" follows the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum GameLanguage {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en-US")]
    EnUs,
    #[serde(rename = "de-DE")]
    DeDe,
    #[serde(rename = "es-ES")]
    EsEs,
    #[serde(rename = "fr-FR")]
    FrFr,
    #[serde(rename = "it-IT")]
    ItIt,
    #[serde(rename = "ja-JP")]
    JaJp,
    #[serde(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "ru-RU")]
    RuRu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum HuntRegion {
    Auto,
    Main,
    Ger,
    Us,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Loading,
    /// No server yet: paste an invite
    Join,
    /// Server known, not reachable (or still checking)
    Connect,
    /// Server reachable, no account key yet
    Register,
    /// Account ready, game not installed
    Install,
    /// Download / verify / repair running (or paused)
    Installing,
    /// Game present but files are missing, damaged or the DLLs need refreshing
    Update,
    /// PLAY
    Ready,
    /// The game is running
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConnectProblem {
    /// Private mode only
    TailscaleMissing,
    /// No answer within 5 s
    Unreachable,
    /// Something answered but it is not a Dauntless Revived server
    BadAnswer,
    /// Public mode: the certificate is not the one from the invite
    CertMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AccountBanned,
    InviteInvalidFormat,
    InviteInvalid,
    UsernameInvalid,
    UsernameTaken,
    RegistrationClosed,
    BadRequest,
    ServerUnreachable,
    ServerError,
    KeyInvalid,
    KeyRejected,
    KeyStorageUnavailable,
    NoContentServer,
    ManifestMismatch,
    ManifestUnavailable,
    DiskSpace,
    FolderInvalid,
    /// "I already have the game files": no 1.4.4 game in the layouts looked at
    GameFolderNotFound,
    FolderNotEmpty,
    DownloadFailed,
    FileMissingOnServer,
    FileDifferentOnServer,
    VerifyFailed,
    DllFailed,
    GameFilesInvalid,
    AlreadyRunning,
    LaunchFailed,
    WindowsRuntimeMissing,
    CompatRuntimeMissing,
    ConfigFailed,
    Busy,
    /// The file picked to restore is not an offline save
    OfflineSaveInvalid,
    Cancelled,
    CertMismatch,
    /// A public invite whose certificate differs from the one this PC's key belongs to
    CertChanged,
    RateLimited,
    RelayPortBusy,
    RelayFailed,
    Unknown,
    OfflineUnavailable,
    OfflineInsufficientMemory,
}

/// Short messages that are not errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NoticeCode {
    KeepOpen,
    BackupSaved,
    RepairDone,
    InstallDone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LauncherError {
    pub code: ErrorCode,
    /// Extra detail for the message, e.g. a file path or a byte count. Never a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub detail: Option<String>,
}

impl LauncherError {
    pub fn new(code: ErrorCode) -> Self {
        Self { code, detail: None }
    }
}

/// Offline play's local runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum OfflinePhase {
    Preparing,
    StartingMetagame,
    StartingDeploy,
    /// First offline start only: no local account yet
    NeedsUsername,
    LaunchingGame,
    Running,
    Stopping,
    Stopped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct OfflineStatus {
    pub phase: OfflinePhase,
    /// 0..1 when meaningful
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<LauncherError>,
}

/// `{ ok: true } | { ok: false, error }`, as the page reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionResult {
    Ok,
    Err(LauncherError),
}

impl ActionResult {
    pub fn err(code: ErrorCode) -> Self {
        Self::Err(LauncherError::new(code))
    }
}

#[derive(Serialize)]
struct Failed<'a> {
    ok: bool,
    error: &'a LauncherError,
}

impl Serialize for ActionResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Done {
            ok: bool,
        }
        match self {
            Self::Ok => Done { ok: true }.serialize(serializer),
            Self::Err(error) => Failed { ok: false, error }.serialize(serializer),
        }
    }
}

/// Asked before joining from an invite. `certificate_changed`: this PC has a key for the invite's
/// host:port that belongs to another certificate (`previous_fingerprint`, None if not recorded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteCheck {
    Ok {
        certificate_changed: bool,
        previous_fingerprint: Option<String>,
    },
    Err(LauncherError),
}

impl Serialize for InviteCheck {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Checked<'a> {
            ok: bool,
            certificate_changed: bool,
            previous_fingerprint: &'a Option<String>,
        }
        match self {
            Self::Ok {
                certificate_changed,
                previous_fingerprint,
            } => Checked {
                ok: true,
                certificate_changed: *certificate_changed,
                previous_fingerprint,
            }
            .serialize(serializer),
            Self::Err(error) => Failed { ok: false, error }.serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Ok { username: String },
    Err(LauncherError),
}

impl Serialize for RegisterOutcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Registered<'a> {
            ok: bool,
            username: &'a str,
        }
        match self {
            Self::Ok { username } => Registered { ok: true, username }.serialize(serializer),
            Self::Err(error) => Failed { ok: false, error }.serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Download,
    Verify,
    Repair,
    Dlls,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub kind: TaskKind,
    pub paused: bool,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub files_total: u32,
    pub files_done: u32,
    pub bytes_per_second: f64,
    pub eta_seconds: Option<f64>,
    pub current_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub mode: ServerMode,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub has_share: bool,
    /// An invite code is stored and not used yet
    pub has_pending_invite: bool,
    pub loopback: bool,
    /// Public mode: the pinned certificate fingerprint (not secret)
    pub fingerprint: Option<String>,
}

/// A server joined before, for switching back to it without the invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SavedServerInfo {
    /// The account key slot id (not secret)
    pub id: String,
    pub mode: ServerMode,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub current: bool,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub hunt_region: Option<HuntRegion>,
    #[cfg_attr(test, ts(type = "GraphicsPreset"))]
    pub graphics: GraphicsPreset,
    pub exposure: ExposureMode,
    pub windowed: bool,
    pub language: Language,
    pub game_language: GameLanguage,
    /// The DLL's log window, normally hidden (DR_SHOW_CONSOLE=1)
    pub show_console: bool,
    /// Hide addresses, fingerprints, paths and player names on screen
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub streamer_mode: Option<bool>,
}

/// What `setSettings` may change: any of the settings, each checked; an unknown field or a wrong value
/// refuses the whole patch (`ipc-validate.ts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub hunt_region: Option<HuntRegion>,
    pub graphics: Option<GraphicsPreset>,
    pub exposure: Option<ExposureMode>,
    pub windowed: Option<bool>,
    pub language: Option<Language>,
    pub game_language: Option<GameLanguage>,
    pub show_console: Option<bool>,
    pub streamer_mode: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotConnect {
    pub checking: bool,
    pub problem: Option<ConnectProblem>,
    pub tailscale_installed: bool,
    pub last_checked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotAccount {
    pub username: Option<String>,
    pub has_key: bool,
    pub offer_backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInstall {
    pub dir: String,
    pub default_dir: String,
    /// Every file present with the right size, and the DLLs in place
    pub installed: bool,
    pub missing_files: u32,
    /// This folder's files were fully checked or downloaded by the launcher
    pub verified: bool,
    pub dlls_ok: bool,
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
    pub total_bytes: u64,
    pub vc_runtime_missing: Vec<String>,
    #[serde(default, rename = "directXMissing", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub direct_x_missing: Option<Vec<String>>,
    pub content_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotGame {
    pub running: bool,
    pub relay_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotApp {
    pub version: String,
    pub packaged: bool,
    pub update_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub busy: bool,
    pub server: Option<ServerInfo>,
    pub saved_servers: Vec<SavedServerInfo>,
    pub connect: SnapshotConnect,
    pub account: SnapshotAccount,
    pub install: SnapshotInstall,
    pub task: Option<TaskProgress>,
    pub game: SnapshotGame,
    pub settings: Settings,
    pub app: SnapshotApp,
    pub status: Option<ServerStatus>,
    /// The host answers but has no ServerStatus endpoint yet
    pub status_unsupported: bool,
    pub last_error: Option<LauncherError>,
    pub notice: Option<NoticeCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct NewsItem {
    pub date: Option<String>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Background {
    /// An app-internal dr-art URL
    pub url: String,
    pub credit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Branding {
    pub backgrounds: Vec<Background>,
    pub accent: Option<String>,
}

/// The links the page can ask the backend to open. The page names one of these; it never passes a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ExternalTarget {
    TailscaleDownload,
    TailscaleShare,
    VcRedist,
    DirectxRuntime,
    ServerSource,
    ProjectSource,
    ProjectLicense,
    ProjectContributors,
    UpstreamSource,
    UpstreamContributors,
    Eugamehost,
    EugamehostGame1,
    EugamehostGame2,
    EugamehostGame3,
    EugamehostGame5,
    #[serde(rename = "eugamehost_5800x")]
    Eugamehost5800x,
    Discord,
    Patreon,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn results_read_as_the_page_expects() {
        assert_eq!(serde_json::to_value(ActionResult::Ok).unwrap(), json!({ "ok": true }));
        assert_eq!(
            serde_json::to_value(ActionResult::err(ErrorCode::KeyInvalid)).unwrap(),
            json!({ "ok": false, "error": { "code": "key_invalid" } })
        );
        let check = InviteCheck::Ok {
            certificate_changed: true,
            previous_fingerprint: None,
        };
        assert_eq!(
            serde_json::to_value(check).unwrap(),
            json!({ "ok": true, "certificateChanged": true, "previousFingerprint": null })
        );
        let registered = RegisterOutcome::Ok {
            username: "Slayer".into(),
        };
        assert_eq!(
            serde_json::to_value(registered).unwrap(),
            json!({ "ok": true, "username": "Slayer" })
        );
        let error = LauncherError {
            code: ErrorCode::DiskSpace,
            detail: Some("12".into()),
        };
        assert_eq!(
            serde_json::to_value(RegisterOutcome::Err(error)).unwrap(),
            json!({ "ok": false, "error": { "code": "disk_space", "detail": "12" } })
        );
    }

    #[test]
    fn names_match_the_typescript_ones() {
        assert_eq!(
            serde_json::to_value(ExternalTarget::Eugamehost5800x).unwrap(),
            json!("eugamehost_5800x")
        );
        assert_eq!(
            serde_json::to_value(ExternalTarget::EugamehostGame1).unwrap(),
            json!("eugamehost_game1")
        );
        assert_eq!(
            serde_json::to_value(OfflinePhase::StartingMetagame).unwrap(),
            json!("starting-metagame")
        );
        assert_eq!(serde_json::to_value(GameLanguage::PtBr).unwrap(), json!("pt-BR"));
        assert_eq!(
            serde_json::to_value(ErrorCode::GameFolderNotFound).unwrap(),
            json!("game_folder_not_found")
        );
    }

    #[test]
    fn a_settings_patch_is_checked_whole() {
        let patch: SettingsPatch = serde_json::from_value(json!({ "huntRegion": "us", "graphics": -1 })).unwrap();
        assert_eq!(patch.hunt_region, Some(HuntRegion::Us));
        assert_eq!(patch.graphics, Some(GraphicsPreset::GAME_MENU));
        for bad in [
            json!({ "graphics": 5 }),
            json!({ "unexpected": true }),
            json!({ "language": "xx" }),
            json!({ "windowed": "yes" }),
            json!([]),
        ] {
            assert!(serde_json::from_value::<SettingsPatch>(bad.clone()).is_err(), "{bad}");
        }
    }
}
