// Generated from the launcher backend (rust/launcher/src-tauri/src/bridge); do not edit.
// Regenerate with: RR_UPDATE_GENERATED=1 cargo test -p launcher generated

export type ServerMode = "private" | "public";
export type PlayerWhere = "menu" | "city" | "hunt" | "dojo" | "tutorial" | "unknown";
export type InstanceKind = "city" | "hunt" | "dojo" | "tutorial";
export type RegistrationMode = "OPEN" | "INVITECODE" | "NONE";
export type StatusPlayer = { name: string, where: PlayerWhere, instance: string | null, };
export type StatusInstance = { id: string, kind: InstanceKind, title: string, map: string, behemoth: string | null, players: number, maxPlayers: number, 
/**
 * ISO string, or null when the server sent something unreadable
 */
startedAt: string | null, };
export type GameDataKind = "real" | "synthetic" | "missing" | "unknown";
export type StatusGame = { 
/**
 * At most 64 characters
 */
build: string, 
/**
 * 0 to 100,000,000, or null when the server sent none or something unreadable
 */
changelist: number | null, };
export type ServerStatus = { name: string, online: boolean, version: string, commit: string, 
/**
 * Only https URLs survive parsing
 */
sourceUrl: string | null, registration: RegistrationMode | null, playersOnline: number, players: Array<StatusPlayer>, instances: Array<StatusInstance>, uptimeSeconds: number, 
/**
 * Null when the server names no build
 */
game: StatusGame | null, gameData: GameDataKind, 
/**
 * True only when the server says so
 */
chat: boolean, 
/**
 * The port of a separate content server, which only Dauntless Revived's servers in private mode
 * named. Kept in the contract; Reforged's launcher does not read it (RD5).
 */
contentPort: number | null, 
/**
 * The server hid who is online (playersOnline 0, no players, no instances): its answer to a
 * launcher without an account key there, or with one it did not accept.
 */
limited: boolean, };
export type Language = "en" | "pt-BR";
export type GameLanguage = "auto" | "en-US" | "de-DE" | "es-ES" | "fr-FR" | "it-IT" | "ja-JP" | "pt-BR" | "ru-RU";
export type RuntimeProfile = "production" | "development";
export type Phase = "loading" | "join" | "connect" | "register" | "locate" | "runtime" | "installing" | "ready" | "running";
export type ConnectProblem = "tailscale_missing" | "unreachable" | "bad_answer" | "cert_mismatch";
export type GameProblem = "not_set" | "not_found" | "unverified" | "wrong_version";
export type RuntimeState = "ok" | "missing" | "outdated" | "untrusted" | "unavailable" | "local";
export type ErrorCode = "account_banned" | "invite_invalid_format" | "invite_invalid" | "username_invalid" | "username_taken" | "registration_closed" | "bad_request" | "server_unreachable" | "server_error" | "key_invalid" | "key_rejected" | "key_storage_unavailable" | "folder_invalid" | "game_folder_not_found" | "game_wrong_version" | "runtime_unavailable" | "runtime_untrusted" | "runtime_failed" | "runtime_different_on_server" | "runtime_in_use" | "local_runtime_invalid" | "download_failed" | "disk_space" | "already_running" | "launch_failed" | "compat_runtime_missing" | "config_failed" | "busy" | "cancelled" | "cert_mismatch" | "cert_changed" | "rate_limited" | "relay_port_busy" | "relay_failed" | "host_failed" | "unknown";
export type NoticeCode = "keep_open" | "backup_saved" | "runtime_installed" | "runtime_installed_restart" | "game_verified" | "sdk_imported" | "host_done";
export type LauncherError = { code: ErrorCode, 
/**
 * Extra detail for the message, e.g. a file path or a byte count. Never a key.
 */
detail?: string, };
export type TaskKind = "verify" | "runtime";
export type TaskProgress = { kind: TaskKind, totalBytes: number, doneBytes: number, filesTotal: number, filesDone: number, bytesPerSecond: number, etaSeconds: number | null, currentFile: string | null, };
export type ServerInfo = { mode: ServerMode, host: string, port: number, name: string, hasShare: boolean, 
/**
 * An invite code is stored and not used yet
 */
hasPendingInvite: boolean, loopback: boolean, 
/**
 * Public mode: the pinned certificate fingerprint (not secret)
 */
fingerprint: string | null, };
export type SavedServerInfo = { 
/**
 * The account key slot id (not secret)
 */
id: string, mode: ServerMode, host: string, port: number, name: string, current: boolean, username: string | null, };
export type GameSource = "epic" | "legendary" | "heroic" | "chosen";
export type DetectedGame = { root: string, source: GameSource, 
/**
 * What the store says (Legendary, Heroic), or Version.txt
 */
version: string | null, };
export type GameInfo = { 
/**
 * The "Dauntless" folder, the one that holds Archon\
 */
root: string | null, problem: GameProblem | null, 
/**
 * Version.txt, e.g. dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472
 */
versionText: string | null, 
/**
 * Version.txt names 1.14.7, changelist 647472
 */
versionOk: boolean, 
/**
 * The executable's SHA-256 is the 1.14.7 one (checked once per file change)
 */
exeVerified: boolean, 
/**
 * Installs found on this PC (Epic, Legendary, Heroic)
 */
detected: Array<DetectedGame>, };
export type RuntimeFileInfo = { name: string, size: number, sha256: string, };
export type RuntimeInfo = { state: RuntimeState, 
/**
 * The server's runtime version
 */
version: string | null, 
/**
 * What the server offers (shown when the player is asked to trust it)
 */
files: Array<RuntimeFileInfo>, localDir: string | null, };
export type Settings = { graphics: GraphicsPreset, windowed: boolean, language: Language, gameLanguage: GameLanguage, runtimeProfile: RuntimeProfile, 
/**
 * Hide addresses, fingerprints, paths and player names on screen
 */
streamerMode: boolean, 
/**
 * Show the Host page in the rail
 */
hostTools: boolean, };
export type SnapshotConnect = { checking: boolean, problem: ConnectProblem | null, tailscaleInstalled: boolean, lastCheckedAt: string | null, };
export type SnapshotAccount = { username: string | null, hasKey: boolean, offerBackup: boolean, };
export type SnapshotRunning = { game: boolean, relayPort: number | null, };
export type SnapshotApp = { version: string, packaged: boolean, updateReady: boolean, 
/**
 * `process.platform`: "win32" or "linux"
 */
platform: string, };
export type Snapshot = { phase: Phase, busy: boolean, server: ServerInfo | null, savedServers: Array<SavedServerInfo>, connect: SnapshotConnect, account: SnapshotAccount, game: GameInfo, runtime: RuntimeInfo, task: TaskProgress | null, running: SnapshotRunning, settings: Settings, app: SnapshotApp, status: ServerStatus | null, 
/**
 * The host answers but has no ServerStatus endpoint
 */
statusUnsupported: boolean, lastError: LauncherError | null, notice: NoticeCode | null, };
export type NewsItem = { date: string | null, title: string, body: string, };
export type Background = { 
/**
 * An app-internal `rr-art://` URL
 */
url: string, credit: string | null, };
export type Branding = { backgrounds: Array<Background>, accent: string | null, };
export type GameDataState = "real" | "synthetic" | "missing";
export type HostUpstream = { 
/**
 * .workspace\Mystic-Paradox holds the pinned source
 */
present: boolean, 
/**
 * And the Reforged patches are applied
 */
patched: boolean, };
export type HostSdk = { 
/**
 * ParadoxRuntime\SDK.hpp
 */
present: boolean, 
/**
 * ParadoxRuntime\SDK\*_functions.cpp
 */
functionFiles: number, };
export type HostDump = { dir: string, name: string, modified: string, };
export type HostBuilds = { 
/**
 * ParadoxRuntime\x64\Release\MysticParadox.dll
 */
runtime: boolean, 
/**
 * tools\RuntimeLoader\target\release\winmm.dll
 */
loader: boolean, 
/**
 * tools\CatalogExporter\x64\Release\CatalogExporter.dll
 */
exporter: boolean, 
/**
 * Both runtime files in the content server's runtime folder
 */
published: boolean, };
export type HostToolchain = { 
/**
 * e.g. "Visual Studio 2022 (17.14)"
 */
visualStudio: string | null, msbuild: boolean, cargo: boolean, };
export type HostGameData = { backend: GameDataState, director: GameDataState, };
export type HostExports = { 
/**
 * Items_Analysis in the workspace
 */
present: boolean, 
/**
 * And beside the game (CatalogExporter's output)
 */
inGame: boolean, };
export type HostServerMode = "private" | "public";
export type HostServerConfig = { configured: boolean, name: string | null, mode: HostServerMode | null, host: string | null, 
/**
 * The test XP and reward multipliers (private servers only), null when off
 */
multipliers: HostMultipliers | null, };
export type HostKit = { 
/**
 * A Dauntless Reforged Revival checkout
 */
workspace: string | null, workspaceOk: boolean, upstream: HostUpstream, sdk: HostSdk, dumps: Array<HostDump>, 
/**
 * The Dumper-7 DLL the host picked
 */
dumperDll: string | null, builds: HostBuilds, toolchain: HostToolchain, gameData: HostGameData, exports: HostExports, serverConfig: HostServerConfig, requirements: Array<Requirement>, job: HostJob | null, 
/**
 * What the last "Update" found, since the launcher started
 */
update: HostUpdateView | null, 
/**
 * Where the project folder is downloaded to: beside the game's folder
 */
projectTarget: string | null, };
export type HostMultipliers = { xp: number, rewards: number, };
export type HostUpdateState = "up_to_date" | "updated";
export type HostUpdateView = { state: HostUpdateState, 
/**
 * The short commit this PC is at now
 */
head: string, 
/**
 * Files the update changed
 */
files: number, 
/**
 * The game runtime changed: build MysticParadox.dll again and publish it
 */
runtimeChanged: boolean, };
export type RequirementId = "dauntless" | "git" | "node" | "mongodb" | "tailscale" | "visual_studio" | "rust" | "dumper7" | "vc_redist";
export type RequirementState = "ok" | "missing" | "problem";
export type Requirement = { id: RequirementId, state: RequirementState, 
/**
 * The version or place found; never a secret
 */
detail: string | null, 
/**
 * What is wrong when state is "problem": node "too_old"; mongodb "service_running"; visual_studio
 * "no_cpp"
 */
problem: string | null, };
export type HostServerState = "stopped" | "starting" | "running" | "stopping" | "external" | "failed";
export type HostInvite = { link: string, for: string | null, };
export type HostServerView = { state: HostServerState, 
/**
 * The error line when it failed
 */
message: string | null, 
/**
 * The newest output of the server and of the host kit commands started here
 */
lines: Array<string>, invite: HostInvite | null, };
export type HostJobKind = "inject" | "build_runtime" | "build_loader" | "build_exporter" | "import_sdk" | "copy_exports" | "synthetic_data" | "import_gamedata" | "publish_runtime" | "setup" | "generate_gamedata" | "export_gamedata" | "create_invite" | "update" | "multipliers" | "upstream" | "download_project";
export type HostJobState = "running" | "done" | "failed";
export type HostJob = { kind: HostJobKind, state: HostJobState, 
/**
 * The last line of its log, never a secret
 */
message: string | null, logFile: string | null, };
export type ExternalTarget = "tailscale_download" | "tailscale_share" | "vc_redist" | "server_source" | "project_source" | "project_license" | "project_issues" | "project_docs_players" | "project_docs_hosting" | "project_docs_sdk" | "installing_guide" | "legendary" | "dumper7" | "visual_studio" | "rust" | "nodejs" | "mongodb" | "git" | "visual_studio_installer" | "rust_installer" | "dumper7_zip" | "revived_source" | "mystic_source" | "undaunted_source";
export type HostAction = "inject_dumper" | "import_sdk" | "build_runtime" | "build_loader" | "build_exporter" | "inject_exporter" | "copy_exports" | "synthetic_data" | "publish_runtime" | "generate_gamedata" | "export_gamedata";
export type HostFolder = "workspace" | "runtime_source" | "dumps" | "game_data" | "runtime_publish" | "host_logs" | "gamedata_packs";
export type HostSetupRequest = { name: string, mode: HostServerMode, 
/**
 * Public mode: the server's public IPv4 or DNS name
 */
publicHost: string | null, };
export type GraphicsPreset = -1 | 0 | 1 | 2 | 3 | 4;
export type ActionResult = { ok: true } | { ok: false; error: LauncherError };
export type InviteCheck = { ok: true; certificateChanged: boolean; previousFingerprint: string | null; movedFrom: string | null } | { ok: false; error: LauncherError };
export type RegisterOutcome = { ok: true; username: string } | { ok: false; error: LauncherError };
export type CommandName = "get_snapshot" | "check_invite" | "submit_invite" | "retry_connect" | "forget_server" | "switch_server" | "remove_saved_server" | "register" | "use_existing_key" | "import_key_file" | "save_key_backup" | "dismiss_backup_offer" | "choose_game_folder" | "use_game_path" | "use_detected_game" | "verify_game" | "trust_runtime" | "install_runtime" | "choose_local_runtime" | "clear_local_runtime" | "cancel_task" | "play" | "set_settings" | "set_status_polling" | "refresh_status" | "get_news" | "get_branding" | "open_external" | "open_game_folder" | "open_logs_folder" | "logout" | "install_update" | "host_get" | "host_choose_workspace" | "host_choose_dumper" | "host_import_gamedata" | "host_import_sdk_folder" | "host_download_project" | "host_action" | "host_open" | "host_server_get" | "host_server_start" | "host_server_stop" | "host_setup" | "host_create_invite" | "host_update" | "host_set_multipliers" | "copy_text" | "take_invite_link" | "dismiss_notice" | "dismiss_error" | "subscribe_progress";
