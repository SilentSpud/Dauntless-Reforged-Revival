// The only bridge between the page and the main process: a fixed list of typed calls.
// There is no generic send/invoke passthrough, and the account key never crosses this bridge
// towards the page.

import { contextBridge, ipcRenderer } from "electron";
import { IPC } from "./shared/types";
import type {
  ActionResult,
  Branding,
  ExternalTarget,
  HostAction,
  HostFolder,
  HostKit,
  HostMultipliers,
  HostServerView,
  HostSetupRequest,
  InviteCheck,
  NewsItem,
  RegisterOutcome,
  Settings,
  Snapshot,
  TaskProgress,
} from "./shared/types";

const api = {
  getSnapshot: (): Promise<Snapshot> => ipcRenderer.invoke(IPC.getSnapshot),
  checkInvite: (text: string): Promise<InviteCheck> => ipcRenderer.invoke(IPC.checkInvite, String(text)),
  // acceptNewCertificate: the user confirmed a public server's changed certificate.
  // carryAccount: the user confirmed taking their account along to a private server's new address.
  submitInvite: (text: string, acceptNewCertificate = false, carryAccount = false): Promise<ActionResult> =>
    ipcRenderer.invoke(IPC.submitInvite, String(text), acceptNewCertificate === true, carryAccount === true),
  retryConnect: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.retryConnect),
  forgetServer: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.forgetServer),
  switchServer: (id: string): Promise<ActionResult> => ipcRenderer.invoke(IPC.switchServer, String(id)),
  removeSavedServer: (id: string): Promise<ActionResult> => ipcRenderer.invoke(IPC.removeSavedServer, String(id)),
  register: (username: string): Promise<RegisterOutcome> => ipcRenderer.invoke(IPC.register, String(username)),
  useExistingKey: (text: string): Promise<ActionResult> => ipcRenderer.invoke(IPC.useExistingKey, String(text)),
  importKeyFile: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.importKeyFile),
  saveKeyBackup: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.saveKeyBackup),
  dismissBackupOffer: (): Promise<void> => ipcRenderer.invoke(IPC.dismissBackupOffer),
  chooseGameFolder: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.chooseGameFolder),
  useGamePath: (gamePath: string): Promise<ActionResult> => ipcRenderer.invoke(IPC.useGamePath, String(gamePath)),
  useDetectedGame: (index: number): Promise<ActionResult> => ipcRenderer.invoke(IPC.useDetectedGame, Number(index)),
  verifyGame: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.verifyGame),
  revivedInstall: (mode: "install" | "repair"): Promise<ActionResult> => ipcRenderer.invoke(IPC.revivedInstall, mode === "repair" ? "repair" : "install"),
  revivedChooseFolder: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.revivedChooseFolder),
  revivedUseDetected: (index: number): Promise<ActionResult> => ipcRenderer.invoke(IPC.revivedUseDetected, Number(index)),
  trustRuntime: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.trustRuntime),
  installRuntime: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.installRuntime),
  chooseLocalRuntime: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.chooseLocalRuntime),
  clearLocalRuntime: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.clearLocalRuntime),
  cancelTask: (): Promise<void> => ipcRenderer.invoke(IPC.cancelTask),
  play: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.play),
  setSettings: (patch: Partial<Settings>): Promise<Settings> =>
    ipcRenderer.invoke(IPC.setSettings, {
      ...(patch.graphics !== undefined ? { graphics: patch.graphics } : {}),
      ...(patch.windowed !== undefined ? { windowed: patch.windowed } : {}),
      ...(patch.streamerMode !== undefined ? { streamerMode: patch.streamerMode } : {}),
      ...(patch.hostTools !== undefined ? { hostTools: patch.hostTools } : {}),
      ...(patch.language !== undefined ? { language: patch.language } : {}),
      ...(patch.gameLanguage !== undefined ? { gameLanguage: patch.gameLanguage } : {}),
      ...(patch.runtimeProfile !== undefined ? { runtimeProfile: patch.runtimeProfile } : {}),
    }),
  setStatusPolling: (on: boolean): Promise<void> => ipcRenderer.invoke(IPC.setStatusPolling, on === true),
  refreshStatus: (): Promise<void> => ipcRenderer.invoke(IPC.refreshStatus),
  getNews: (): Promise<NewsItem[]> => ipcRenderer.invoke(IPC.getNews),
  getBranding: (): Promise<Branding> => ipcRenderer.invoke(IPC.getBranding),
  openExternal: (target: ExternalTarget): Promise<ActionResult> => ipcRenderer.invoke(IPC.openExternal, String(target)),
  openGameFolder: (): Promise<void> => ipcRenderer.invoke(IPC.openGameFolder),
  openLogsFolder: (): Promise<void> => ipcRenderer.invoke(IPC.openLogsFolder),
  logout: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.logout),
  installUpdate: (): Promise<void> => ipcRenderer.invoke(IPC.installUpdate),
  hostGet: (): Promise<HostKit> => ipcRenderer.invoke(IPC.hostGet),
  hostChooseWorkspace: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostChooseWorkspace),
  hostChooseDumper: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostChooseDumper),
  hostImportGameData: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostImportGameData),
  hostImportSdkFolder: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostImportSdkFolder),
  hostDownloadProject: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostDownloadProject),
  hostAction: (action: HostAction): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostAction, String(action)),
  hostOpen: (folder: HostFolder): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostOpen, String(folder)),
  hostServerGet: (): Promise<HostServerView> => ipcRenderer.invoke(IPC.hostServerGet),
  hostServerStart: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostServerStart),
  hostServerStop: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostServerStop),
  hostSetup: (req: HostSetupRequest): Promise<ActionResult> =>
    ipcRenderer.invoke(IPC.hostSetup, { name: String(req.name), mode: String(req.mode), publicHost: req.publicHost === null ? null : String(req.publicHost) }),
  hostCreateInvite: (forName: string | null): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostCreateInvite, forName === null ? null : String(forName)),
  hostUpdate: (): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostUpdate),
  hostSetMultipliers: (m: HostMultipliers | null): Promise<ActionResult> => ipcRenderer.invoke(IPC.hostSetMultipliers, m === null ? null : { xp: Number(m.xp), rewards: Number(m.rewards) }),
  copyText: (text: string): Promise<boolean> => ipcRenderer.invoke(IPC.copyText, String(text)),
  dismissNotice: (): Promise<void> => ipcRenderer.invoke(IPC.dismissNotice),
  dismissError: (): Promise<void> => ipcRenderer.invoke(IPC.dismissError),
  minimize: (): Promise<void> => ipcRenderer.invoke(IPC.windowMinimize),
  toggleMaximize: (): Promise<boolean> => ipcRenderer.invoke(IPC.windowMaximize),
  close: (): Promise<void> => ipcRenderer.invoke(IPC.windowClose),
  takeInviteLink: (): Promise<string | null> => ipcRenderer.invoke(IPC.takeInviteLink),
  onSnapshot: (fn: (s: Snapshot) => void): void => {
    ipcRenderer.on(IPC.snapshot, (_e, s: Snapshot) => fn(s));
  },
  onProgress: (fn: (p: TaskProgress) => void): void => {
    ipcRenderer.on(IPC.progress, (_e, p: TaskProgress) => fn(p));
  },
  onHostKit: (fn: (k: HostKit) => void): void => {
    ipcRenderer.on(IPC.hostKit, (_e, k: HostKit) => fn(k));
  },
  onHostServer: (fn: (v: HostServerView) => void): void => {
    ipcRenderer.on(IPC.hostServer, (_e, v: HostServerView) => fn(v));
  },
  onInviteLink: (fn: () => void): void => {
    ipcRenderer.on(IPC.inviteLink, () => fn());
  },
  onWindowState: (fn: (maximized: boolean) => void): void => {
    ipcRenderer.on(IPC.windowState, (_e, maximized: unknown) => fn(maximized === true));
  },
};

export type LauncherApi = typeof api;

contextBridge.exposeInMainWorld("launcher", api);
