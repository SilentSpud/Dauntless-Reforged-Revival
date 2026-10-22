// A stand-in for the preload bridge, so the launcher's page can be opened in a plain browser with
// `npm run preview` (no Electron, no server, no game). The page is told it is in one of a fixed set
// of scenes through the address: ?scene=ready&lang=pt-BR&view=host. Nothing here ships.

import type { LauncherApi } from "../src/preload";
import type { HostKit, HostServerView, Language, NewsItem, Phase, Snapshot, TaskProgress } from "../src/shared/types";
import type { ServerStatus } from "../src/shared/status";

const params = new URLSearchParams(location.search);
const scene = params.get("scene") ?? "ready";
const lang = (params.get("lang") === "pt-BR" ? "pt-BR" : "en") as Language;

const FP = "3f1c9a0b7d2e4f6a8b0c1d2e3f405162738495a6b7c8d9e0f1a2b3c4d5e6f708";
const now = Date.now();

const status: ServerStatus = {
  name: "Friday Night Hunts",
  online: true,
  version: "0.1.0",
  commit: "a4cb7cf",
  sourceUrl: "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival",
  registration: "INVITECODE",
  playersOnline: 4,
  players: [
    { name: "Aurora", where: "city", instance: "city-8777" },
    { name: "Borealis", where: "hunt", instance: "hunt-8770" },
    { name: "Cirrus", where: "hunt", instance: "hunt-8770" },
    { name: "Dawnbreaker", where: "dojo", instance: "dojo-8776" },
  ],
  instances: [
    { id: "city-8777", kind: "city", title: "Ramsgate", map: "Hub", behemoth: null, players: 1, maxPlayers: 100, startedAt: new Date(now - 3 * 3600_000).toISOString() },
    { id: "hunt-8770", kind: "hunt", title: "Shrike", map: "Island", behemoth: "Shrike", players: 2, maxPlayers: 4, startedAt: new Date(now - 9 * 60_000).toISOString() },
    { id: "dojo-8776", kind: "dojo", title: "Training Grounds", map: "Dojo", behemoth: null, players: 1, maxPlayers: 1, startedAt: new Date(now - 40 * 60_000).toISOString() },
  ],
  uptimeSeconds: 3 * 3600 + 120,
  game: { build: "rel-1.14.7-Archon", changelist: 647472 },
  gameData: scene === "synthetic" ? "synthetic" : "real",
  chat: true,
  contentPort: null,
  limited: false,
};

const runtimeFiles = [
  { name: "MysticParadox.dll", size: 4_718_592, sha256: "9b3d0f2c6a4e8b1d7c5f3a9e0b2d4f6a8c0e1f3a5b7d9c1e3f5a7b9d1c3e5f70" },
  { name: "winmm.dll", size: 233_472, sha256: "e1c3a5b7d9f0e2c4a6b8d0f1e3c5a7b9d1f3e5c7a9b0d2f4e6c8a0b1d3f5e7c9" },
];

function base(phase: Phase): Snapshot {
  return {
    phase,
    busy: false,
    server: { game: "reforged", mode: "public", host: "hunts.example.net", port: 443, name: status.name, hasShare: false, hasPendingInvite: true, loopback: false, fingerprint: FP },
    savedServers: [
      { id: "aaaaaaaaaaaaaaaaaaaaaaaa", game: "reforged", mode: "public", host: "hunts.example.net", port: 443, name: status.name, current: true, username: "Slayer_42" },
      { id: "bbbbbbbbbbbbbbbbbbbbbbbb", game: "reforged", mode: "private", host: "100.101.102.103", port: 61000, name: "LAN Party", current: false, username: "Slayer_42" },
      { id: "cccccccccccccccccccccccc", game: "revived", mode: "private", host: "100.88.12.40", port: 61000, name: "Revived Fridays", current: false, username: "Slayer_42" },
    ],
    connect: { checking: false, problem: null, tailscaleInstalled: true, lastCheckedAt: new Date(now - 30_000).toISOString() },
    account: { username: "Slayer_42", hasKey: true, offerBackup: false },
    game: {
      root: "C:\\Games\\Dauntless",
      problem: null,
      versionText: "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472",
      versionOk: true,
      exeVerified: true,
      detected: [{ root: "C:\\Games\\Dauntless", source: "legendary", version: "1.14.7" }],
    },
    revived: null,
    runtime: { state: "ok", version: "20261014-1a2b3c4d", files: runtimeFiles, localDir: null },
    task: null,
    running: { game: false, relayPort: null },
    settings: { graphics: -1, windowed: false, language: lang, gameLanguage: "auto", runtimeProfile: "production", streamerMode: params.get("streamer") === "1", hostTools: true },
    app: { version: "0.1.0", packaged: false, updateReady: false, platform: "win32" },
    status,
    statusUnsupported: false,
    lastError: null,
    notice: null,
  };
}

