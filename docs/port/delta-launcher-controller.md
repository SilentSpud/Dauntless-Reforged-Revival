<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Delta: launcher controller, bridge and page

Row RX1 of [`MIGRATION_PLAN.md`](../../MIGRATION_PLAN.md), the launcher controller, bridge and page family. Written
2026-10-22. Read-only work: nothing here changes code.

Each section compares one Reforged file with its counterpart in Dauntless Revived's Node launcher at `e1c5fec`
(`legacy/UndauntedLauncher` there), then names the Rust module of DR's Tauri launcher
(`legacy/UndauntedLauncher/src-tauri` there) or the `protocol` crate (`rust/crates/protocol`) the behavior lands in.
The rows that port it are RS1 (protocol), RA1 (copy of `src-tauri`), RA2 (bridge), RA3 (languages) and RA13
(controller).

- Paths are as on this branch: `ReforgedLauncher/…`. RR2 moves the folder to `legacy/ReforgedLauncher/` in parallel;
  the paths under it do not change.
- "Changed lines" counts the `<` and `>` lines of a plain `diff` between the two Node files: a rough size, not a
  measure of work.
- Line ranges are Reforged's (`ReforgedLauncher/…`) unless they say DR.
- DR Rust paths such as `controller/mod.rs` are under `legacy/UndauntedLauncher/src-tauri/src/` at `e1c5fec`;
  `isolation/` is `legacy/UndauntedLauncher/isolation/`, beside `src-tauri`.
- "DR only" lists what DR's file has and Reforged's does not. The Rust copy brings it along, so RA1 or RA13 has to
  remove it, or keep it on purpose (see Open points).
- Behavior that belongs to the files the sibling RX1 agent covers (`hostapi.ts`, `relay.ts`, `runtime-files.ts`,
  `launch.ts`, `keystore.ts`, `settings.ts`, …) is named only where the controller calls it differently.

## Summary

| Reforged file | DR Node file (`e1c5fec`) | DR Rust module | Changed lines | Row |
| --- | --- | --- | --- | --- |
| `src/main/controller.ts` | `src/main/controller.ts` | `controller/mod.rs`, `controller/platform.rs`, `controller/stored.rs`; not `controller/offline.rs` | 2,241 (+1,438 −803) | RA13 |
| `src/main/ipc-validate.ts` | `src/main/ipc-validate.ts` | `bridge/spec.rs`, `bridge/commands.rs`, `isolation/commands.js` | 133 (+115 −18) | RA2 |
| `src/preload.ts`, `src/renderer/global.d.ts` | `src/preload.ts`, `src/renderer/api.ts`, `src/renderer/backend.ts` | `bridge/spec.rs`, `bridge/commands.rs`, `bridge/generate.rs`, `isolation/hook.js` | 69 (+47 −22); `global.d.ts` new | RA2 |
| `src/main.ts` | `src/main.ts` | `lib.rs`, `window.rs`, `controller/platform.rs` (`TauriPlatform`), `platform/dialogs.rs`, `platform/invite_link.rs`, `platform/art.rs` | 503 (+151 −352) | RA1, RA2, RA11 |
| `src/shared/types.ts` | `src/shared/types.ts` | `bridge/types.rs` (TS made from it by `bridge/generate.rs`) | 464 (+321 −143) | RA2 |
| `src/shared/invite.ts` | `src/shared/invite.ts` | `protocol::invite` (`rust/crates/protocol/src/invite.rs`), launcher `core/invite.rs` | 32 (+22 −10) | RS1, RA4 |
| `src/shared/status.ts` | `src/shared/status.ts` | `protocol::status`, launcher `core/status.rs` | 33 (+28 −5) | RS1, RA4 |
| `src/shared/i18n.ts` | `src/shared/i18n/index.ts`, `en.ts`, `pt.ts` (+ 7 more languages) | `core/i18n.rs`, `core/text.rs`, `build.rs` (`gen/strings.json`) | key-level, see section 8 | RA3 |
| `src/shared/username.ts`, `format.ts`, `ui-model.ts`, `credits.ts`, `text.ts` | same names | `core/username.rs`, `core/text.rs`; the rest stays TS | 8, 50, 114, 97, 0 | RA3, RA13 |
| `src/renderer/index.ts`, `dom.ts`, `scene.ts`, `styles.css`, `modern.css`, `index.html` | same names; `dauntless-style.css` | none: the page stays TS (RR5 moves it to `rust/launcher/ui`) | 2,048; 19; 26; 1,535; new; 52 | RA2, RA13 |

The changed-line counts include DR's own work since the two launchers split (offline play, Linux, partners, mods,
nine languages), so most of a large count is DR code Reforged never had, not Reforged code to carry.

## 1. `src/main/controller.ts`

- Reforged: `ReforgedLauncher/src/main/controller.ts` (2,385 lines).
- DR Node: `legacy/UndauntedLauncher/src/main/controller.ts` (1,750 lines).
- DR Rust: `src-tauri/src/controller/mod.rs` (the state machine, one mutex never held across an await),
  `controller/platform.rs` (the `Platform` trait and `TauriPlatform`), `controller/stored.rs` (the settings the
  controller reads), `controller/tests.rs`. `controller/offline.rs` and `controller/tests/offline_play.rs` are not
  brought in (RA1).
- Changed lines: 2,241 (+1,438 −803).
- Row: **RA13**. The Host page part (lines 1765–2351) is RA12's backend called from RA13's controller; RA2 has to
  register its commands first.

Reforged's controller is DR's with the 1.4.4 install path (manifest download, pause and resume, client DLLs, mods,
hunt region, device signals, offline play, Linux compatibility runtime) taken out and four things added: the game
folder (find and check an existing 1.14.7), the server's game runtime (accept once, install, check before every
launch), the Host page, and a second code path for DR's own 1.4.4 servers that RD5 drops again.

### 1.1 Mapping to DR's Rust controller

| Reforged (lines) | DR Rust `controller/mod.rs` | Port |
| --- | --- | --- |
| `Platform` (115–163) | `controller/platform.rs` `Platform` trait | Add the Reforged members (1.3); drop `prepare_game_launch`'s 1.4.4 use, `collect_device_signals`, `emit_offline_status`, `choose_offline_save` |
| fields, constructor (173–259) | `Controller` state, `new` (250), `game_changed` (292) | 1.2: game exit handling, hash cache, Host server |
| `init` (313–323) | `init` (399) | 1.2 |
| `computePhase` (325–336) | `compute_phase` (412) | 1.2: `locate`/`runtime` replace `install`/`update` |
| `currentSettings`, `snapshot` (338–393) | `settings_of` (441), `snapshot` (454) | New snapshot shape (section 5) |
| `checkInvite`, `movedServer`, `carryAccount`, `certificateChange` (445–486) | `check_invite` (607), `certificate_change` (622) | 1.2: moved private server |
| `submitInvite`, `switchServer`, `removeSavedServer`, `join` (491–551) | `submit_invite` (638), `switch_server` (659), `remove_saved_server` (673), `join` (690) | 1.2 |
| `resetServerState`, `forgetServer` (553–578) | `reset_server` (166), `forget_server` (741) | 1.2 |
| `connect` (580–618) | `connect` (755), `connect_now` (774) | 1.2: runtime offer instead of content probe |
| `rememberServerName`, `refreshKeyFlag`, `statusKey`, `fetchStatus`, `checkAccount` (620–678) | 845–916 | `flavor()` goes with RD5 |
| `setStatusPolling`, `pollStatus`, `refreshStatus` (682–735) | `set_status_polling` (917), `poll_status` (943) | 1.2: runtime offer re-read |
| `refreshNews`, `loadExtras`, `getNews`, `getBranding`, `resolveArt` (737–790) | `in_background` (1015), `refresh_news` (1028), `load_extras` (1060), `news`, `branding` (1103–1107); art in `platform/art.rs` | 1.2: gateway endpoint, generation guard, `rr-art` |
| account: `register` … `logout` (794–943) | `register` (1116) … `logout` (1289) | Unchanged but for names (1.2) |
| game folder: `inspectGame`, `verifyGame`, `chooseGameFolder`, `useGamePath`, `useDetectedGame`, `openGameFolder` (949–1077) | replaces `inspect_install` (1319), `choose_install_dir` (1336), `use_existing_game_folder` (1403), `use_existing_game_path` (1421); `open_game_folder` (1450) kept | New module (Open points) |
| runtime: `trustedSet` … `clearLocalRuntime` (1081–1226) | replaces `start_install`, `repair`, `download_job`, `run_task`, `run_task_now`, `download` (1459–1822) | New module |
| `cancelTask` (1228–1231) | `cancel_task` (1488); `pause_task`, `resume_task` (1476, 1482) dropped | — |
| 1.4.4: `revivedManifest` … `playRevived` (1233–1586) | DR's own 1.4.4 path | **Not ported (RD5)** |
| `play` (1590–1682) | `play` (1823), `play_now` (1845), `check_game_launch_environment` (1910), `prepare_game_install` (1922), `launch_game` (1963) | 1.2: the largest behavioral change after the runtime |
| `startRelay`, `stopRelay` (1684–1716) | `start_relay` (2039), `stop_relay` (2093) | 1.2 |
| `setSettings` (1720–1740) | `set_settings` (2106) | 1.2 |
| `openExternal`, `openLogsFolder` (1742–1763) | `open_external` (2138); logs folder new | — |
| Host page (1765–2351) | none | New module, backed by RA12 |
| `shutdown` (2353–2360) | `shutdown` (2161) | 1.2 |
| `lastJson`, `updateFailure`, `NEEDS_UPSTREAM` (2366–2385) | none | With the Host module |

