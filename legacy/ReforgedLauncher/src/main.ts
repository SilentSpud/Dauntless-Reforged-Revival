// Dauntless Reforged Revival Launcher - Electron main process.
//
// Security model (Dauntless Revived's): the renderer is sandboxed, has no Node access and no
// network access at all (every request is made here, in the main process, and only to the host
// from the invite). It can only call the small typed API in preload.ts, and every argument is
// checked again here.

import { app, autoUpdater, BrowserWindow, clipboard, dialog, ipcMain, Menu, nativeTheme, protocol, safeStorage, screen, session, shell, type IpcMainInvokeEvent } from "electron";
import path from "node:path";
import { existsSync, promises as fsp } from "node:fs";
import { pathToFileURL } from "node:url";
import started from "electron-squirrel-startup";
import { updateElectronApp, UpdateSourceType } from "update-electron-app";
import { Controller, type Platform } from "./main/controller";
import { findRunningClients } from "./main/launch";
import { injectDll } from "./main/injector";
import { findTailscale } from "./main/system";
import { describeError, log, logToFile } from "./main/log";
import { boundedString, externalTarget, hostAction, hostFolder, hostMultipliers, hostSetupRequest, inviteFor, isTrustedPageUrl, relayPortOverride, settingsPatch } from "./main/ipc-validate";
import { INVITE_SCHEME, parseInvite } from "./shared/invite";
import { IPC, type HostKit, type HostServerView, type Snapshot, type TaskProgress } from "./shared/types";
import { systemLanguage, translate } from "./shared/i18n";
import { APP_ID, SQUIRREL_NAME, UPDATE_FEED_URL } from "./main/constants";

// Squirrel install / update / uninstall events: create or remove shortcuts, then quit.
if (started) app.quit();

// Every renderer runs in the Chromium sandbox, whatever a window's own options say.
app.enableSandbox();
if (process.argv.includes("--safe-mode")) app.disableHardwareAcceleration();
nativeTheme.themeSource = "dark";
app.setAppUserModelId(process.platform === "win32" && app.isPackaged ? `com.squirrel.${SQUIRREL_NAME}.${SQUIRREL_NAME}` : APP_ID);

// Art from the host's art pack is served to the renderer through this app-internal scheme.
protocol.registerSchemesAsPrivileged([{ scheme: "rr-art", privileges: { standard: true, secure: true } }]);

const DEV_SERVER_URL: string | undefined = MAIN_WINDOW_VITE_DEV_SERVER_URL;
const RENDERER_INDEX = path.join(__dirname, `../renderer/${MAIN_WINDOW_VITE_NAME}/index.html`);
const RENDERER_INDEX_URL = pathToFileURL(RENDERER_INDEX).toString();

// The same policy as the page's own meta tag, sent as a header where one can be sent (dev server).
const CSP =
  "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' rr-art: data:; font-src 'self'; " +
  "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'";

let mainWindow: BrowserWindow | null = null;
let controller: Controller | null = null;
let pendingInviteLink: string | null = null;
let quitting = false;

function resourcesDir(): string {
  return app.isPackaged ? process.resourcesPath : path.join(app.getAppPath(), "assets");
}

// ------------------------------------------------------------------ invite links

function inviteFromArgv(argv: string[]): string | null {
  for (const arg of argv) {
    if (typeof arg === "string" && arg.toLowerCase().startsWith(`${INVITE_SCHEME}:`)) {
      return parseInvite(arg).ok ? arg.trim() : null;
    }
  }
  return null;
}

function deliverInviteLink(link: string | null): void {
  if (!link) return;
  pendingInviteLink = link;
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC.inviteLink);
}

function registerProtocolClient(): void {
  if (!app.isPackaged) return; // a dev build must not take over the scheme
  // Squirrel keeps a stub exe one folder up that always starts the newest version on Windows.
  const stub = path.resolve(path.dirname(process.execPath), "..", path.basename(process.execPath));
  const target = process.platform === "win32" && existsSync(stub) ? stub : process.execPath;
  if (!app.setAsDefaultProtocolClient(INVITE_SCHEME, target, [])) log.warn("could not register the invite link handler");
}

