// Fixed values (row L18): pinned hashes, file locations and limits, as in Reforged's
// `src/main/constants.ts`. DR's 1.4.4 pins (its executable, `EXPECTED_BUILD`, `PINNED_DLLS`) are not
// brought in (RD5); RA9 and RA7 add the rest of Reforged's (`GAME_VERSION_TEXT`, `RUNTIME_FILE_NAMES`, …).

// Game layout (paths relative to the "Dauntless" folder, the one that holds Archon/)
pub const WIN64_RELATIVE_DIR: &str = "Archon/Binaries/Win64";
pub const EXE_NAME: &str = "Dauntless-Win64-Shipping.exe";
pub const EXE_RELATIVE_PATH: &str = "Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe";
/// Dauntless 1.14.7 (rel-1.14.7-Archon, changelist 647472): the hash only, no size pin
pub const PINNED_EXE_SHA256: &str = "f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b";

/// The game needs the Visual C++ 2015-2022 runtime (x64).
pub const VC_RUNTIME_FILES: [&str; 2] = ["MSVCP140.dll", "VCRUNTIME140_1.dll"];

/// Chat and presence (XMPP): DR's own port. Reforged's chat goes through the relay; RA6 removes it.
pub const XMPP_PORT: u16 = 61099;

pub const CONNECT_TIMEOUT_MS: u64 = 5000;
pub const STATUS_POLL_MS: u64 = 15000;
/// 256 MiB on top of what is still to download
pub const DISK_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