### 1.2 Behavior changes

Carry over (Reforged behavior the Rust copy does not have):

- **Header and transport (1–10).** Private mode is plain HTTP to the host's gateway over Tailscale, public mode TLS
  pinned to `fp`, as in DR. Unlike DR, the local relay on `127.0.0.1:61100` runs in **both** modes: the runtime
  reaches it through `REFORGED_BACKEND_URL`. DR's comments and `start_relay` assume public mode only.
- **Game exit (242–258).** The exit code of a game started for Dumper-7 is never a failed launch (`dumping`), and
  neither is `STATUS_CONTROL_C_EXIT` (0xC000013A). After exit the loader set aside for Dumper-7 is put back
  (`restoreAfterDump`, 2326–2335), the relay stops and the runtime is inspected again. DR only stops the relay.
- **Stores and caches (230–241).** The key backup folder is `Documents\Dauntless Reforged Revival\Account Keys`;
  the executable hash cache is `userData\file-hashes.json` (`HashCache`, DR has `VerifiedCache` over every 1.4.4
  file); a `HostServer` is created with the controller and pushes `emitHostServer` on every change.
- **`init` (313–323).** Reads the key flag, inspects the game folder with store detection, inspects the runtime,
  connects, hashes an `unverified` executable once, and starts `ensureHostWorkspace({ auto: true })`. DR recovers an
  offline save and inspects the install.
- **Phase (325–336).** `join` → `installing` (a task) → `running` → `connect` → `register` → `locate` (game folder
  problem) → `runtime` (runtime not playable) → `ready`. Line 332 is the 1.4.4 branch (dropped).
- **Snapshot (351–393).** Adds `game`, `runtime`, `running.{game,relayPort}`, `app.platform`; `savedServers[].game`
  and `server.game` and `revived` exist only for 1.4.4 (dropped with RD5). Section 5 has the type changes.
- **Moved private server (445–480, 528).** `checkInvite` answers `movedFrom`: a private invite for a host:port with no
  key, whose server name and port match a server this PC has a key for at another address. `submitInvite` with
  `carryAccount` copies that key to the new address, carries the username over and removes the old address from
  the saved servers. Only after the player confirmed (`server_moved` modal on the page).
- **Join (518–551).** `same` also compares `game` (1.4.4 only); the log line names the edition (1.4.4 only).
- **Reset and forget (553–578).** `resetServerState` also drops the runtime manifest; `forgetServer` inspects the
  runtime again before publishing.
- **Connect (580–618).** After a good answer: `checkAccount`, then `refreshRuntimeOffer` (fetches the runtime
  manifest, a `cert_mismatch` there flips the connection), then `loadExtras` without waiting. DR probes the content
  server in public mode (`probeContent`), inspects the 1.4.4 install and queues extras through `in_background`.
