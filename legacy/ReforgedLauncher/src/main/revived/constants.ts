// Dauntless 1.4.4 for Dauntless Revived servers, the way Dauntless Revived's launcher 0.1.35 plays it
// (https://github.com/mixutin/dauntless-revived/releases/tag/launcher-v0.1.35). Its later launchers
// changed the client DLL and no longer join the private servers that run 0.1.35's server side, so the
// pins below are 0.1.35's own, from that tag's src/main/constants.ts.

// The tag's commit, and where its two client DLLs are (Undaunted's, AGPL-3.0: built from that repository).
export const REVIVED_LAUNCHER_VERSION = "0.1.35";
export const REVIVED_COMMIT = "0e2b1e6730b9f14f1020d7c903a6955464fa4586";
export const REVIVED_SOURCE_URL = "https://github.com/mixutin/dauntless-revived/tree/launcher-v0.1.35";
const REVIVED_RAW = `https://raw.githubusercontent.com/mixutin/dauntless-revived/${REVIVED_COMMIT}/UndauntedLauncher/assets`;

export const REVIVED_EXE_RELATIVE_PATH = "Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe";
export const REVIVED_EXE_SHA256 = "d3d41e614908d2befd518b27046d9822d6130ef12ba3504babbdb786bef9cff4";
export const REVIVED_EXE_SIZE = 103673520;
export const REVIVED_BUILD = "dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827";

export interface PinnedDll {
  name: string;
  sha256: string;
  size: number;
  url: string;
}

// dxgi.dll loads UndauntedInternalServer.dll into the game, which points it at the server named on its
// command line. Both go next to the game executable; both are checked by hash before and after copying.
export const REVIVED_DLLS: readonly PinnedDll[] = [
  { name: "dxgi.dll", sha256: "9a431d7b6fd20c43fa92bebd91c3bc023ec7a3fcbc52871c41f4df293d4b0d1f", size: 11264, url: `${REVIVED_RAW}/dxgi.dll` },
  { name: "UndauntedInternalServer.dll", sha256: "87aecd7beba33f479f6ef3f335e90e20dcd32fddd31ebd3a23d6bd542b1156a0", size: 261120, url: `${REVIVED_RAW}/UndauntedInternalServer.dll` },
];
export const REVIVED_DLL_RELATIVE_PATHS = REVIVED_DLLS.map((d) => `Archon/Binaries/Win64/${d.name}`);

// The server's launcher API and the header its account key goes in.
export const REVIVED_API = "/undaunted/api";
export const REVIVED_KEY_HEADER = "x-undaunted-user-api-key";
// Chat and presence (XMPP) in private mode: the host itself, on this port.
export const REVIVED_XMPP_PORT = 61099;
export const DOWNLOAD_CONCURRENCY = 4;
export const DISK_MARGIN_BYTES = 1024 * 1024 * 1024; // 1 GiB on top of what is still to download
// The DirectX June 2010 runtime files the 1.4.4 client loads (checked before a launch, as 0.1.35 does).
export const DIRECTX_FILES = ["XINPUT1_3.dll", "X3DAudio1_7.dll", "XAPOFX1_5.dll"];
