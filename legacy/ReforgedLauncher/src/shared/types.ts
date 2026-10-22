// Types shared by the main process, the preload bridge and the renderer.
// The main process owns all state; the renderer only draws a Snapshot and sends intents.

import type { ServerStatus } from "./status";
import type { GameEdition, ServerMode } from "./invite";

export type { GameEdition, ServerMode };

// The launcher's own text: English and Brazilian Portuguese.
export type Language = "en" | "pt-BR";
export const LANGUAGES: readonly Language[] = ["en", "pt-BR"];

// -1 = leave graphics to the in-game menu, 0..4 = Low, Medium, High, Epic, Cinematic.
export type GraphicsPreset = -1 | 0 | 1 | 2 | 3 | 4;
export const GRAPHICS_PRESETS: readonly GraphicsPreset[] = [-1, 0, 1, 2, 3, 4];
export const DEFAULT_GRAPHICS: GraphicsPreset = -1;

// The game's own text language, sent to it as -epiclocale; "auto" follows the operating system.
export type GameLanguage = "auto" | "en-US" | "de-DE" | "es-ES" | "fr-FR" | "it-IT" | "ja-JP" | "pt-BR" | "ru-RU";
export const GAME_LANGUAGES: readonly GameLanguage[] = ["auto", "en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU"];

// The game runtime's log profile (MYSTICPARADOX_RUNTIME_PROFILE). "development" also honours the
// runtime's diagnostic .flag files next to the game executable.
export type RuntimeProfile = "production" | "development";
export const RUNTIME_PROFILES: readonly RuntimeProfile[] = ["production", "development"];

export type Phase =
  | "loading"
  | "join" // no server yet: paste an invite
  | "connect" // server known, not reachable (or still checking)
  | "register" // server reachable, no account key yet
  | "locate" // account ready, but no Dauntless 1.14.7 folder chosen, found or accepted (1.4.4: not installed or checked)
  | "runtime" // game found; the server's game runtime is not installed, out of date or not trusted yet
  | "installing" // a check or the runtime download is running
  | "ready" // PLAY
  | "running"; // the game is running

export type ConnectProblem =
  | "tailscale_missing" // private mode only
  | "unreachable" // no answer within 5 s
  | "bad_answer" // something answered but it is not a Reforged server
  | "cert_mismatch" // public mode: the certificate is not the one from the invite
  | null;

// Why the chosen game folder cannot be used yet.
export type GameProblem =
  | "not_set" // nothing chosen and nothing found on this PC
  | "not_found" // the chosen folder no longer holds the game
  | "unverified" // found, but its executable has not been checked yet (or changed since)
  | "wrong_version" // the executable is not the Dauntless 1.14.7 build (CL 647472)
  | null;

// The server's game runtime (the Mystic Paradox DLL and its loader, built by the host).
export type RuntimeState =
  | "ok" // installed next to the game and matching the server's list
  | "missing" // not installed yet
  | "outdated" // installed, but the server now offers another build
  | "untrusted" // offered, but not accepted on this PC yet (the player is asked first)
  | "unavailable" // the server offers no runtime: its host has not built one yet
  | "local"; // a local runtime folder is used instead (Settings > Advanced)