// ------------------------------------------------------------------ window

function createWindow(): void {
  const iconPath = path.join(resourcesDir(), "icon.png");
  // Never larger than the primary screen's usable area: the big button at the bottom must stay on
  // screen on 1366x768 laptops and at 150 % scaling.
  const work = screen.getPrimaryDisplay().workAreaSize;
  mainWindow = new BrowserWindow({
    width: Math.min(1280, work.width),
    height: Math.min(760, work.height),
    minWidth: Math.min(1024, work.width),
    minHeight: Math.min(640, work.height),
    frame: false,
    show: false,
    // The page's ink (styles.css), so nothing flashes before it paints.
    backgroundColor: "#03080F",
    title: "Dauntless Reforged Revival Launcher",
    icon: existsSync(iconPath) ? iconPath : undefined,
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
      navigateOnDragDrop: false,
      spellcheck: false,
      devTools: !app.isPackaged,
      safeDialogs: true,
      disableBlinkFeatures: "Auxclick",
    },
  });
  mainWindow.removeMenu();
  mainWindow.once("ready-to-show", () => mainWindow?.show());
  const window = mainWindow;
  const showTimer = setTimeout(() => {
    if (!window.isDestroyed()) window.show();
  }, 10000);
  window.once("closed", () => clearTimeout(showTimer));
  window.webContents.on("render-process-gone", (_event, details) => {
    log.error(`launcher renderer stopped: ${details.reason}`);
    if (!process.argv.includes("--safe-mode") && !controller?.relayActive) {
      app.relaunch({ args: [...process.argv.slice(1), "--safe-mode"] });
      app.exit(0);
    } else {
      dialog.showErrorBox("Dauntless Reforged Revival could not open", "The launcher page stopped. Reinstall the latest launcher and send the launcher logs to the host.");
    }
  });
  mainWindow.on("maximize", () => mainWindow?.webContents.send(IPC.windowState, true));
  mainWindow.on("unmaximize", () => mainWindow?.webContents.send(IPC.windowState, false));

  // The launcher carries the game's connection. Closing it would cut the game off, so ask first.
  mainWindow.on("close", (event) => {
    if (quitting || !controller?.relayActive || !mainWindow) return;
    const lang = controller.snapshot().settings.language;
    const choice = dialog.showMessageBoxSync(mainWindow, {
      type: "warning",
      title: translate(lang, "app_title"),
      message: translate(lang, "close_title"),
      detail: translate(lang, "close_text"),
      buttons: [translate(lang, "close_keep"), translate(lang, "close_anyway")],
      defaultId: 0,
      cancelId: 0,
      noLink: true,
    });
    if (choice === 0) {
      event.preventDefault();
      mainWindow.minimize();
      controller.setNotice("keep_open");
    }
  });
  mainWindow.on("closed", () => {
    mainWindow = null;
  });

  const loaded = DEV_SERVER_URL ? mainWindow.loadURL(DEV_SERVER_URL) : mainWindow.loadFile(RENDERER_INDEX);
  void loaded.catch((error) => {
    log.error(`launcher page failed: ${describeError(error)}`);
    dialog.showErrorBox("Dauntless Reforged Revival could not open", "The launcher page could not load. Reinstall the latest launcher. Your account and game files are kept.");
  });
}