- **Status and account calls (642–678).** Pass `this.flavor()` to pick `/reforged/api` or `/undaunted/api`; with RD5
  the flavor goes and only the Reforged paths stay (sibling's `hostapi.ts` delta).
- **Polling (693–731).** On a good poll with no runtime offer yet (`unavailable` or no manifest), not playing and no
  task, the runtime offer is fetched again (711): a host can publish a runtime while the player waits.
- **News and art (737–790).** Fetched from the server's own endpoint (the gateway), not a content endpoint. The art
  pack load drops its result when the server changed meanwhile (`generation`, 759 and 773); DR's has no such guard.
  Art ids are served as `rr-art://bg/<sha256>` (771, 786–790). DR wraps both in `in_background` so `shutdown` can
  wait for them; Reforged fires them with `void`. Keep DR's `in_background` and add the guard.
- **Account (794–943).** Same flow; `register` and `adoptKey` no longer inspect a 1.4.4 install; the backup file is
  `Dauntless Reforged Revival key - <name>.txt` (894).
- **Game folder (949–1077), new.** `inspectGame` takes, when nothing is chosen, an install the stores report
  (Legendary, Heroic, Epic), the 1.14.7 one first, and stores it as `gameRoot`; then classifies it as `not_set`,
  `not_found`, `unverified` (no cached hash), `wrong_version` (cached hash not the pin, or `Version.txt` names another
  build) or ok. `verifyGame` hashes `Dauntless-Win64-Shipping.exe` (~120 MB) as a `verify` task with progress every
  150 ms, cancellable, and sets `game_verified`. `useGamePath` accepts the Dauntless folder, its parent, `Archon` or
  `Win64` and verifies at once when unverified. `useDetectedGame` picks from `game.detected` by index.
- **Runtime (1081–1226), new.** A runtime set is accepted per server: `trustedRuntimes[serverId]` holds
  `runtimeSetId(files)` (1081–1086, 1117–1129). `installServerRuntime` refuses an untrusted set, downloads with the
  account key into `Win64`, reports `runtime` progress and maps `RuntimeError` codes to `ErrorCode`s (1182–1191);
  replaced-while-in-use files on a host PC whose server runs give `runtime_installed_restart` (1169–1170). A local
  runtime folder (`chooseLocalRuntime`, 1206–1217) must hold both runtime files and is used instead of the server's
  until `clearLocalRuntime`. This is plan section 6's "runtime acceptance per server".
- **Play (1590–1682).** Order: busy and running checks; missing Visual C++ files are **logged only** (1598–1599; DR
  refuses with `windows_runtime_missing`, and also checks DirectX); another client running → `already_running`;
  key and `GetUserInfo`; the executable hashed on every launch against the 1.14.7 pin (1615–1629); a local runtime
  copied in (1632–1639); runtime inspected and must be playable (1640–1643); relay started; game config written with
  host `127.0.0.1` and the relay port, `clearPhoenixEndpoints` removing DR launcher's `[OnlineSubsystemPhoenix]`
  overrides from Game.ini (1652–1656); launch arguments with key, user id and username (1661–1667); environment
  with `REFORGED_BACKEND_URL=http://127.0.0.1:<port>` and the runtime profile (1668); start in `Win64`.
- **Relay (1684–1716).** Starts for private servers too (public needs `fp`), adds the QoS listener on port 80
  (`qosPort`, 1693) and logs QoS answers. **DR's relay owner check is missing** (DR 1618–1626, Rust `start_relay`
  2051–2058: `createRelayOwnerCheck`, only the game's PID may connect, Windows only); see Open points.
- **Settings (1720–1740).** Accepts `graphics`, `windowed`, `streamerMode`, `hostTools`, `language`, `gameLanguage`,
  `runtimeProfile`. Turning `hostTools` on starts `ensureHostWorkspace({ auto: true })`.
- **Links (1742–1763).** The same `openExternal` switch with Reforged's targets (section 5); `openLogsFolder` opens
  `userData\logs`.
- **Host page (1765–2351), new.** `hostKit` (detects the workspace, toolchain, requirements with a 15 s cache, the
  game's `Items_Analysis`); `hostServerView/Start/Stop`; `hostSetup` (`npm run setup` with mode, name, public host
  and the launcher's game folder); `hostCreateInvite`; `whileStopped` (stop a server this launcher runs for a change,
  start it again after; refuse one started elsewhere); `hostSetMultipliers` (private only); `hostUpdate` with
  `--check --json` first and `legacyUpdate` (git fetch and fast-forward for projects without `selfupdate.ts`);
  `generateGameData`, `exportGameData`; `ensureHostWorkspace` (find or `git clone` the project beside the game folder,
  never into a non-empty folder); workspace and Dumper-7 DLL pickers; game data pack import; `hostOpen` folders;
  jobs and notices (`setJob`, `finishJob`); `ensureUpstream` (`npm run upstream`); SDK import; `hostAction`'s eleven
  actions; the Dumper-7 session (`startDump`: loader renamed to `winmm.dll.rr-off`, game started with `-NoEAC
  -EpicPortal -DisableEOS` and no `REFORGED_`/`MYSTICPARADOX_` variables, DLL injected by PID); CatalogExporter
  injection into the running game with `export_flags.txt`. RH removes `npm`, `upstream` and the git update; RA12
  rewires these calls to `reforged-host` (RD3).
- **Shutdown (2353–2360).** Stops the Host page's server first, then polling, the task, the relay, and saves the hash
  cache. DR's also waits for the background extras (`extrasTask`) and refuses `play` while shutting down
  (`shuttingDown`); keep both.

Not carried over (RD5, 1.4.4 for DR's servers): `isRevived`, `flavor` (299–306), the 1.4.4 branch in
`computePhase` (332), `revivedInfo` and friends (213–219, 235), `inspectRevived`, `chooseRevivedFolder`,
`useRevivedPath`, `useDetectedRevived`, `useRevivedRoot`, `revivedContentEndpoint`, `installRevived`, `playRevived`
(1233–1586), the `endPlayingSession` hook (it only ends a 1.4.4 session), and `cancelTask`'s `revivedJob` (1230).

DR only (the Rust copy has these; RA1 or RA13 removes them unless an Open point keeps them): offline play
(`offlineKeys`, `offlineSession`, `startOffline`, `stopOffline`, save import and export; DR 126–150, 732–772,
1212–1436, 1726–1750); the install folder and 1.4.4 download, verify, repair, pause and resume (DR 928–1211); client
mods (`installMods`); device signals before play (`checkDeviceSignals`, DR 1457–1458); hunt region choice and
`saveHuntRegion` (DR 1460–1466); `endPlaying` on game exit; exposure and `showConsole` settings with
`DR_SHOW_CONSOLE`; the Linux compatibility runtime (`prepareGameLaunch`, `compat_runtime_missing`, DR 1553–1565),
which RA10 re-fits rather than removes; the relay owner check, which should stay.

### 1.3 `Platform` members

| Member | DR | Reforged | Port |
| --- | --- | --- | --- |
| `chooseFolder(purpose, default)` | install or existing | `game`, `runtime`, `workspace`, `sdk`, `revived` (113) | Rust `choose_folder` takes a purpose; `revived` dropped |
| `chooseDll`, `choosePack` | — | new (145, 149) | New dialogs in `platform/dialogs.rs` |
| `chooseSaveFile`, `chooseOpenFile`, `openExternal`, `openPath`, `emitSnapshot`, `emitProgress`, `findTailscale`, `findRunningClients` | yes | yes | Kept |
| `emitHostKit`, `emitHostServer` | — | new (154–155) | Two new events (section 11) |
| `inject(opts)` | — | new (161) | RA12's injector (RD9) |
| `installUpdate` | yes | yes | Tauri updater (RA14) |
| `hostServer`, `exePin`, `dumpRoot`, `projectGitUrl`, `qosPort`, `revived*`, `runtimeFilesExist` | — | test seams (130–143, 157) | `revived*` dropped; the rest as test seams |
| `prepareGameLaunch`, `collectDeviceSignals`, `offlineSave`, `missingRuntimes` | yes | — | Device signals and offline dropped; Linux launch kept for RA10 |

## 2. `src/main/ipc-validate.ts`

- Reforged: `ReforgedLauncher/src/main/ipc-validate.ts` (170 lines). DR Node: `src/main/ipc-validate.ts` (73).
- DR Rust: `bridge/spec.rs` (limits, `SETTINGS_KEYS`, the command list), `bridge/commands.rs` (each command checks
  again), `isolation/commands.js` (generated from the spec, read by `isolation/hook.js`).
- Changed lines: 133 (+115 −18). Row: **RA2**.

Behavior changes:

- **Settings patch (26–40).** Keys `graphics`, `windowed`, `streamerMode`, `hostTools`, `language`, `gameLanguage`,
  `runtimeProfile`. DR's `exposure`, `showConsole`, `huntRegion` go. `SETTINGS_KEYS` in `spec.rs` changes to match;
  `language` values are `en` and `pt-BR` (not DR's `pt`).
- **Link names (44–77).** Reforged's 24 targets (section 5) replace DR's 18; `pick` (71–73) is a shared lookup.
- **Host page arguments (79–149), new.** `hostAction`: 11 names; `hostFolder`: 7 names; `plainText`: trimmed, length
  limit, no control characters; `hostSetupRequest`: name ≤ 60, mode `private`/`public`, public host a DNS name or
  IPv4 of at most 253 characters; `hostMultipliers`: `null` (off) or `xp` and `rewards` finite numbers 1–100, rounded
  to two decimals, 1/1 meaning off, anything else refused (`undefined`); `inviteFor`: optional, ≤ 40.
- **Relay port override (151–156).** The default named in the comment is 61100 (DR 61000); the logic is the same. The
  variable is `REFORGED_RELAY_PORT` (read in `main.ts` 363).
- **`isTrustedPageUrl` (158–170).** Unchanged; in Tauri the isolation pattern replaces it.

The spec's argument kinds (`Text`, `OptionalText`, `Flag`, `SettingsPatch`, `Channel`) cannot express four of these:
an index (`useDetectedGame`), a name from a fixed list (`hostAction`, `hostFolder`; DR passes link names as `Text`),
the setup form and the multipliers object. See Open points.

## 3. `src/preload.ts` and `src/renderer/global.d.ts`

- Reforged: `ReforgedLauncher/src/preload.ts` (118 lines), `src/renderer/global.d.ts` (13, new).
- DR Node: `src/preload.ts` (93); the Tauri page uses `src/renderer/api.ts` (same `LauncherApi`, over `invoke`, with
  deadlines) chosen by `src/renderer/backend.ts`.
- DR Rust: `bridge/spec.rs`, `bridge/commands.rs`, `bridge/generate.rs` (TS types), `isolation/hook.js`.
- Changed lines: 69 (+47 −22). Row: **RA2**.

Behavior changes:

- **`submitInvite` (30–31)** gains a third argument, `carryAccount`.
- **Game folder (41–44)** replaces DR's install folder calls: `chooseGameFolder`, `useGamePath`, `useDetectedGame`,
  `verifyGame`.
- **1.4.4 (45–47):** `revivedInstall`, `revivedChooseFolder`, `revivedUseDetected`. Dropped (RD5).
- **Runtime (48–51), new:** `trustRuntime`, `installRuntime`, `chooseLocalRuntime`, `clearLocalRuntime`.
- **Removed from DR's list:** `pauseTask`, `resumeTask`, `repair`, `startInstall`, `startOffline`, `stopOffline`,
  `offlineStatus`, `exportOfflineSave`, `importOfflineSave`, `onOfflineStatus`.
- **`setSettings` (54–63)** forwards Reforged's seven keys only.
- **`openLogsFolder` (70), Host page (73–88), `copyText` (89), `onHostKit`, `onHostServer` (102–107)** are new.
- **`global.d.ts`** declares `window.launcher` for the page, which uses it directly (`renderer/index.ts` 51). DR's page
  imports `api` and `artUrl` from `backend.ts` instead, so the same page runs in Electron and Tauri. RA2 brings
  `api.ts` and `backend.ts` over with Reforged's calls; RA13 switches the page to them.

## 4. `src/main.ts`

- Reforged: `ReforgedLauncher/src/main.ts` (504 lines). DR Node: `src/main.ts` (705).
- DR Rust: `lib.rs` (builder, `generate_handler!`, art scheme, shutdown on last window), `window.rs` (close
  confirmation, window state), `controller/platform.rs` (`TauriPlatform`), `platform/dialogs.rs`,
  `platform/invite_link.rs` (scheme registration and single instance), `platform/art.rs`, `platform/updater.rs`.
- Changed lines: 503 (+151 −352). Rows: **RA1** (app identity), **RA2** (handlers), **RA11** (links, dialogs, window).

Behavior changes:

- **Identity (1–35).** App name, `APP_ID`, `SQUIRREL_NAME`, the `rr-art` scheme (DR `dr-art`, `platform/art.rs`
  `SCHEME`). No Linux keyring repair (DR 48–56), no Electron-to-Tauri bridge download (DR 155–200, L50), no offline
  stack (DR 86–123).
- **CSP (42–44).** `img-src 'self' rr-art: data:`. Tauri's CSP in `tauri.conf.json` has to name the art scheme as
  WebView2 serves it (`http://rr-art.localhost`).
- **Invite links (57–78).** Only `reforged-revival:` arguments are taken (`INVITE_SCHEME`); a `dauntless-revived:` link
  never reaches the page. The Squirrel stub path (74–76) goes with Squirrel.
- **Window (82–163).** 1280×760 capped to the work area, minimum 1024×640 capped, background `#03080F`, title
  "Dauntless Reforged Revival Launcher". Renderer crash restarts once in `--safe-mode` unless the relay is active.
  The close confirmation only while the relay runs (DR also while offline play runs: `close_text_offline`), keys
  `app_title`, `close_title`, `close_text`, `close_keep`, `close_anyway`.
- **Sessions (165–212).** The same hardening; only the art scheme differs.
- **IPC (216–355).** The handler list is section 11. Validation in the handler, kept in `commands.rs`:
  `useDetectedGame` index an integer 0–15 (274); `revivedInstall` mode (276, dropped); `hostSetup` refusal detail
  `setup_form` (323); `hostSetMultipliers` refusal detail `multipliers_form` (328); `copyText` ≤ 4096, returns a
  boolean (335–339; the page has no clipboard permission). `hostGet` always asks for fresh requirements (304).
- **Platform (359–446).** Dialog titles by purpose from the language file: `dialog_game_folder`,
  `dialog_runtime_folder`, `dialog_sdk_folder`, `dialog_revived_folder` (dropped), `dialog_workspace_folder`,
  `dialog_dumper_dll` (DLL filter), `dialog_gamedata_pack` (`.gz` filter). DR's backend text has only
  `dialog_install_folder` and `dialog_existing_folder` (`core/i18n.rs`). `inject` calls `injectDll`.
- **Startup (450–504).** Single instance, invite from argv, `logToFile`, the update feed from `UPDATE_FEED_URL` every
  hour (Squirrel; RA14 replaces it), `window-all-closed` waits for `shutdown`.

## 5. `src/shared/types.ts`

- Reforged: `ReforgedLauncher/src/shared/types.ts` (482 lines). DR Node: `src/shared/types.ts` (304).
- DR Rust: `bridge/types.rs` (every type that crosses the bridge, with `ts-rs` derives); `bridge/generate.rs` writes
  `src/shared/generated/types.ts` from it (`CommandName` too).
- Changed lines: 464 (+321 −143). Row: **RA2** (types), RA13 (their use).

| Type (Reforged lines) | Change against DR |
| --- | --- |
| `Language`, `LANGUAGES` (10–11) | `"en" \| "pt-BR"`, a literal list. DR derives nine from the `i18n` registry, with Portuguese as `pt`. A stored `pt-BR` must keep reading back (section 6 of the plan: both languages word for word) |
| `DEFAULT_GRAPHICS` (16) | `-1` (leave graphics to the game). DR `4` |
| `ExposureMode`, `EXPOSURE_MODES` | DR only; dropped |
| `RuntimeProfile`, `RUNTIME_PROFILES` (22–25) | New: `production`, `development` (`MYSTICPARADOX_RUNTIME_PROFILE`) |
| `Phase` (27–36) | `locate` and `runtime` replace `install` and `update` |
| `ConnectProblem` (38–43) | Same values; `bad_answer` means "not a Reforged server" |
| `GameProblem` (46–51), `RuntimeState` (54–60) | New |
| `ErrorCode` (62–107) | New: `game_wrong_version`, `runtime_unavailable`, `runtime_untrusted`, `runtime_failed`, `runtime_different_on_server`, `runtime_in_use`, `local_runtime_invalid`, `host_failed`, `folder_other_game`. 1.4.4 only in Reforged (dropped with RD5): `manifest_mismatch`, `no_content_server`, `file_missing_on_server`, `file_different_on_server`, `dll_failed`, `game_files_invalid`, `windows_runtime_missing`, `folder_other_game`, `folder_not_empty`. DR only: `manifest_unavailable`, `verify_failed`, `compat_runtime_missing` (keep for RA10), `offline_save_invalid`, `offline_unavailable`, `offline_insufficient_memory`. RS1 adds the refusal for a `dauntless-revived://` invite (RA3 its text) |
| `NoticeCode` (110) | New: `runtime_installed`, `runtime_installed_restart`, `game_verified`, `sdk_imported`, `host_done`. `install_done`, `repair_done`: 1.4.4 only |
| `OfflinePhase`, `OfflineStatus` | DR only; dropped |
| `InviteCheck` (122–125) | `ok: true` gains `movedFrom: string \| null` |
| `TaskKind`, `TaskProgress` (128–139) | Kinds `verify`, `runtime` (`download`, `dlls` 1.4.4 only); DR's `repair` and `paused` go |
| `ServerInfo`, `SavedServerInfo` (141–163) | `game: GameEdition` added; 1.4.4 only, dropped with RD5 |
| `GameSource`, `DetectedGame`, `GameInfo`, `RuntimeFileInfo`, `RuntimeInfo` (165–205) | New. `GameSource` `revived` and `RevivedInfo` (188–198) are 1.4.4 only |
| `Settings` (207–215) | `graphics`, `windowed`, `language`, `gameLanguage`, `runtimeProfile`, `streamerMode` (required), `hostTools`. DR's `huntRegion`, `exposure`, `showConsole` go |
| `Snapshot` (217–244) | DR's `install {…}` replaced by `game: GameInfo`, `revived` (1.4.4), `runtime: RuntimeInfo`; DR's `game {running, relayPort}` renamed `running {game, relayPort}`; `app.platform` added |
| `Branding` (252–255) | URLs are `rr-art://` |
| Host page types (257–359) | New: `GameDataState`, `HostKit`, `HostMultipliers`, `MAX_MULTIPLIER`, `HostUpdateView`, `RequirementId`, `Requirement`, `HostServerState`, `HostServerView`, `HostJobKind`, `HostJob` |
| `ExternalTarget` (363–387) | New: `project_issues`, `project_docs_players`, `project_docs_hosting`, `project_docs_sdk`, `installing_guide`, `legendary`, `dumper7`, `visual_studio`, `rust`, `nodejs`, `mongodb`, `git`, `visual_studio_installer`, `rust_installer`, `dumper7_zip`, `revived_source`, `mystic_source`, `undaunted_source`. Kept: `tailscale_download`, `tailscale_share`, `vc_redist`, `server_source`, `project_source`, `project_license`. DR only: `directx_runtime`, `project_contributors`, `upstream_source`, `upstream_contributors`, `eugamehost*` (6), `discord`, `patreon`. `nodejs` and `mongodb` go once the host kit needs neither (RD3, RB15) |
| `IPC` (394–459) | `rr:` prefix; the list is section 11 |
| `HostAction`, `HostFolder`, `HostSetupRequest` (461–482) | New |

## 6. `src/shared/invite.ts`

- Reforged: `ReforgedLauncher/src/shared/invite.ts` (246 lines). DR Node: `src/shared/invite.ts` (234).
- DR Rust: `protocol::invite` (`rust/crates/protocol/src/invite.rs`, `INVITE_SCHEME = "dauntless-revived"` at line
  15) and the launcher's `core/invite.rs`.
- Changed lines: 32 (+22 −10). Rows: **RS1** (protocol), RA4 (launcher use).

Behavior changes:

- **Scheme (24).** `INVITE_SCHEME = "reforged-revival"`. The two formats (v1 private, v2 public) and every field are
  DR's (comments 1–22).
- **Second scheme (25–30, 36, 139–141, 208).** `dauntless-revived://` invites are read as well and marked
  `game: "revived"`; `reforged-revival://` gives `game: "reforged"`. RD5 replaces this with a typed refusal: the
  parser recognises the `dauntless-revived` prefix and answers an error the page can word, never an `Invite`.
- **`formatInvite` (235, 245).** Writes the scheme of the invite's `game`; with RD5 only `reforged-revival`.

## 7. `src/shared/status.ts`

- Reforged: `ReforgedLauncher/src/shared/status.ts` (188 lines). DR Node: `src/shared/status.ts` (165).
- DR Rust: `protocol::status` and the launcher's `core/status.rs`.
- Changed lines: 33 (+28 −5). Rows: **RS1**, RA4.

Behavior changes:

- **Route (1–2).** `GET /reforged/api/ServerStatus` (DR `/undaunted/api/ServerStatus`); the contract is DR's plus
  three fields.
- **`game` (31–35, 106–112, 165).** `{ build ≤ 64 chars, changelist 0–100,000,000 or null }`, null when `build` is
  missing.
- **`gameData` (10–12, 62, 166).** `real`, `synthetic`, `missing`; anything else `unknown`. The page warns on
  `synthetic` and `missing` (`renderer/index.ts` 750–754).
- **`chat` (167).** `true` only when the server says so.
- **`contentPort` (50–52, 168).** Kept but only the 1.4.4 path reads it (`controller.ts` 1348). After RD5 the launcher
  does not need it; DR's protocol has it for its own content server.

## 8. `src/shared/i18n.ts`

- Reforged: `ReforgedLauncher/src/shared/i18n.ts` (1,632 lines): `en` (7–806) and `ptBR` (810–1609) in one file,
  770 keys each, then `translate`, `isStringKey`, `systemLanguage`.
- DR Node: `src/shared/i18n/index.ts` (registry), `en.ts` (510 keys), `pt.ts` and seven more languages; README on
  adding one.
- DR Rust: `core/i18n.rs` (the backend's own text: `APP_TITLE`, `CLOSE_*`, `DIALOG_*`, from `gen/strings.json` made
  by `build.rs`), `core/text.rs`.
- Row: **RA3**.

Behavior changes:

- **Split (RD14).** One file per language as DR's L13 did: `i18n/en.ts`, `i18n/pt-BR.ts` (the registry key must be
  `"pt-BR"`, not DR's `pt`), `i18n/index.ts` with `REGISTRY = { en, "pt-BR": … }`.
- **`systemLanguage` (1626–1632).** Any Portuguese locale (`pt`, `pt-*`, `pt_*`) gives `pt-BR`; an exact `en`
  otherwise, else `en`. DR takes the first two letters and matches them against nine codes.
- **Backend text.** `core/i18n.rs` names `close_text_offline`, `dialog_install_folder` and `dialog_existing_folder`,
  which Reforged does not have, and lacks the seven `dialog_*` keys `main.ts` uses (section 4).
- **Language names.** Reforged's page names the two languages itself (`renderer/index.ts` 54); DR has `lang_*` keys.
- **Keys.** 448 keys are new against DR's `en.ts`, 37 exist only for 1.4.4, 188 of DR's are not in Reforged
  (section 12).

## 9. Other `src/shared/` files

| File | Changed | Change | DR Rust | Row |
| --- | --- | --- | --- | --- |
| `username.ts` (38 lines) | 8 | Keys made by a Reforged server are `RRK_<48 hex>` (22, 36); DR `UUK_`. Username rule unchanged | `core/username.rs`, `protocol::account` | RS1, RA4 |
| `format.ts` (73) | 50 | Two languages: comma decimals for `pt-BR` (7–10), `d`/`h` units for both, `acabou de começar`, Portuguese month names (57–61), new `formatPercentText` (71–72). DR's per-language date formats go | page only | RA3 |
| `ui-model.ts` (154) | 114 | `PrimaryAction` gains `locate`, `verify`, `trust`, `install_runtime` (`revived_install` 1.4.4 only) and loses `install`, `update`, `pause`, `resume`; five steps (`STEP_KEYS` 86); new `hostSteps` (131–154). DR's `exposureKey`, `playersKey`, `updateReason`, `shortFile` go | page only | RA13 |
| `credits.ts` (86) | 97 | English and Portuguese; `CreditProject` with link names `revived_source`, `mystic_source`, `undaunted_source`; no patrons | page only | RA3 |
| `text.ts` (17) | 0 | Identical | `core/text.rs` | — |

## 10. The renderer page

- Reforged: `ReforgedLauncher/src/renderer/index.ts` (2,626 lines), `dom.ts` (169), `scene.ts` (281), `styles.css`
  (3,377), `modern.css` (369, new), `global.d.ts` (13, new), `brand/`, and `ReforgedLauncher/index.html` (145).
- DR Node: the same names, plus `api.ts`, `backend.ts`, `dauntless-style.css`, `vite-env.d.ts`.
- DR Rust: none. The page stays TypeScript and talks to `bridge/`.
- Changed lines: `index.ts` 2,048; `dom.ts` 19; `scene.ts` 26; `styles.css` 1,535; `index.html` 52.
- Rows: **RA2** (the page's calls into the bridge), **RA13** (the page against the Rust controller; `ui.test.ts`).
  Plan section 6: the page's look and both languages stay word for word.

Behavior changes in `index.ts`:

- **Bridge (51).** `const api = window.launcher`, the Electron preload only. DR's page imports `api` and `artUrl` from
  `backend.ts`, which picks Tauri's `invoke` when `__TAURI_INTERNALS__` is set. Needed before the page runs in Tauri.
- **Art (52, 2480–2520).** Accepts `rr-art://bg/<sha256>` only. On WebView2 Tauri serves custom schemes as
  `http://<scheme>.localhost/…`; DR's `artUrl` (`shared/art.ts`) maps the name. Reforged needs the same for `rr-art`.
- **Views (39–40, 2124–2216).** `play`, `news`, `server`, `host` (only with `hostTools`), `settings`, `credits`. DR's
  `partners` and `mods` go.
- **Modals (41–49, 2279–2479).** New `server_moved` (2309, the carry-account confirmation; `submitInvite(…, false,
  true)`), `trust_runtime` (2341, lists the runtime files and their hashes). `cert_changed` as DR's. DR's
  `offline_restore` goes.
- **Play view (383–862).** Join, connect and register as DR's; new `playLocate` (672–713) with the detected installs
  card, the pasted-path form and the wrong-version advice; new `playRuntime` (723–742) for `untrusted`, `missing`,
  `outdated`, `unavailable` (with the host hint, 739); `playInstalling` (744–748) for `verify` and `runtime` tasks;
  `gameDataWarning` (750–754); `playReady` with the build badge and local-runtime note (762–796); `hostTeaser`
  (444–458) when Host tools are off. `playLocateRevived` (652–670) and the 1.4.4 branches (746, 773, 779, 912, 1008)
  go with RD5.
- **Action bar (864–1031).** Five steps; primary actions `locate`, `verify`, `trust`, `install_runtime`, `play`,
  `cancel`. No pause or resume.
- **Host page (1217–1760), new.** Tabs `requirements`, `build`, `gamedata`, `server`; the workspace card, requirement
  cards with copyable commands, build and publish steps (Windows only: `snap.app.platform`, 1599, 1648, 1733), the
  server console, setup form, invite with `copyText` (1264–1300), multipliers, update. Pushed state arrives through
  `onHostKit` and `onHostServer`.
- **Settings (1881–2020).** Tabs `game`, `graphics`, `account`, `launcher`; runtime profile, local runtime folder,
  logs folder, Host tools toggle (1964–1977). DR's exposure, hunt region, show-console rows go.
- **Credits (2022–2086).** Projects instead of patrons and contributors.

Other files: `dom.ts` adds 15 icons for the Host page and drops `chevron_down`; `scene.ts` changes the seed (1147)
and adds a forge-ember layer (205–217, 280); `styles.css` and `modern.css` are Reforged's look; `index.html` has
Reforged's title, CSP with `rr-art:` and the ember layer. None of these touch the bridge.

## 11. IPC command list (for RA2)

Every call `ReforgedLauncher/src/renderer/index.ts` makes today; each `LauncherApi` method in `preload.ts` is used by
the page at least once. The Tauri name follows DR's rule (`bridge/commands.rs`: the channel without its prefix, in
snake_case). Limits are those `main.ts` and `ipc-validate.ts` apply.

Status: **DR** = in DR's `bridge/spec.rs` `COMMANDS` with the same arguments; **DR, changed** = in it, but
arguments, limits or answer differ; **new** = not in DR's spec; **dropped** = `revived/` (1.4.4), not ported (RD5);
**page** = served by Tauri's window API in DR's `api.ts`, not a command.

| # | `LauncherApi` method | Electron channel | Tauri command | Arguments (limits) | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | `getSnapshot` | `rr:get-snapshot` | `get_snapshot` | — | DR |
| 2 | `checkInvite` | `rr:check-invite` | `check_invite` | `text` ≤ 2048 | DR, changed: answer has `movedFrom`; a `dauntless-revived://` invite is refused (RS1) |
| 3 | `submitInvite` | `rr:submit-invite` | `submit_invite` | `text` ≤ 2048, `acceptNewCertificate` flag, `carryAccount` flag | DR, changed: `carryAccount` added |
| 4 | `retryConnect` | `rr:retry-connect` | `retry_connect` | — | DR |
| 5 | `forgetServer` | `rr:forget-server` | `forget_server` | — | DR |
| 6 | `switchServer` | `rr:switch-server` | `switch_server` | `id` 24 lower-case hex | DR |
| 7 | `removeSavedServer` | `rr:remove-saved-server` | `remove_saved_server` | `id` 24 lower-case hex | DR |
| 8 | `register` | `rr:register` | `register` | `username` ≤ 32 | DR |
| 9 | `useExistingKey` | `rr:use-existing-key` | `use_existing_key` | `text` ≤ 8192 | DR |
| 10 | `importKeyFile` | `rr:import-key-file` | `import_key_file` | — (file dialog) | DR |
| 11 | `saveKeyBackup` | `rr:save-key-backup` | `save_key_backup` | — (file dialog) | DR |
| 12 | `dismissBackupOffer` | `rr:dismiss-backup-offer` | `dismiss_backup_offer` | — | DR |
| 13 | `chooseGameFolder` | `rr:choose-game-folder` | `choose_game_folder` | — (folder dialog) | new (replaces `choose_install_dir`, `use_existing_game_folder`) |
| 14 | `useGamePath` | `rr:use-game-path` | `use_game_path` | `gamePath` ≤ 260 | new (DR's `use_existing_game_path`, same limit, renamed) |
| 15 | `useDetectedGame` | `rr:use-detected-game` | `use_detected_game` | `index` integer 0–15 | new; needs an index argument kind |
| 16 | `verifyGame` | `rr:verify-game` | `verify_game` | — | new |
| 17 | `revivedInstall` | `rr:revived-install` | — | `mode` `install` or `repair` | dropped |
| 18 | `revivedChooseFolder` | `rr:revived-choose-folder` | — | — | dropped |
| 19 | `revivedUseDetected` | `rr:revived-use-detected` | — | `index` 0–15 | dropped |
| 20 | `trustRuntime` | `rr:trust-runtime` | `trust_runtime` | — | new |
| 21 | `installRuntime` | `rr:install-runtime` | `install_runtime` | — | new |
| 22 | `chooseLocalRuntime` | `rr:choose-local-runtime` | `choose_local_runtime` | — (folder dialog) | new |
| 23 | `clearLocalRuntime` | `rr:clear-local-runtime` | `clear_local_runtime` | — | new |
| 24 | `cancelTask` | `rr:cancel-task` | `cancel_task` | — | DR |
| 25 | `play` | `rr:play` | `play` | — | DR |
| 26 | `setSettings` | `rr:set-settings` | `set_settings` | `patch`: `graphics`, `windowed`, `streamerMode`, `hostTools`, `language`, `gameLanguage`, `runtimeProfile` | DR, changed: `SETTINGS_KEYS` |
| 27 | `setStatusPolling` | `rr:set-status-polling` | `set_status_polling` | `on` flag | DR |
| 28 | `refreshStatus` | `rr:refresh-status` | `refresh_status` | — | DR |
| 29 | `getNews` | `rr:get-news` | `get_news` | — | DR |
| 30 | `getBranding` | `rr:get-branding` | `get_branding` | — | DR |
| 31 | `openExternal` | `rr:open-external` | `open_external` | `target` ≤ 32, one of section 5's names | DR, changed: the name table |
| 32 | `openGameFolder` | `rr:open-game-folder` | `open_game_folder` | — | DR |
| 33 | `openLogsFolder` | `rr:open-logs-folder` | `open_logs_folder` | — | new |
| 34 | `logout` | `rr:logout` | `logout` | — | DR |
| 35 | `installUpdate` | `rr:install-update` | `install_update` | — | DR |
| 36 | `hostGet` | `rr:host-get` | `host_get` | — (requirements looked at afresh) | new |
| 37 | `hostChooseWorkspace` | `rr:host-choose-workspace` | `host_choose_workspace` | — (folder dialog) | new |
| 38 | `hostChooseDumper` | `rr:host-choose-dumper` | `host_choose_dumper` | — (DLL dialog) | new |
| 39 | `hostImportGameData` | `rr:host-import-gamedata` | `host_import_gamedata` | — (`.gz` dialog) | new |
| 40 | `hostImportSdkFolder` | `rr:host-import-sdk-folder` | `host_import_sdk_folder` | — (folder dialog) | new |
| 41 | `hostDownloadProject` | `rr:host-download-project` | `host_download_project` | — | new |
| 42 | `hostAction` | `rr:host-action` | `host_action` | `action`: one of 11 `HostAction` names | new; needs a name-list argument kind |
| 43 | `hostOpen` | `rr:host-open` | `host_open` | `folder`: one of 7 `HostFolder` names | new; same |
| 44 | `hostServerGet` | `rr:host-server-get` | `host_server_get` | — | new |
| 45 | `hostServerStart` | `rr:host-server-start` | `host_server_start` | — | new |
| 46 | `hostServerStop` | `rr:host-server-stop` | `host_server_stop` | — | new |
| 47 | `hostSetup` | `rr:host-setup` | `host_setup` | `{ name` ≤ 60 no control characters, `mode` `private`/`public`, `publicHost` DNS name or IPv4 ≤ 253 or null `}`; refusal detail `setup_form` | new; needs an object argument kind |
| 48 | `hostCreateInvite` | `rr:host-create-invite` | `host_create_invite` | `forName` ≤ 40 no control characters, or null | new (`OptionalText` fits) |
| 49 | `hostUpdate` | `rr:host-update` | `host_update` | — | new |
| 50 | `hostSetMultipliers` | `rr:host-set-multipliers` | `host_set_multipliers` | `{ xp, rewards }` numbers 1–100, or null; refusal detail `multipliers_form` | new; needs an object argument kind |
| 51 | `copyText` | `rr:copy-text` | `copy_text` | `text` ≤ 4096; answers a boolean | new (or the clipboard plugin, scoped to write) |
| 52 | `dismissNotice` | `rr:dismiss-notice` | `dismiss_notice` | — | DR |
| 53 | `dismissError` | `rr:dismiss-error` | `dismiss_error` | — | DR |
| 54 | `minimize` | `rr:window-minimize` | — | — | page |
| 55 | `toggleMaximize` | `rr:window-maximize` | — | — | page |
| 56 | `close` | `rr:window-close` | — | — | page |
| 57 | `takeInviteLink` | `rr:take-invite-link` | `take_invite_link` | — | DR |

Events (main to page):

| # | `LauncherApi` method | Electron channel | Tauri | Status |
| --- | --- | --- | --- | --- |
| E1 | `onSnapshot` | `rr:snapshot` | event `snapshot` | DR |
| E2 | `onProgress` | `rr:progress` | command `subscribe_progress` with a `Channel` | DR |
| E3 | `onHostKit` | `rr:host-kit` | event `host-kit` | new |
| E4 | `onHostServer` | `rr:host-server` | event `host-server` | new (carries the newest invite link, a secret: plan section 6 says only the key never crosses, so this is allowed, but it should not be logged) |
| E5 | `onInviteLink` | `rr:invite-link` | event `invite-link` | DR |
| E6 | `onWindowState` | `rr:window-state` | event `window-state` | DR |

Totals: 51 commands plus `subscribe_progress` (27 in DR's spec, 25 new), 3 dropped, 3 served by the page; 6
events (4 in DR, 2 new).

In DR's spec and not used by Reforged's page: `choose_install_dir`, `use_existing_game_folder`,
`use_existing_game_path` (renamed, row 14), `start_install`, `pause_task`, `resume_task`, `repair`, `start_offline`,
`stop_offline`, `offline_status`, `offline_export_save`, `offline_import_save`, and the `offline-status-changed`
event. RA2 removes them from `COMMANDS`, `generate_handler!` and the isolation hook together
(`every_command_is_served_and_nothing_else` checks the first two agree).

## 12. i18n keys

Counted on the `en` table: Reforged `i18n.ts` lines 7–806 against DR `src/shared/i18n/en.ts`. Reforged's `ptBR`
table has the same 770 keys.

| Set | Count |
| --- | --- |
| Reforged keys | 770 |
| DR keys | 510 |
| In both | 322 |
| New against DR, excluding the 1.4.4 ones | 423 |
| Only for 1.4.4 (DR's servers), all of them | 37 (25 new, 12 also in DR) |
| In DR only | 188 |

RA3 also adds one key: the refusal of a `dauntless-revived://` invite (RD5, RS1). RA15's 1.6.0 bridge needs a string
too, but in the Electron launcher, not here.

### 12.1 Keys only for 1.4.4

Reforged lines 713–750 (`en`) and 1516–1553 (`ptBR`). All are removed by RA3 (RD5).

- New against DR (25): `game_source_revived`, `edition_revived`, `btn_download`, `btn_check`,
  `hint_revived_download`, `hint_revived_check`, `ready_build_revived`, `revived_download_title`,
  `revived_download_text`, `revived_check_title`, `revived_check_text`, `revived_check_missing`,
  `revived_folder_target`, `revived_folder_current`, `revived_choose`, `revived_choose_hint`,
  `revived_detected_title`, `revived_about`, `revived_installing_download`, `revived_installing_verify`,
  `revived_installing_dlls`, `revived_installing_text`, `revived_repair`, `dialog_revived_folder`,
  `err_folder_other_game`.
- Also in DR, used in Reforged only by the 1.4.4 path (12): `task_download`, `task_dlls`, `notice_install_done`,
  `notice_repair_done`, `err_manifest_mismatch`, `err_no_content_server`, `err_file_missing_on_server`,
  `err_file_different_on_server`, `err_dll_failed`, `err_game_files_invalid`, `err_folder_not_empty`,
  `err_windows_runtime_missing`.

### 12.2 New keys (not 1.4.4)

Grouped by prefix, with the count per group.

- **about_** (1): `about_issues`
- **btn_** (5): `btn_cancel`, `btn_locate`, `btn_trust`, `btn_verify`, `btn_waiting_host`
- **credits_** (1): `credits_source`
- **dialog_** (6): `dialog_dumper_dll`, `dialog_game_folder`, `dialog_gamedata_pack`, `dialog_runtime_folder`,
  `dialog_sdk_folder`, `dialog_workspace_folder`
- **err_** (9): `err_account_banned_noreason`, `err_game_wrong_version`, `err_host_failed`,
  `err_local_runtime_invalid`, `err_runtime_different_on_server`, `err_runtime_failed`, `err_runtime_in_use`,
  `err_runtime_unavailable`, `err_runtime_untrusted`
- **game_** (4): `game_source_chosen`, `game_source_epic`, `game_source_heroic`, `game_source_legendary`
- **gamedata_** (8): `gamedata_kind_missing`, `gamedata_kind_real`, `gamedata_kind_synthetic`,
  `gamedata_kind_unknown`, `gamedata_missing_text`, `gamedata_missing_title`, `gamedata_synthetic_text`,
  `gamedata_synthetic_title`
- **hbuild_** (1): `hbuild_intro`
- **hgd_** (17): `hgd_example_title`, `hgd_export_found`, `hgd_export_none`, `hgd_export_title`, `hgd_generate`,
  `hgd_generate_text`, `hgd_generate_title`, `hgd_intro`, `hgd_pack_open`, `hgd_pack_save`, `hgd_pack_text`,
  `hgd_pack_title`, `hgd_restart`, `hgd_step_build`, `hgd_step_generate`, `hgd_step_inject`, `hgd_step_play`
- **hint_** (5): `hint_locate`, `hint_runtime`, `hint_runtime_none`, `hint_trust`, `hint_verify`
- **hmult_** (12): `hmult_apply`, `hmult_disable`, `hmult_enable`, `hmult_external`, `hmult_off`, `hmult_on`,
  `hmult_range`, `hmult_restart`, `hmult_rewards`, `hmult_text`, `hmult_title`, `hmult_xp`
- **host_** (133): `host_build_exporter`, `host_build_loader`, `host_build_missing`, `host_build_no_cargo`,
  `host_build_no_vs`, `host_build_ready`, `host_build_runtime`, `host_build_text`, `host_build_title`, `host_built`,
  `host_data_backend`, `host_data_copy`, `host_data_director`, `host_data_import_pack`, `host_data_inject`,
  `host_data_need_exporter`, `host_data_need_game`, `host_data_step_copy`, `host_data_step_export`,
  `host_data_step_generate`, `host_data_step_pack`, `host_data_step_synthetic`, `host_data_synthetic`,
  `host_data_text`, `host_data_title`, `host_err_cargo`, `host_err_dumper`, `host_err_exporter`, `host_err_folder`,
  `host_err_game_not_running`, `host_err_gamedata_missing`, `host_err_git_missing`, `host_err_invite`,
  `host_err_msbuild`, `host_err_multipliers_form`, `host_err_multipliers_public`, `host_err_no_dump`,
  `host_err_no_exports`, `host_err_no_process`, `host_err_node_missing`, `host_err_not_configured`,
  `host_err_npm_missing`, `host_err_pack_damaged`, `host_err_pack_game`, `host_err_pack_invalid`,
  `host_err_project_folder_taken`, `host_err_project_no_game`, `host_err_sdk_folder`, `host_err_server_external`,
  `host_err_server_not_running`, `host_err_server_running`, `host_err_setup_form`, `host_err_update`,
  `host_err_update_diverged`, `host_err_update_fetch_failed`, `host_err_update_local_changes`,
  `host_err_update_merge_failed`, `host_err_update_no_upstream`, `host_err_update_not_git`, `host_err_upstream_old`,
  `host_err_windows_only`, `host_err_workspace`, `host_get_mongodb`, `host_get_node`, `host_get_rust`, `host_get_vs`,
  `host_intro`, `host_job_build_exporter`, `host_job_build_loader`, `host_job_build_runtime`,
  `host_job_copy_exports`, `host_job_create_invite`, `host_job_download_project`, `host_job_export_gamedata`,
  `host_job_generate_gamedata`, `host_job_import_gamedata`, `host_job_import_sdk`, `host_job_inject`,
  `host_job_multipliers`, `host_job_publish_runtime`, `host_job_setup`, `host_job_state_done`,
  `host_job_state_failed`, `host_job_state_running`, `host_job_synthetic_data`, `host_job_update`,
  `host_job_upstream`, `host_not_built`, `host_open`, `host_open_logs`, `host_publish`, `host_publish_missing`,
  `host_publish_ready`, `host_publish_text`, `host_publish_title`, `host_run_chip`, `host_run_text`,
  `host_run_title`, `host_sdk_choose`, `host_sdk_choose_folder`, `host_sdk_found`, `host_sdk_get`, `host_sdk_import`,
  `host_sdk_missing`, `host_sdk_need_game`, `host_sdk_need_setup`, `host_sdk_none`, `host_sdk_ready`, `host_sdk_run`,
  `host_sdk_step_dumper`, `host_sdk_step_import`, `host_sdk_step_run`, `host_sdk_text`, `host_sdk_title`,
  `host_tab_build`, `host_tab_gamedata`, `host_tab_requirements`, `host_tab_server`, `host_teaser_docs`,
  `host_teaser_open`, `host_teaser_text`, `host_teaser_title`, `host_title`, `host_windows_only`, `host_ws_choose`,
  `host_ws_download`, `host_ws_download_no_game`, `host_ws_download_to`, `host_ws_missing`, `host_ws_ready`,
  `host_ws_text`, `host_ws_title`, `host_ws_unpatched`
- **hreq_** (14): `hreq_copied`, `hreq_copy`, `hreq_download`, `hreq_group_build`, `hreq_group_host`,
  `hreq_group_play`, `hreq_hide`, `hreq_how`, `hreq_intro`, `hreq_open_page`, `hreq_recheck`, `hreq_state_missing`,
  `hreq_state_ok`, `hreq_state_problem`
- **hsrv_** (52): `hsrv_address`, `hsrv_check_gamedata`, `hsrv_check_gamedata_example`, `hsrv_check_runtime`,
  `hsrv_check_setup`, `hsrv_check_workspace`, `hsrv_checks`, `hsrv_console`, `hsrv_console_empty`,
  `hsrv_console_hide`, `hsrv_console_show`, `hsrv_fix`, `hsrv_invite_copied`, `hsrv_invite_copy`,
  `hsrv_invite_create`, `hsrv_invite_for`, `hsrv_invite_need_running`, `hsrv_invite_ready`, `hsrv_invite_tailscale`,
  `hsrv_invite_text`, `hsrv_invite_title`, `hsrv_mode_private`, `hsrv_mode_public`, `hsrv_mode_short_private`,
  `hsrv_mode_short_public`, `hsrv_setup_done`, `hsrv_setup_game`, `hsrv_setup_host`, `hsrv_setup_mode`,
  `hsrv_setup_name`, `hsrv_setup_no_game`, `hsrv_setup_run`, `hsrv_setup_tailscale`, `hsrv_setup_text`,
  `hsrv_setup_title`, `hsrv_setup_update`, `hsrv_start`, `hsrv_state_external`, `hsrv_state_failed`,
  `hsrv_state_running`, `hsrv_state_starting`, `hsrv_state_stopped`, `hsrv_state_stopping`, `hsrv_stop`,
  `hsrv_text_external`, `hsrv_text_failed`, `hsrv_text_failed_noreason`, `hsrv_text_running`, `hsrv_text_starting`,
  `hsrv_text_stopped`, `hsrv_text_stopping`, `hsrv_title`
- **hupd_** (10): `hupd_external`, `hupd_git`, `hupd_note`, `hupd_run`, `hupd_runtime`, `hupd_text`, `hupd_title`,
  `hupd_to_build`, `hupd_up_to_date`, `hupd_updated`
- **installing_** (3): `installing_runtime_text`, `installing_runtime_title`, `installing_verify_text`
- **locate_** (28): `locate_browse`, `locate_current`, `locate_found_title`, `locate_found_version`,
  `locate_guide`, `locate_in_use`, `locate_legendary`, `locate_missing_text`, `locate_missing_title`,
  `locate_none_text`, `locate_none_title`, `locate_path`, `locate_path_hide`, `locate_path_label`,
  `locate_path_note`, `locate_path_placeholder`, `locate_path_use`, `locate_text`, `locate_title`, `locate_use`,
  `locate_verify_text`, `locate_verify_title`, `locate_wrong_fix`, `locate_wrong_found`, `locate_wrong_subtitle`,
  `locate_wrong_text`, `locate_wrong_title`, `locate_wrong_unknown`
- **moved_** (4): `moved_advice`, `moved_confirm`, `moved_text`, `moved_title`
- **nav_** (1): `nav_host`
- **notice_** (5): `notice_game_verified`, `notice_host_done`, `notice_runtime_installed`,
  `notice_runtime_installed_restart`, `notice_sdk_imported`
- **profile_** (2): `profile_development`, `profile_production`
- **ready_** (2): `ready_build`, `ready_local_runtime`
- **req_** (50): `req_dauntless_name`, `req_dauntless_s1`, `req_dauntless_s2`, `req_dauntless_why`,
  `req_dumper7_name`, `req_dumper7_s1`, `req_dumper7_s2`, `req_dumper7_s3`, `req_dumper7_why`, `req_git_name`,
  `req_git_s1`, `req_git_s2`, `req_git_s3`, `req_git_why`, `req_mongodb_name`, `req_mongodb_s1`, `req_mongodb_s2`,
  `req_mongodb_s3`, `req_mongodb_s4`, `req_mongodb_s5`, `req_mongodb_service_running`, `req_mongodb_why`,
  `req_node_name`, `req_node_s1`, `req_node_s2`, `req_node_s3`, `req_node_too_old`, `req_node_why`, `req_rust_name`,
  `req_rust_s1`, `req_rust_s2`, `req_rust_s3`, `req_rust_s4`, `req_rust_why`, `req_tailscale_name`,
  `req_tailscale_s1`, `req_tailscale_s2`, `req_tailscale_s3`, `req_tailscale_why`, `req_vc_redist_name`,
  `req_vc_redist_s1`, `req_vc_redist_s2`, `req_vc_redist_why`, `req_visual_studio_name`, `req_visual_studio_no_cpp`,
  `req_visual_studio_s1`, `req_visual_studio_s2`, `req_visual_studio_s3`, `req_visual_studio_s4`,
  `req_visual_studio_why`
- **running_** (2): `running_relay_text`, `running_relay_title`
- **runtime_** (17): `runtime_files_title`, `runtime_install_text`, `runtime_install_title`,
  `runtime_none_host_text`, `runtime_none_host_title`, `runtime_none_text`, `runtime_none_title`,
  `runtime_state_local`, `runtime_state_missing`, `runtime_state_ok`, `runtime_state_outdated`,
  `runtime_state_unavailable`, `runtime_state_untrusted`, `runtime_trust_detail`, `runtime_trust_text`,
  `runtime_trust_title`, `runtime_update_title`
- **server_** (6): `server_chat`, `server_chat_off`, `server_chat_on`, `server_game`, `server_gamedata`,
  `server_runtime`
- **set_** (17): `set_advanced`, `set_folder_none`, `set_host_tools`, `set_host_tools_text`, `set_local_runtime`,
  `set_local_runtime_choose`, `set_local_runtime_clear`, `set_local_runtime_text`, `set_logs`, `set_logs_open`,
  `set_logs_text`, `set_runtime_profile`, `set_runtime_profile_text`, `set_verify`, `set_verify_button`,
  `set_verify_ok`, `set_verify_text`
- **step_** (2): `step_game`, `step_runtime`
- **task_** (1): `task_runtime`
- **trust_** (5): `trust_advice`, `trust_confirm`, `trust_detail`, `trust_text`, `trust_title`

### 12.3 Keys in DR only

Not in Reforged's file. The Rust copy needs none of them once RA3 builds `gen/strings.json` from Reforged's files;
the ones `core/i18n.rs` names (`close_text_offline`, `dialog_install_folder`, `dialog_existing_folder`) have to go
from there too.

- **btn_** (2): `btn_pause`, `btn_resume`
- **close_** (1): `close_text_offline`
- **credits_** (9): `credits_all_contributors`, `credits_github_handle`, `credits_patrons_text`,
  `credits_patrons_title`, `credits_project_text`, `credits_role_patron`, `credits_upstream_contributors`,
  `credits_upstream_source`, `credits_upstream_text`
- **dialog_** (2): `dialog_existing_folder`, `dialog_install_folder`
- **discord_** (1): `discord_link`
- **dx_** (3): `dx_button`, `dx_text`, `dx_title`
- **err_** (8): `err_compat_runtime_missing`, `err_disk_space_need`, `err_manifest_unavailable`,
  `err_offline_insufficient_memory`, `err_offline_later`, `err_offline_save_invalid`, `err_offline_unavailable`,
  `err_verify_failed`
- **exposure_** (2): `exposure_basic`, `exposure_game`
- **hint_** (2): `hint_install`, `hint_update`
- **install_** (20): `install_change`, `install_existing`, `install_existing_browse`, `install_existing_hide`,
  `install_existing_note`, `install_existing_path`, `install_existing_placeholder`, `install_existing_text`,
  `install_existing_use`, `install_folder`, `install_free`, `install_low_space`, `install_need`,
  `install_need_label`, `install_no_content`, `install_no_content_title`, `install_size`, `install_text`,
  `install_text_public`, `install_title`
- **installing_** (1): `installing_download_title`
- **join_** (1): `join_preview`
- **just_** (1): `just_now`
- **lang_** (9): `lang_de`, `lang_en`, `lang_es`, `lang_fi`, `lang_fr`, `lang_it`, `lang_ja`, `lang_pt`, `lang_ru`
- **mod_** (12): `mod_bhb_name`, `mod_bhb_text`, `mod_bhb_use`, `mod_cst_name`, `mod_cst_text`, `mod_cst_use`,
  `mod_menu_name`, `mod_menu_text`, `mod_menu_use`, `mod_trk_name`, `mod_trk_text`, `mod_trk_use`
- **mods_** (10): `mods_badge`, `mods_by`, `mods_credit`, `mods_intro`, `mods_note_local`, `mods_note_settings`,
  `mods_note_windows`, `mods_notes_title`, `mods_subtitle`, `mods_title`
- **nav_** (2): `nav_mods`, `nav_partners`
- **off_** (42): `off_again`, `off_back`, `off_backup_done`, `off_btn`, `off_btn_play`, `off_cancel`,
  `off_continue`, `off_error_title`, `off_eyebrow`, `off_hint_active`, `off_hint_idle`, `off_menu_offline`,
  `off_menu_online`, `off_more`, `off_name_label`, `off_name_text`, `off_name_title`, `off_restore_confirm`,
  `off_restore_done`, `off_restore_text`, `off_restore_title`, `off_retry`, `off_running_close`, `off_running_note`,
  `off_running_text`, `off_running_title`, `off_save_backup`, `off_save_card`, `off_save_locked`, `off_save_restore`,
  `off_save_text`, `off_save_title`, `off_starting_text`, `off_starting_title`, `off_step_deploy`, `off_step_game`,
  `off_step_metagame`, `off_step_preparing`, `off_stopped_text`, `off_stopped_title`, `off_stopping_title`,
  `off_stopping_wait`
- **partner_** (19): `partner_ad`, `partner_banner_cta`, `partner_banner_label`, `partner_banner_title`,
  `partner_capacity`, `partner_capacity_note`, `partner_capacity_title`, `partner_dedicated`, `partner_disclosure`,
  `partner_explore`, `partner_headroom`, `partner_intro`, `partner_network_dedicated`, `partner_network_vds`,
  `partner_per_month`, `partner_plan_cta`, `partner_recommended`, `partner_small`, `partner_title`
- **partners_** (1): `partners_subtitle`
- **patreon_** (1): `patreon_link`
- **rail_** (1): `rail_signed_in_as`
- **reg_** (1): `reg_hint`
- **region_** (4): `region_auto`, `region_ger`, `region_main`, `region_us`
- **running_** (2): `running_public_text`, `running_public_title`
- **set_** (17): `set_exposure`, `set_exposure_text`, `set_launcher_language`, `set_launcher_language_text`,
  `set_leave`, `set_locked_game`, `set_locked_task`, `set_region`, `set_region_text`, `set_repair`,
  `set_repair_button`, `set_repair_text`, `set_server`, `set_show_console`, `set_show_console_text`, `set_streaming`,
  `set_troubleshooting`
- **sp_** (2): `sp_players_one`, `sp_players_other`
- **step_** (1): `step_install`
- **task_** (4): `task_bytes`, `task_cancel`, `task_download_paused`, `task_note`
- **update_** (4): `update_dlls`, `update_missing`, `update_title`, `update_unverified`
- **vc_** (3): `vc_button`, `vc_text`, `vc_title`

## Open points

Decisions the rows above need; none is made here.

1. **Relay owner check (RA6, RA13).** DR's relay accepts only connections whose socket belongs to the game's PID
   (Windows, `net/relay_owner.rs`). Reforged's Node relay has no such check. Keep DR's (recommended: it is a
   trust-boundary control and costs nothing on Windows), and decide whether it applies to the Dumper-7 session's game,
   which is started without the runtime and never talks to the relay.
2. **Where the new controller code goes (RA2, RA13).** DR's `controller/mod.rs` is one 2.2k-line module. The game
   folder, the runtime and the Host page are each a few hundred lines. Plan section 5.1 has one owner per file, so RA2
   should register them as stub modules (for example `controller/game.rs`, `controller/runtime.rs`,
   `controller/host.rs`) or RA12 and RA13 will both need `mod.rs`.
3. **Argument kinds in the bridge spec (RA2).** `spec.rs` has `Text`, `OptionalText`, `Flag`, `SettingsPatch`,
   `Channel`. Reforged needs an index (`use_detected_game`), a fixed name list (`host_action`, `host_open`, and better
   than `Text` for `open_external`), and two objects (`host_setup`, `host_set_multipliers`). New kinds mean changes to
   `isolation/hook.js` and its generator as well.
4. **Missing Visual C++ files (RA13).** Reforged only logs them before PLAY; DR refuses with
   `windows_runtime_missing` and also checks DirectX. The Reforged code gives no reason for the difference, and
   the error code and its strings exist only for 1.4.4, so RA3 removes them.
   Confirm the warning-only behavior stays.
5. **Background work at shutdown (RA13).** DR waits for news and art downloads (`extrasTask`) and refuses PLAY while
   shutting down; Reforged does neither. Take DR's behavior (recommended) or port Reforged's as is.
6. **`contentPort` in `ServerStatus` (RS1).** After RD5 the launcher no longer reads it. Keep it in `protocol::status`
   (it costs nothing and DR's tests use it) or remove it with the 1.4.4 code.
7. **Copying text (RA2).** Reforged copies the invite link through `copy_text` because the page has no clipboard
   permission. In Tauri, a command or `tauri-plugin-clipboard-manager` with write-only permission; the command keeps the
   4096 limit in one place.
8. **Host page commands while RH ports the host kit (RA12).** The controller calls `npm run setup`, `invite`,
   `multipliers`, `update`, `gamedata`, `upstream` and `git` directly (1834–2187). RD3 replaces them with
   `reforged-host`; `upstream` and the git fallback (`legacyUpdate`) disappear with RP4 and RH5. The bridge commands
   stay the same either way, so RA2 is not blocked, but RA12 has to know which host kit it targets.
9. **Stored 1.4.4 servers (RA8, RA13).** A 1.5.x profile can hold a current or saved server with `game: "revived"`.
   After RD5 the Rust settings reader has to drop or hide those entries (and the `revivedRoot`, `revivedVerified`
   settings), and the page has nothing to show for them. RA15's bridge warns first; RA8 owns the reader.
10. **`dumping` and the injector on Linux (RA12a).** The Dumper-7 session renames `winmm.dll` and starts the game
    directly; under Proton the same steps run inside the prefix. Nothing to decide for Windows; noted for the spike.