export type ErrorCode =
  | "account_banned"
  | "invite_invalid_format"
  | "invite_invalid"
  | "username_invalid"
  | "username_taken"
  | "registration_closed"
  | "bad_request"
  | "server_unreachable"
  | "server_error"
  | "key_invalid"
  | "key_rejected"
  | "key_storage_unavailable"
  | "folder_invalid"
  | "game_folder_not_found" // no Dauntless game in the layouts looked at
  | "game_wrong_version" // the executable is not the 1.14.7 build
  | "runtime_unavailable" // the server offers no runtime yet
  | "runtime_untrusted" // the player has not accepted this server's runtime
  | "runtime_failed" // downloading or installing the runtime failed
  | "runtime_different_on_server" // a file did not match the server's own list
  | "runtime_in_use" // a runtime file is held by a program that would not let it be replaced
  | "local_runtime_invalid"
  | "download_failed"
  | "disk_space"
  | "already_running"
  | "launch_failed"
  | "config_failed"
  | "busy"
  | "cancelled"
  | "cert_mismatch"
  | "cert_changed" // a public invite whose certificate differs from the one this PC's key belongs to
  | "rate_limited"
  | "relay_port_busy"
  | "relay_failed"
  | "host_failed" // a Host tools action failed (detail says which)
  // Dauntless 1.4.4 (Dauntless Revived servers):
  | "manifest_mismatch" // the server's list of game files is not the pinned 1.4.4 one
  | "no_content_server" // the server offers no game download
  | "file_missing_on_server"
  | "file_different_on_server"
  | "dll_failed" // the pinned Undaunted DLLs could not be downloaded or installed
  | "game_files_invalid" // a 1.4.4 file is missing or not the pinned one: Check and repair
  | "windows_runtime_missing" // the Visual C++ or DirectX runtime the game needs (detail: the files)
  | "folder_other_game" // the folder chosen for 1.4.4 holds another Dauntless: never written into
  | "folder_not_empty" // the folder chosen for 1.4.4 holds other files
  | "unknown";

// Short messages that are not errors.
export type NoticeCode = "keep_open" | "backup_saved" | "runtime_installed" | "runtime_installed_restart" | "game_verified" | "sdk_imported" | "host_done" | "install_done" | "repair_done";

export interface LauncherError {
  code: ErrorCode;
  // Extra detail for the message, e.g. a file path or a byte count. Never a key.
  detail?: string;
}

export type ActionResult = { ok: true } | { ok: false; error: LauncherError };

// Asked before joining from an invite. certificateChanged: this PC has a key for the invite's
// host:port that belongs to another certificate (previousFingerprint, null if not recorded).
export type InviteCheck =
  // movedFrom: a private server this PC has an account on, now at the invite's new address (its old address).
  | { ok: true; certificateChanged: boolean; previousFingerprint: string | null; movedFrom: string | null }
  | { ok: false; error: LauncherError };

// download, dlls: installing Dauntless 1.4.4 for a Dauntless Revived server.
export type TaskKind = "verify" | "runtime" | "download" | "dlls";

export interface TaskProgress {
  kind: TaskKind;
  totalBytes: number;
  doneBytes: number;
  filesTotal: number;
  filesDone: number;
  bytesPerSecond: number;
  etaSeconds: number | null;
  currentFile: string | null;
}

export interface ServerInfo {
  game: GameEdition;
  mode: ServerMode;
  host: string;
  port: number;
  name: string;
  hasShare: boolean;
  hasPendingInvite: boolean; // an invite code is stored and not used yet
  loopback: boolean;
  fingerprint: string | null; // public mode: the pinned certificate fingerprint (not secret)
}

// A server joined before, for switching back to it without the invite.
export interface SavedServerInfo {
  id: string; // the account key slot id (not secret)
  game: GameEdition;
  mode: ServerMode;
  host: string;
  port: number;
  name: string;
  current: boolean;
  username: string | null;
}

export type GameSource = "epic" | "legendary" | "heroic" | "chosen" | "revived";

export interface DetectedGame {
  root: string;
  source: GameSource;
  version: string | null; // what the store says (Legendary/Heroic), or Version.txt
}

export interface GameInfo {
  root: string | null; // the "Dauntless" folder, the one that holds Archon\
  problem: GameProblem;
  versionText: string | null; // Version.txt, e.g. dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472
  versionOk: boolean; // Version.txt names 1.14.7, changelist 647472
  exeVerified: boolean; // the executable's SHA-256 is the 1.14.7 one (checked once per file change)
  detected: DetectedGame[]; // installs found on this PC (Epic, Legendary, Heroic)
}

export interface RuntimeFileInfo {
  name: string;
  size: number;
  sha256: string;
}