function sceneSnapshot(): Snapshot {
  const s = base("ready");
  switch (scene) {
    case "join":
      return { ...s, phase: "join", server: null, savedServers: s.savedServers.map((v) => ({ ...v, current: false })), status: null, account: { username: null, hasKey: false, offerBackup: false }, settings: { ...s.settings, hostTools: false } };
    case "tailscale":
      return { ...s, phase: "connect", server: { ...s.server!, mode: "private", host: "100.101.102.103", port: 61000, fingerprint: null, hasShare: true }, connect: { ...s.connect, problem: "tailscale_missing", tailscaleInstalled: false }, status: null };
    case "cert":
      return { ...s, phase: "connect", connect: { ...s.connect, problem: "cert_mismatch" }, status: null };
    case "register":
      return { ...s, phase: "register", account: { username: null, hasKey: false, offerBackup: false }, status: { ...status, limited: true, players: [], instances: [], playersOnline: 0 } };
    case "locate":
      return { ...s, phase: "locate", game: { root: null, problem: "not_set", versionText: null, versionOk: false, exeVerified: false, detected: [] } };
    case "wrong":
      return { ...s, phase: "locate", game: { ...s.game, problem: "wrong_version", versionText: "dauntless_rel-1.12.0_Shipping_2023-11-02_10-00-00_612345", versionOk: false, exeVerified: false } };
    case "trust":
      return { ...s, phase: "runtime", runtime: { ...s.runtime, state: "untrusted" } };
    case "noruntime":
      return { ...s, phase: "runtime", runtime: { state: "unavailable", version: null, files: [], localDir: null } };
    case "installing":
      return { ...s, phase: "installing", task: progress };
    case "running":
      return { ...s, phase: "running", running: { game: true, relayPort: 61100 }, notice: "keep_open" };
    case "synthetic":
      return { ...s, account: { ...s.account, offerBackup: true } };
    case "error":
      return { ...s, lastError: { code: "game_wrong_version" } };
    // A Dauntless Revived server (Dauntless 1.4.4): nothing downloaded, partly there, being downloaded, ready.
    case "revived":
    case "revived-repair":
    case "revived-download":
    case "revived-ready": {
      const server = { ...s.server!, game: "revived" as const, mode: "private" as const, host: "100.88.12.40", port: 61000, fingerprint: null, name: "Revived Fridays" };
      const savedServers = s.savedServers.map((v) => ({ ...v, current: v.game === "revived" }));
      const revived = {
        root: "C:\\Users\\Slayer\\AppData\\Local\\DauntlessRevived\\Game",
        chosen: scene !== "revived",
        problem: scene === "revived-ready" ? null : scene === "revived" ? ("not_set" as const) : ("unverified" as const),
        missingFiles: scene === "revived-repair" ? 12 : scene === "revived" ? 410 : 0,
        totalBytes: 10_893_512_875,
        dllsOk: scene === "revived-ready",
        detected: [],
      };
      const st = { ...status, name: "Revived Fridays", game: { build: "dauntless_rel-1.4.4", changelist: null } };
      if (scene === "revived-download") return { ...s, phase: "installing", server, savedServers, revived, status: st, task: { kind: "download", totalBytes: 10_893_512_875, doneBytes: 3_402_000_000, filesTotal: 410, filesDone: 131, bytesPerSecond: 28_400_000, etaSeconds: 263, currentFile: "Archon/Content/Paks/pakchunk0-WindowsNoEditor.pak" } };
      return { ...s, phase: scene === "revived-ready" ? "ready" : "locate", server, savedServers, revived, status: st };
    }
    default:
      return s;
  }
}

const progress: TaskProgress = { kind: "runtime", totalBytes: 4_952_064, doneBytes: 2_100_000, filesTotal: 2, filesDone: 0, bytesPerSecond: 1_800_000, etaSeconds: 2, currentFile: "MysticParadox.dll" };

const news: NewsItem[] = [
  { date: new Date(now - 86400_000).toISOString(), title: "Escalations are back", body: "Heroic escalations now run on the server. Bring a full party and a lot of potions." },
  { date: new Date(now - 4 * 86400_000).toISOString(), title: "Welcome to Reforged", body: "Thanks for joining. Ramsgate is open every Friday from 20:00." },
];

