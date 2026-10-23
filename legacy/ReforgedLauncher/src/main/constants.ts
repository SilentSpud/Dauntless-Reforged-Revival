// Fixed values: the pinned Dauntless 1.14.7 build, file locations, ports and the few public URLs the
// launcher may open.

export const APP_ID = "io.github.freezamotoqueiro.dauntlessreforgedrevival";
export const SQUIRREL_NAME = "ReforgedRevivalLauncher";
export const PROJECT_URL = "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival";
// A host's project folder is a git clone of it, named so, beside the game's folder (Host page).
export const PROJECT_GIT_URL = `${PROJECT_URL}.git`;
export const PROJECT_FOLDER_NAME = "Reforged";
export const PROJECT_LICENSE_URL = `${PROJECT_URL}/blob/main/LICENSE.txt`;
export const PROJECT_ISSUES_URL = `${PROJECT_URL}/issues`;
export const PROJECT_DOCS_PLAYERS_URL = `${PROJECT_URL}/blob/main/docs/players.md`;
export const PROJECT_DOCS_HOSTING_URL = `${PROJECT_URL}/blob/main/docs/hosting.md`;
export const PROJECT_DOCS_SDK_URL = `${PROJECT_URL}/blob/main/docs/sdk-and-game-data.md`;
// The projects this one is built from.
export const REVIVED_URL = "https://github.com/mixutin/dauntless-revived";
export const MYSTIC_URL = "https://github.com/pranav158/Mystic-Paradox";
export const UNDAUNTED_URL = "https://github.com/SyST3MDeV/Undaunted";
export const INSTALLING_GUIDE_URL = "https://github.com/pranav158/Mystic-Paradox/blob/main/docs/INSTALLING_DAUNTLESS.md";
export const LEGENDARY_URL = "https://github.com/legendary-gl/legendary/releases/latest";
export const DUMPER7_URL = "https://github.com/Encryqed/Dumper-7";
export const VISUAL_STUDIO_URL = "https://visualstudio.microsoft.com/downloads/";
export const RUST_URL = "https://www.rust-lang.org/tools/install";
export const NODEJS_URL = "https://nodejs.org/en/download";
export const MONGODB_URL = "https://www.mongodb.com/try/download/community";
export const TAILSCALE_DOWNLOAD_URL = "https://tailscale.com/download";
// Direct downloads for the Host page's Requirements tab: each starts the official installer, or the
// source archive (Dumper-7 is built from source).
export const GIT_URL = "https://git-scm.com/download/win";
export const VISUAL_STUDIO_INSTALLER_URL = "https://aka.ms/vs/17/release/vs_community.exe";
export const RUST_INSTALLER_URL = "https://win.rustup.rs/x86_64";
export const DUMPER7_ZIP_URL = "https://github.com/Encryqed/Dumper-7/archive/refs/heads/main.zip";
export const VC_REDIST_URL = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
// Self-update feed: a rolling GitHub release that the release workflow keeps current.
export const UPDATE_FEED_URL = `${PROJECT_URL}/releases/download/launcher-updates`;

// ------------------------------------------------------------------ the game

// Dauntless 1.14.7 (rel-1.14.7-Archon, changelist 647472), the final release. The values are the
// ones Mystic Paradox's install guide gives for the Epic build installed with Legendary.
export const GAME_BUILD = "rel-1.14.7-Archon";
export const GAME_CHANGELIST = 647472;
export const GAME_VERSION_TEXT = "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472";
export const PINNED_EXE_SHA256 = "f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b";
// Epic's internal name for Dauntless (Legendary and Heroic use it too).
export const EPIC_APP_NAME = "Jackal";

// Game layout (paths relative to the "Dauntless" folder, the one that holds Archon/).
export const WIN64_RELATIVE_DIR = "Archon/Binaries/Win64";
export const EXE_NAME = "Dauntless-Win64-Shipping.exe";
export const EXE_RELATIVE_PATH = `${WIN64_RELATIVE_DIR}/${EXE_NAME}`;
export const VERSION_FILE = "Version.txt";

// The runtime files a server may hand out, and nothing else: the Mystic Paradox runtime and the
// winmm.dll proxy that loads it when the game starts. They go next to the game executable.
export const RUNTIME_FILE_NAMES = ["MysticParadox.dll", "winmm.dll"] as const;
export const MAX_RUNTIME_FILE_BYTES = 64 * 1024 * 1024;
// Written next to the runtime files: which server's runtime is installed (names and hashes only).
export const RUNTIME_MARKER_FILE = ".reforged-runtime.json";

// ------------------------------------------------------------------ ports and timings

// The local relay. The game's backend calls (REFORGED_BACKEND_URL) and its chat WebSocket go to
// http://127.0.0.1:61100, and the relay carries them to the server: over TLS pinned to the invite's
// certificate in public mode, over Tailscale in private mode. The server's QOS_TARGET_URL points
// at the same address, and the relay answers those region pings itself.
export const DEFAULT_RELAY_PORT = 61100;

export const CONNECT_TIMEOUT_MS = 5000;
export const STATUS_POLL_MS = 15000;
export const DISK_MARGIN_BYTES = 256 * 1024 * 1024;