// Dauntless 1.4.4, for a Dauntless Revived server (src/main/revived/): the folder it is in (or goes
// into), and whether every file is the pinned one.
export interface RevivedInfo {
  root: string; // the chosen folder, or where a download goes when none is chosen
  chosen: boolean; // the player chose it (or it was found), rather than the default
  problem: GameProblem; // not_set: nothing there yet; unverified: there, but not every file checked
  missingFiles: number; // known after a check: files to download
  totalBytes: number; // the whole game
  dllsOk: boolean; // the pinned Undaunted DLLs are next to the game
  detected: DetectedGame[]; // 1.4.4 installs found on this PC (Dauntless Revived's launcher)
}

export interface RuntimeInfo {
  state: RuntimeState;
  version: string | null; // the server's runtime version
  files: RuntimeFileInfo[]; // what the server offers (shown when the player is asked to trust it)
  localDir: string | null;
}

export interface Settings {
  graphics: GraphicsPreset;
  windowed: boolean;
  language: Language;
  gameLanguage: GameLanguage;
  runtimeProfile: RuntimeProfile;
  streamerMode: boolean; // hide addresses, fingerprints, paths and player names on screen
  hostTools: boolean; // show the Host page in the rail
}

export interface Snapshot {
  phase: Phase;
  busy: boolean;
  server: ServerInfo | null;
  savedServers: SavedServerInfo[];
  connect: {
    checking: boolean;
    problem: ConnectProblem;
    tailscaleInstalled: boolean;
    lastCheckedAt: string | null;
  };
  account: {
    username: string | null;
    hasKey: boolean;
    offerBackup: boolean;
  };
  game: GameInfo;
  revived: RevivedInfo | null; // the current server is a Dauntless Revived one
  runtime: RuntimeInfo;
  task: TaskProgress | null;
  running: { game: boolean; relayPort: number | null };
  settings: Settings;
  app: { version: string; packaged: boolean; updateReady: boolean; platform: string };
  status: ServerStatus | null;
  statusUnsupported: boolean; // the host answers but has no ServerStatus endpoint
  lastError: LauncherError | null;
  notice: NoticeCode | null;
}

export interface NewsItem {
  date: string | null;
  title: string;
  body: string;
}

export interface Branding {
  backgrounds: { url: string; credit: string | null }[]; // app-internal rr-art:// URLs
  accent: string | null;
}

// ------------------------------------------------------------------ Host tools

// What the Host page shows: the steps a host goes through before friends can play, each one
// detected from files on this PC. Nothing here is secret.
export type GameDataState = "real" | "synthetic" | "missing";

export interface HostKit {
  workspace: string | null; // a Dauntless Reforged Revival checkout (the folder with ReforgedHost\)
  workspaceOk: boolean;
  upstream: {
    present: boolean; // .workspace\Mystic-Paradox holds the pinned source
    patched: boolean; // and the Reforged patches are applied
  };
  sdk: {
    present: boolean; // ParadoxRuntime\SDK.hpp
    functionFiles: number; // ParadoxRuntime\SDK\*_functions.cpp
  };
  dumps: { dir: string; name: string; modified: string }[]; // Dumper-7 CppSDK outputs found on this PC
  dumperDll: string | null; // the Dumper-7 DLL the host picked
  builds: {
    runtime: boolean; // ParadoxRuntime\x64\Release\MysticParadox.dll
    loader: boolean; // tools\RuntimeLoader\target\release\winmm.dll
    exporter: boolean; // tools\CatalogExporter\x64\Release\CatalogExporter.dll
    published: boolean; // both runtime files in the content server's runtime folder
  };
  toolchain: {
    visualStudio: string | null; // e.g. "Visual Studio 2022 (17.14)"
    msbuild: boolean;
    cargo: boolean;
  };
  gameData: { backend: GameDataState; director: GameDataState };
  exports: { present: boolean; inGame: boolean }; // Items_Analysis in the workspace, and beside the game (CatalogExporter's output)
  // The server's .workspace\config\host.json (no secrets there: they are in secrets.env).
  // multipliers: the test XP and reward multipliers (private servers only), null when off.
  serverConfig: { configured: boolean; name: string | null; mode: "private" | "public" | null; host: string | null; multipliers: HostMultipliers | null };
  requirements: Requirement[];
  job: HostJob | null;
  update: HostUpdateView | null; // what the last "Update" found, since the launcher started
  projectTarget: string | null; // where the project folder is downloaded to: beside the game's folder
}