const hostKit: HostKit = {
  workspace: scene === "host-noproject" ? null : "C:\\Users\\host\\Dauntless-Reforged-Revival",
  workspaceOk: scene !== "host-noproject",
  upstream: { present: scene !== "host-empty", patched: scene !== "host-empty" },
  sdk: { present: scene !== "host-empty", functionFiles: scene !== "host-empty" ? 412 : 0 },
  dumps: [{ dir: "C:\\Dumper-7\\4.26.2-647472-Dauntless\\CppSDK", name: "4.26.2-647472-Dauntless", modified: new Date(now - 3600_000).toISOString() }],
  dumperDll: "C:\\Tools\\Dumper-7.dll",
  builds: { runtime: scene !== "host-empty", loader: scene !== "host-empty", exporter: false, published: false },
  toolchain: { visualStudio: "Visual Studio Community 2022 (17.14)", msbuild: true, cargo: true },
  gameData: { backend: "synthetic", director: "synthetic" },
  exports: { present: false, inGame: scene !== "host-empty" },
  serverConfig: scene === "host-empty" ? { configured: false, name: null, mode: null, host: null, multipliers: null } : { configured: true, name: "Friday Night Hunts", mode: "private", host: "100.101.102.103", multipliers: scene === "host-running" ? { xp: 5, rewards: 3 } : null },
  requirements: [
    { id: "dauntless", state: "ok", detail: "1.14.7 (647472)", problem: null },
    { id: "git", state: "ok", detail: "2.47.1.windows.1", problem: null },
    { id: "node", state: scene === "host-empty" ? "missing" : "ok", detail: scene === "host-empty" ? null : "v22.11.0", problem: null },
    { id: "mongodb", state: "problem", detail: "8.0", problem: "service_running" },
    { id: "tailscale", state: "ok", detail: null, problem: null },
    { id: "visual_studio", state: scene === "host-empty" ? "problem" : "ok", detail: "Visual Studio Community 2022", problem: scene === "host-empty" ? "no_cpp" : null },
    { id: "rust", state: "missing", detail: null, problem: null },
    { id: "dumper7", state: "ok", detail: "Dumper-7.dll", problem: null },
    { id: "vc_redist", state: "ok", detail: null, problem: null },
  ],
  projectTarget: "D:\\Games\\D1147\\Reforged",
  update: scene === "host-running" ? { state: "updated", head: "9377a20c41f6", files: 12, runtimeChanged: true } : null,
  job: scene === "host-empty" ? null : { kind: "build_loader", state: "done", message: "Finished `release` profile [optimized] target(s) in 41.2s", logFile: "C:\\Users\\host\\Dauntless-Reforged-Revival\\.workspace\\logs\\build_loader.log" },
};

// The Host page's server: Start runs it for a moment, then it is "Online"; Stop stops it.
let hostServer: HostServerView = {
  state: scene === "host-running" ? "running" : "stopped",
  message: null,
  lines: scene === "host-running" ? ["> start", "mongod   | waiting for connections on port 27017", "backend  | listening on 127.0.0.1:61000", "director | 0 instances", "content  | serving news and the runtime", "gateway  | private mode on 100.101.102.103:61000", 'Server "Friday Night Hunts" is up: http://100.101.102.103:61000 inside your tailnet'] : [],
  invite: scene === "host-running" ? { link: "reforged-revival://join?v=1&m=private&h=100.101.102.103&p=61000&c=RR-7K2M-9QXA&n=Friday%20Night%20Hunts", for: "Alex" } : null,
};
const hostServerListeners: ((v: HostServerView) => void)[] = [];
const hostKitListeners: ((k: HostKit) => void)[] = [];
function setHostServer(patch: Partial<HostServerView>): void {
  hostServer = { ...hostServer, ...patch };
  for (const l of hostServerListeners) l(hostServer);
}

let snap = sceneSnapshot();
const snapshotListeners: ((s: Snapshot) => void)[] = [];
const ok = async () => ({ ok: true as const });

function update(patch: Partial<Snapshot>): void {
  snap = { ...snap, ...patch };
  for (const l of snapshotListeners) l(snap);
}