function hardenSessions(): void {
  const ses = session.defaultSession;
  ses.setPermissionRequestHandler((_wc, _permission, callback) => callback(false));
  ses.setPermissionCheckHandler(() => false);
  ses.setDevicePermissionHandler(() => false);
  ses.on("will-download", (event) => event.preventDefault());
  // The renderer may load its own files and the rr-art images, nothing from the network.
  ses.webRequest.onBeforeRequest((details, callback) => {
    const url = details.url;
    const allowed =
      url.startsWith("file:") ||
      url.startsWith("rr-art:") ||
      url.startsWith("devtools:") ||
      url.startsWith("data:") ||
      (DEV_SERVER_URL !== undefined && isTrustedPageUrl(url, DEV_SERVER_URL, RENDERER_INDEX_URL)) ||
      (DEV_SERVER_URL !== undefined && url.startsWith(DEV_SERVER_URL.replace(/^http/, "ws")));
    callback({ cancel: !allowed });
  });
  if (DEV_SERVER_URL) {
    ses.webRequest.onHeadersReceived((details, callback) => {
      callback({ responseHeaders: { ...details.responseHeaders, "Content-Security-Policy": [CSP.replace("connect-src 'self'", "connect-src 'self' ws:")] } });
    });
  }

  protocol.handle("rr-art", async (request) => {
    try {
      const url = new URL(request.url);
      const id = url.pathname.replace(/^\//, "");
      const art = url.hostname === "bg" && controller ? controller.resolveArt(id) : null;
      if (!art) return new Response(null, { status: 404 });
      const data = await fsp.readFile(art.file);
      return new Response(data, { status: 200, headers: { "content-type": art.type, "cache-control": "no-store", "x-content-type-options": "nosniff" } });
    } catch {
      return new Response(null, { status: 404 });
    }
  });
}

app.on("web-contents-created", (_e, contents) => {
  contents.setWindowOpenHandler(() => ({ action: "deny" }));
  contents.on("will-navigate", (event, url) => {
    if (!isTrustedPageUrl(url, DEV_SERVER_URL, RENDERER_INDEX_URL)) event.preventDefault();
  });
  contents.on("will-redirect", (event, url) => {
    if (!isTrustedPageUrl(url, DEV_SERVER_URL, RENDERER_INDEX_URL)) event.preventDefault();
  });
  contents.on("will-attach-webview", (event) => event.preventDefault());
});

// ------------------------------------------------------------------ IPC

function trusted(event: IpcMainInvokeEvent): boolean {
  const frame = event.senderFrame;
  return (
    mainWindow !== null &&
    event.sender === mainWindow.webContents &&
    frame !== null &&
    frame === mainWindow.webContents.mainFrame &&
    isTrustedPageUrl(frame.url, DEV_SERVER_URL, RENDERER_INDEX_URL)
  );
}

function handle(channel: string, fn: (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown): void {
  ipcMain.handle(channel, async (event, ...args) => {
    if (!trusted(event)) {
      log.warn(`refused IPC ${channel} from an untrusted frame`);
      throw new Error("refused");
    }
    try {
      return await fn(event, ...args);
    } catch (e) {
      log.error(`IPC ${channel} failed: ${describeError(e)}`);
      return { ok: false, error: { code: "unknown" } };
    }
  });
}

const SAVED_ID = /^[0-9a-f]{24}$/;

function registerIpc(c: Controller): void {
  handle(IPC.getSnapshot, () => c.snapshot());
  handle(IPC.checkInvite, (_e, text) => {
    const s = boundedString(text, 2048);
    return s === null ? { ok: false, error: { code: "invite_invalid_format" } } : c.checkInvite(s);
  });
  handle(IPC.submitInvite, (_e, text, acceptNewCertificate, carryAccount) => {
    const s = boundedString(text, 2048);
    return s === null ? { ok: false, error: { code: "invite_invalid_format" } } : c.submitInvite(s, { acceptNewCertificate: acceptNewCertificate === true, carryAccount: carryAccount === true });
  });
  handle(IPC.retryConnect, () => c.connect());
  handle(IPC.forgetServer, () => c.forgetServer());
  handle(IPC.switchServer, (_e, id) => (typeof id === "string" && SAVED_ID.test(id) ? c.switchServer(id) : { ok: false, error: { code: "invite_invalid_format" } }));
  handle(IPC.removeSavedServer, (_e, id) => (typeof id === "string" && SAVED_ID.test(id) ? c.removeSavedServer(id) : { ok: false, error: { code: "invite_invalid_format" } }));
  handle(IPC.register, (_e, username) => {
    const s = boundedString(username, 32);
    return s === null ? { ok: false, error: { code: "username_invalid" } } : c.register(s);
  });
  handle(IPC.useExistingKey, (_e, text) => {
    const s = boundedString(text, 8192);
    return s === null ? { ok: false, error: { code: "key_invalid" } } : c.useExistingKey(s);
  });
  handle(IPC.importKeyFile, () => c.importKeyFile());
  handle(IPC.saveKeyBackup, () => c.saveKeyBackup());
  handle(IPC.dismissBackupOffer, () => c.dismissBackupOffer());
  handle(IPC.chooseGameFolder, () => c.chooseGameFolder());
  handle(IPC.useGamePath, (_e, value) => {
    const s = boundedString(value, 260);
    return s === null ? { ok: false, error: { code: "folder_invalid" } } : c.useGamePath(s);
  });
  handle(IPC.useDetectedGame, (_e, index) => (typeof index === "number" && Number.isInteger(index) && index >= 0 && index < 16 ? c.useDetectedGame(index) : { ok: false, error: { code: "folder_invalid" } }));
  handle(IPC.verifyGame, () => c.verifyGame());
  handle(IPC.revivedInstall, (_e, mode) => (mode === "install" || mode === "repair" ? c.installRevived(mode) : { ok: false, error: { code: "unknown" } }));
  handle(IPC.revivedChooseFolder, () => c.chooseRevivedFolder());
  handle(IPC.revivedUseDetected, (_e, index) => (typeof index === "number" && Number.isInteger(index) && index >= 0 && index < 16 ? c.useDetectedRevived(index) : { ok: false, error: { code: "folder_invalid" } }));
  handle(IPC.trustRuntime, () => c.trustRuntime());
  handle(IPC.installRuntime, () => c.installServerRuntime());
  handle(IPC.chooseLocalRuntime, () => c.chooseLocalRuntime());
  handle(IPC.clearLocalRuntime, () => c.clearLocalRuntime());
  handle(IPC.cancelTask, () => c.cancelTask());
  handle(IPC.play, () => c.play());
  handle(IPC.setSettings, (_e, patch) => {
    const p = settingsPatch(patch);
    return p === null ? c.setSettings({}) : c.setSettings(p);
  });
  handle(IPC.setStatusPolling, (_e, on) => {
    if (typeof on === "boolean") c.setStatusPolling(on);
  });
  handle(IPC.refreshStatus, () => c.refreshStatus());
  handle(IPC.getNews, () => c.getNews());
  handle(IPC.getBranding, () => c.getBranding());
  handle(IPC.openExternal, (_e, target) => {
    const t = externalTarget(target);
    return t === null ? { ok: false, error: { code: "unknown" } } : c.openExternal(t);
  });
  handle(IPC.openGameFolder, () => c.openGameFolder());
  handle(IPC.openLogsFolder, () => c.openLogsFolder());
  handle(IPC.logout, () => c.logout());
  handle(IPC.installUpdate, () => c.installUpdate());
  // The page asks when it opens, after an action and on "Check again": the requirements are looked at afresh.
  handle(IPC.hostGet, () => c.hostKit({ fresh: true }));
  handle(IPC.hostChooseWorkspace, () => c.hostChooseWorkspace());
  handle(IPC.hostChooseDumper, () => c.hostChooseDumper());
  handle(IPC.hostImportGameData, () => c.hostImportGameData());
  handle(IPC.hostImportSdkFolder, () => c.hostImportSdkFolder());
  handle(IPC.hostDownloadProject, () => c.ensureHostWorkspace());
  handle(IPC.hostAction, (_e, action) => {
    const a = hostAction(action);
    return a === null ? { ok: false, error: { code: "unknown" } } : c.hostAction(a);
  });
  handle(IPC.hostOpen, (_e, folder) => {
    const f = hostFolder(folder);
    return f === null ? { ok: false, error: { code: "unknown" } } : c.hostOpen(f);
  });
  handle(IPC.hostServerGet, () => c.hostServerView());
  handle(IPC.hostServerStart, () => c.hostServerStart());
  handle(IPC.hostServerStop, () => c.hostServerStop());
  handle(IPC.hostSetup, (_e, req) => {
    const r = hostSetupRequest(req);
    return r === null ? { ok: false, error: { code: "host_failed", detail: "setup_form" } } : c.hostSetup(r);
  });
  handle(IPC.hostUpdate, () => c.hostUpdate());
  handle(IPC.hostSetMultipliers, (_e, m) => {
    const v = hostMultipliers(m);
    return v === undefined ? { ok: false, error: { code: "host_failed", detail: "multipliers_form" } } : c.hostSetMultipliers(v);
  });
  handle(IPC.hostCreateInvite, (_e, forName) => {
    const f = inviteFor(forName);
    return f === undefined ? { ok: false, error: { code: "unknown" } } : c.hostCreateInvite(f);
  });
  // The page may not use the clipboard itself (every permission is refused): an invite link is copied here.
  handle(IPC.copyText, (_e, text) => {
    const t = boundedString(text, 4096);
    if (t !== null) clipboard.writeText(t);
    return t !== null;
  });
  handle(IPC.dismissNotice, () => c.dismissNotice());
  handle(IPC.dismissError, () => c.dismissError());
  handle(IPC.windowMinimize, () => mainWindow?.minimize());
  handle(IPC.windowMaximize, () => {
    if (!mainWindow) return false;
    if (mainWindow.isMaximized()) mainWindow.unmaximize();
    else mainWindow.maximize();
    return mainWindow.isMaximized();
  });
  handle(IPC.windowClose, () => mainWindow?.close());
  handle(IPC.takeInviteLink, () => {
    const link = pendingInviteLink;
    pendingInviteLink = null;
    return link;
  });
}

// ------------------------------------------------------------------ platform for the controller

function makePlatform(): Platform {
  const send = (channel: string, payload: Snapshot | TaskProgress | HostKit | HostServerView) => {
    if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(channel, payload);
  };
  const relayPort = relayPortOverride(process.env.REFORGED_RELAY_PORT);
  if (relayPort !== undefined) log.warn(`relay port overridden to ${relayPort}`);
  const lang = () => controller?.snapshot().settings.language ?? "en";
  return {
    userDataDir: app.getPath("userData"),
    documentsDir: app.getPath("documents"),
    resourcesDir: resourcesDir(),
    hostPlatform: process.platform,
    env: process.env,
    appVersion: app.getVersion(),
    packaged: app.isPackaged,
    defaultLanguage: systemLanguage(app.getLocale()),
    systemLocale: app.getLocale(),
    relayPort,
    encryptor: {
      isAvailable: () => safeStorage.isEncryptionAvailable(),
      encrypt: (plain) => safeStorage.encryptString(plain),
      decrypt: (data) => safeStorage.decryptString(data),
    },
    chooseFolder: async (purpose, defaultPath) => {
      if (!mainWindow) return null;
      const title = purpose === "game" ? "dialog_game_folder" : purpose === "runtime" ? "dialog_runtime_folder" : purpose === "sdk" ? "dialog_sdk_folder" : purpose === "revived" ? "dialog_revived_folder" : "dialog_workspace_folder";
      const r = await dialog.showOpenDialog(mainWindow, {
        title: translate(lang(), title),
        defaultPath: defaultPath && existsSync(defaultPath) ? defaultPath : undefined,
        properties: ["openDirectory", "dontAddToRecent"],
      });
      return r.canceled || r.filePaths.length !== 1 ? null : r.filePaths[0];
    },
    chooseDll: async (defaultPath) => {
      if (!mainWindow) return null;
      const r = await dialog.showOpenDialog(mainWindow, {
        title: translate(lang(), "dialog_dumper_dll"),
        defaultPath: defaultPath && existsSync(defaultPath) ? defaultPath : undefined,
        filters: [{ name: "DLL", extensions: ["dll"] }],
        properties: ["openFile", "dontAddToRecent"],
      });
      return r.canceled || r.filePaths.length !== 1 ? null : r.filePaths[0];
    },
    chooseSaveFile: async (defaultName) => {
      if (!mainWindow) return null;
      const r = await dialog.showSaveDialog(mainWindow, {
        defaultPath: path.join(app.getPath("documents"), defaultName),
        filters: [{ name: "Text", extensions: ["txt"] }],
        properties: ["dontAddToRecent", "showOverwriteConfirmation"],
      });
      return r.canceled || !r.filePath ? null : r.filePath;
    },
    chooseOpenFile: async () => {
      if (!mainWindow) return null;
      const r = await dialog.showOpenDialog(mainWindow, {
        filters: [{ name: "Text", extensions: ["txt"] }],
        properties: ["openFile", "dontAddToRecent"],
      });
      return r.canceled || r.filePaths.length !== 1 ? null : r.filePaths[0];
    },
    choosePack: async () => {
      if (!mainWindow) return null;
      const r = await dialog.showOpenDialog(mainWindow, {
        title: translate(lang(), "dialog_gamedata_pack"),
        filters: [{ name: "Game data pack", extensions: ["gz"] }],
        properties: ["openFile", "dontAddToRecent"],
      });
      return r.canceled || r.filePaths.length !== 1 ? null : r.filePaths[0];
    },
    openExternal: (url) => shell.openExternal(url),
    openPath: async (dir) => {
      await shell.openPath(dir);
    },
    emitSnapshot: (s) => send(IPC.snapshot, s),
    emitProgress: (p) => send(IPC.progress, p),
    emitHostKit: (k) => send(IPC.hostKit, k),
    emitHostServer: (v) => send(IPC.hostServer, v),
    findTailscale: () => findTailscale(process.env, existsSync, process.platform),
    findRunningClients: () => findRunningClients(process.platform),
    inject: (opts) => injectDll(opts),
    installUpdate: () => {
      if (app.isPackaged && process.platform === "win32") {
        quitting = true;
        autoUpdater.quitAndInstall();
      }
    },
  };
}

// ------------------------------------------------------------------ startup

if (!started) {
  const gotLock = app.requestSingleInstanceLock();
  if (!gotLock) {
    app.quit();
  } else {
    pendingInviteLink = inviteFromArgv(process.argv);

    app.on("second-instance", (_event, argv) => {
      if (mainWindow) {
        if (mainWindow.isMinimized()) mainWindow.restore();
        mainWindow.focus();
      }
      deliverInviteLink(inviteFromArgv(argv));
    });

    app
      .whenReady()
      .then(async () => {
        Menu.setApplicationMenu(null);
        logToFile(path.join(app.getPath("userData"), "logs"));
        log.info(`Dauntless Reforged Revival Launcher ${app.getVersion()} starting`);
        registerProtocolClient();
        hardenSessions();
        controller = new Controller(makePlatform());
        registerIpc(controller);
        createWindow();
        await controller.init();

        if (app.isPackaged && process.platform === "win32") {
          updateElectronApp({
            updateSource: { type: UpdateSourceType.StaticStorage, baseUrl: UPDATE_FEED_URL },
            updateInterval: "1 hour",
            notifyUser: true,
            onNotifyUser: () => controller?.setUpdateReady(),
            logger: { log: (m) => log.info(`update: ${m}`), info: (m) => log.info(`update: ${m}`), warn: (m) => log.warn(`update: ${m}`), error: (m) => log.error(`update: ${m}`) },
          });
        }
      })
      .catch((error) => {
        log.error(`launcher startup failed: ${describeError(error)}`);
        dialog.showErrorBox("Dauntless Reforged Revival could not open", `Startup failed. Send the launcher logs to the host.\n${describeError(error)}`);
        app.exit(1);
      });

    app.on("before-quit", () => {
      quitting = true;
    });

    app.on("window-all-closed", () => {
      const c = controller;
      controller = null;
      void (c ? c.shutdown() : Promise.resolve()).finally(() => app.quit());
    });
  }
}
