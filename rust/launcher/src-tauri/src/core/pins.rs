// Fixed values (row L18): pinned hashes, file locations and limits, as in `src/main/constants.ts`.
// build.rs includes this file too (to check the DLLs and the manifest), so it holds constants only.

// Game layout (paths relative to the folder that contains Archon/)
pub const WIN64_RELATIVE_DIR: &str = "Archon/Binaries/Win64";
pub const EXE_NAME: &str = "Dauntless-Win64-Shipping.exe";
pub const EXE_RELATIVE_PATH: &str = "Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe";
pub const PINNED_EXE_SHA256: &str = "d3d41e614908d2befd518b27046d9822d6130ef12ba3504babbdb786bef9cff4";
pub const PINNED_EXE_SIZE: u64 = 103_673_520;

/// The build every manifest must name.
pub const EXPECTED_BUILD: &str = "dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827";

pub struct PinnedDll {
    pub name: &'static str,
    pub sha256: &'static str,
}

/// Same pins as friend-kit/play.ps1. The files ship in the launcher's resources (`assets/`).
pub const PINNED_DLLS: [PinnedDll; 2] = [
    PinnedDll {
        name: "dxgi.dll",
        sha256: "9a431d7b6fd20c43fa92bebd91c3bc023ec7a3fcbc52871c41f4df293d4b0d1f",
    },
    PinnedDll {
        name: "UndauntedInternalServer.dll",
        sha256: "6f2433abebdee451edfc9060fa58eb3e2a3e73e5f7063de05386316b52d7b30b",
    },
];

/// The server DLL needs the Visual C++ 2015-2022 runtime (x64).
pub const VC_RUNTIME_FILES: [&str; 2] = ["MSVCP140.dll", "VCRUNTIME140_1.dll"];

/// Chat and presence (XMPP): pointed at the host instead of Epic, like friend-kit/play.ps1.
pub const XMPP_PORT: u16 = 61099;

pub const CONNECT_TIMEOUT_MS: u64 = 5000;
pub const STATUS_POLL_MS: u64 = 15000;
/// 1 GiB on top of what is still to download
pub const DISK_MARGIN_BYTES: u64 = 1024 * 1024 * 1024;
pub const DOWNLOAD_CONCURRENCY: usize = 4;