const mock: LauncherApi = {
  getSnapshot: async () => snap,
  checkInvite: async () => ({ ok: true, certificateChanged: false, previousFingerprint: null, movedFrom: scene === "moved" ? "100.117.171.17" : null }),
  submitInvite: ok,
  retryConnect: ok,
  forgetServer: ok,
  switchServer: ok,
  removeSavedServer: ok,
  register: async (username) => ({ ok: true, username }),
  useExistingKey: ok,
  importKeyFile: ok,
  saveKeyBackup: ok,
  dismissBackupOffer: async () => undefined,
  chooseGameFolder: ok,
  useGamePath: ok,
  useDetectedGame: ok,
  verifyGame: ok,
  trustRuntime: ok,
  installRuntime: ok,
  chooseLocalRuntime: ok,
  clearLocalRuntime: ok,
  cancelTask: async () => undefined,
  play: ok,
  setSettings: async (patch) => {
    update({ settings: { ...snap.settings, ...patch } });
    return snap.settings;
  },
  setStatusPolling: async () => undefined,
  refreshStatus: async () => undefined,
  getNews: async () => news,
  getBranding: async () => ({ backgrounds: [], accent: null }),
  openExternal: ok,
  openGameFolder: async () => undefined,
  openLogsFolder: async () => undefined,
  logout: ok,
  installUpdate: async () => undefined,
  hostGet: async () => hostKit,
  hostChooseWorkspace: ok,
  hostChooseDumper: ok,
  hostImportGameData: ok,
  hostImportSdkFolder: ok,
  revivedInstall: ok,
  revivedChooseFolder: ok,
  revivedUseDetected: ok,
  hostDownloadProject: ok,
  hostAction: ok,
  hostOpen: ok,
  hostServerGet: async () => hostServer,
  hostServerStart: async () => {
    setHostServer({ state: "starting", lines: [...hostServer.lines, "> start", "mongod   | waiting for connections on port 27017"] });
    setTimeout(() => setHostServer({ state: "running", lines: [...hostServer.lines, "backend  | listening on 127.0.0.1:61000", 'Server "Friday Night Hunts" is up'] }), 1200);
    return { ok: true };
  },
  hostServerStop: async () => {
    setHostServer({ state: "stopping" });
    setTimeout(() => setHostServer({ state: "stopped", lines: [...hostServer.lines, "Stopped."] }), 800);
    return { ok: true };
  },
  hostSetup: ok,
  hostUpdate: async () => {
    hostKit.update = { state: "up_to_date", head: "9377a20c41f6", files: 0, runtimeChanged: false };
    for (const l of hostKitListeners) l(hostKit);
    return { ok: true };
  },
  hostSetMultipliers: async (m) => {
    hostKit.serverConfig = { ...hostKit.serverConfig, multipliers: m };
    for (const l of hostKitListeners) l(hostKit);
    return { ok: true };
  },
  hostCreateInvite: async (forName) => {
    setHostServer({ invite: { link: "reforged-revival://join?v=1&m=private&h=100.101.102.103&p=61000&c=RR-4D8F-2WJN&n=Friday%20Night%20Hunts", for: forName } });
    return { ok: true };
  },
  copyText: async () => true,
  dismissNotice: async () => update({ notice: null }),
  dismissError: async () => update({ lastError: null }),
  minimize: async () => undefined,
  toggleMaximize: async () => false,
  close: async () => undefined,
  takeInviteLink: async () => null,
  onSnapshot: (fn) => {
    snapshotListeners.push(fn);
  },
  onProgress: (fn) => {
    if (scene === "installing") setTimeout(() => fn(progress), 50);
  },
  onHostKit: (fn) => {
    hostKitListeners.push(fn);
  },
  onHostServer: (fn) => {
    hostServerListeners.push(fn);
  },
  onInviteLink: () => undefined,
  onWindowState: () => undefined,
};

(window as unknown as { launcher: LauncherApi }).launcher = mock;

// Opens a page other than Play once the page is up (?view=host).
const view = params.get("view");
if (view) {
  window.addEventListener("load", () => {
    setTimeout(() => {
      if (view === "credits") document.getElementById("credits-btn")?.click();
      else document.querySelector<HTMLButtonElement>(`[data-fk="nav-${view}"]`)?.click();
      const tab = params.get("tab");
      if (tab) setTimeout(() => document.querySelector<HTMLButtonElement>(`[data-fk="set-tab-${tab}"]`)?.click(), 50);
      if (params.get("modal") === "trust") setTimeout(() => document.getElementById("primary")?.click(), 80);
    }, 150);
  });
} else if (params.get("modal") === "trust") {
  window.addEventListener("load", () => setTimeout(() => document.getElementById("primary")?.click(), 250));
}