// XP and reward multipliers for a private test server (ReforgedHost's npm run multipliers): 1 to 100 each.
export interface HostMultipliers {
  xp: number;
  rewards: number;
}
export const MAX_MULTIPLIER = 100;

// The project compared with GitHub (ReforgedHost's npm run update).
export interface HostUpdateView {
  state: "up_to_date" | "updated";
  head: string; // the short commit this PC is at now
  files: number; // files the update changed
  runtimeChanged: boolean; // the game runtime changed: build MysticParadox.dll again and publish it
}

// What the Requirements tab lists, each one detected on this PC.
export type RequirementId = "dauntless" | "git" | "node" | "mongodb" | "tailscale" | "visual_studio" | "rust" | "dumper7" | "vc_redist";

export interface Requirement {
  id: RequirementId;
  state: "ok" | "missing" | "problem";
  detail: string | null; // the version or place found; never a secret
  // What is wrong when state is "problem": node "too_old"; mongodb "service_running"; visual_studio "no_cpp".
  problem: string | null;
}

// The server the Host page starts and stops (ReforgedHost's npm run start, run by the launcher).
//  external: the server answers, but was started elsewhere (a terminal): the launcher cannot stop it.
export type HostServerState = "stopped" | "starting" | "running" | "stopping" | "external" | "failed";

export interface HostServerView {
  state: HostServerState;
  message: string | null; // the error line when it failed
  lines: string[]; // the newest output of the server and of the host kit commands started here
  invite: { link: string; for: string | null } | null; // the invite made last (a secret: shown blurred in streamer mode)
}

export type HostJobKind =
  | "inject"
  | "build_runtime"
  | "build_loader"
  | "build_exporter"
  | "import_sdk"
  | "copy_exports"
  | "synthetic_data"
  | "import_gamedata"
  | "publish_runtime"
  | "setup"
  | "generate_gamedata"
  | "export_gamedata"
  | "create_invite"
  | "update"
  | "multipliers"
  | "upstream"
  | "download_project";

export interface HostJob {
  kind: HostJobKind;
  state: "running" | "done" | "failed";
  message: string | null; // the last line of its log, never a secret
  logFile: string | null;
}

// The links the page can ask the main process to open. The page names one of these; it never passes
// a URL. The main process maps each to its URL (src/main/links.ts).
export type ExternalTarget =
  | "tailscale_download"
  | "tailscale_share"
  | "vc_redist"
  | "server_source"
  | "project_source"
  | "project_license"
  | "project_issues"
  | "project_docs_players"
  | "project_docs_hosting"
  | "project_docs_sdk"
  | "installing_guide"
  | "legendary"
  | "dumper7"
  | "visual_studio"
  | "rust"
  | "nodejs"
  | "mongodb"
  | "git"
  | "visual_studio_installer"
  | "rust_installer"
  | "dumper7_zip"
  | "revived_source"
  | "mystic_source"
  | "undaunted_source";

export type RegisterOutcome =
  | { ok: true; username: string }
  | { ok: false; error: LauncherError };

// Channel names (renderer -> main are invoke channels, main -> renderer are events).
export const IPC = {
  getSnapshot: "rr:get-snapshot",
  checkInvite: "rr:check-invite",
  submitInvite: "rr:submit-invite",
  retryConnect: "rr:retry-connect",
  forgetServer: "rr:forget-server",
  switchServer: "rr:switch-server",
  removeSavedServer: "rr:remove-saved-server",
  register: "rr:register",
  useExistingKey: "rr:use-existing-key",
  importKeyFile: "rr:import-key-file",
  saveKeyBackup: "rr:save-key-backup",
  dismissBackupOffer: "rr:dismiss-backup-offer",
  chooseGameFolder: "rr:choose-game-folder",
  useGamePath: "rr:use-game-path",
  useDetectedGame: "rr:use-detected-game",
  verifyGame: "rr:verify-game",
  revivedInstall: "rr:revived-install",
  revivedChooseFolder: "rr:revived-choose-folder",
  revivedUseDetected: "rr:revived-use-detected",
  trustRuntime: "rr:trust-runtime",
  installRuntime: "rr:install-runtime",
  chooseLocalRuntime: "rr:choose-local-runtime",
  clearLocalRuntime: "rr:clear-local-runtime",
  cancelTask: "rr:cancel-task",
  play: "rr:play",
  setSettings: "rr:set-settings",
  setStatusPolling: "rr:set-status-polling",
  refreshStatus: "rr:refresh-status",
  getNews: "rr:get-news",
  getBranding: "rr:get-branding",
  openExternal: "rr:open-external",
  openGameFolder: "rr:open-game-folder",
  openLogsFolder: "rr:open-logs-folder",
  logout: "rr:logout",
  installUpdate: "rr:install-update",
  hostGet: "rr:host-get",
  hostChooseWorkspace: "rr:host-choose-workspace",
  hostChooseDumper: "rr:host-choose-dumper",
  hostImportGameData: "rr:host-import-gamedata",
  hostImportSdkFolder: "rr:host-import-sdk-folder",
  hostDownloadProject: "rr:host-download-project",
  hostAction: "rr:host-action",
  hostOpen: "rr:host-open",
  hostServerGet: "rr:host-server-get",
  hostServerStart: "rr:host-server-start",
  hostServerStop: "rr:host-server-stop",
  hostSetup: "rr:host-setup",
  hostCreateInvite: "rr:host-create-invite",
  hostUpdate: "rr:host-update",
  hostSetMultipliers: "rr:host-set-multipliers",
  copyText: "rr:copy-text",
  windowMinimize: "rr:window-minimize",
  windowMaximize: "rr:window-maximize",
  windowClose: "rr:window-close",
  takeInviteLink: "rr:take-invite-link",
  dismissNotice: "rr:dismiss-notice",
  dismissError: "rr:dismiss-error",
  // events
  inviteLink: "rr:invite-link",
  snapshot: "rr:snapshot",
  progress: "rr:progress",
  hostKit: "rr:host-kit",
  hostServer: "rr:host-server",
  windowState: "rr:window-state",
} as const;

// Host page actions, by name (the page never passes a path or a command).
export type HostAction =
  | "inject_dumper" // start the game without the runtime and load Dumper-7 into it
  | "import_sdk" // copy the newest Dumper-7 CppSDK into the runtime source
  | "build_runtime"
  | "build_loader"
  | "build_exporter"
  | "inject_exporter" // load CatalogExporter into the running game
  | "copy_exports" // copy the exporter's Items_Analysis into the workspace
  | "synthetic_data" // copy the example game data (startup tests only)
  | "publish_runtime" // copy the built runtime files to the content server
  | "generate_gamedata" // copy the game's Items_Analysis in, then npm run gamedata
  | "export_gamedata"; // npm run gamedata -- --export (a game data pack)

export type HostFolder = "workspace" | "runtime_source" | "dumps" | "game_data" | "runtime_publish" | "host_logs" | "gamedata_packs";

// The Host page's server setup form (npm run setup). The game folder is the launcher's own.
export interface HostSetupRequest {
  name: string;
  mode: "private" | "public";
  publicHost: string | null; // public mode: the server's public IPv4 or DNS name
}
