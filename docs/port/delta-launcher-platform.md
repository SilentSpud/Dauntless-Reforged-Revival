<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Delta: launcher network and platform (RX1)

What Reforged changed in the launcher's main-process files since the Node code it shares with Dauntless Revived
(DR), so the Rust rows (RA5 to RA12) carry every change onto DR's `src-tauri` at `e1c5fec`.

- Reforged side: `ReforgedLauncher/src/main/` on this branch (RR2 moves it to `legacy/` in parallel).
- DR Node side: `legacy/UndauntedLauncher/src/main/` at `e1c5fec`.
- DR Rust side: `legacy/UndauntedLauncher/src-tauri/src/` at `e1c5fec`, written below as `net/…`, `platform/…`,
  `core/…`.
- Changed lines are `diff` lines (added plus removed) between the two Node files. Line ranges are in the Reforged
  file unless they say "DR".
- Out of scope here (the controller family's delta file): `controller.ts`, `ipc-validate.ts`, `preload.ts`,
  `main.ts` and the page.

## Identical

| Reforged file | DR Node file | DR Rust module | Row |
| --- | --- | --- | --- |
| `pinned.ts` | `pinned.ts` | `net/pinned.rs` (with `net/connector.rs`) | RA5 |
| `display-repair.ts` | `display-repair.ts` | `platform/game_config.rs` (`repair_display_settings`) | RA9 |

`display-repair.ts` differs by one added comment line (line 1) and no code. Both files are ported as DR has them.

## Summary

| Reforged file | DR Node file | DR Rust module | Changed lines | Row |
| --- | --- | --- | --- | --- |
| `constants.ts` | `constants.ts` | `core/pins.rs`, `platform/paths.rs`, `platform/links.rs` | 94 (+60 −34) | RA1, RA5–RA12 |
| `engineini.ts` | `engineini.ts` | `platform/game_config.rs` | 136 (+82 −54) | RA9 |
| `game-folder.ts` | `game-folder.ts` | `platform/game_folder.rs` | 176 (+146 −30) | RA9, RA10 |
| `hostapi.ts` | `hostapi.ts` | `net/hostapi.rs` | 158 (+92 −66) | RA5, RA7 |
| `http.ts` | `http.ts` | `net/http.rs` | 72 (+72 −0) | RA5 |
| `keystore.ts` | `keystore.ts` | `platform/keystore/{mod,os,legacy}.rs` | 14 (+8 −6) | RA8 |
| `launch.ts` | `launch.ts` | `platform/launch.rs` | 112 (+56 −56) | RA9, RA10 |
| `links.ts` | `links.ts` | `platform/links.rs` | 89 (+41 −48) | RA11 |
| `log.ts` | `log.ts` | `platform/log.rs` | 8 (+4 −4) | RA8 |
| `relay.ts` | `relay.ts` | `net/relay.rs`, `net/relay_owner.rs` | 137 (+87 −50) | RA6 |
| `settings.ts` | `settings.ts` | `platform/settings.rs` | 86 (+58 −28) | RA8 |
| `system.ts` | `system.ts` | `platform/system.rs` | 8 (+6 −2) | RA9 |
| `verify.ts` | `verify.ts` | `net/verify.rs` | 172 (+30 −142) | RA7 |
| `runtime-files.ts` | new | new module | 289 lines | RA7 |
| `hostkit.ts` | new | new module | 459 lines | RA12 |
| `hostserver.ts` | new | new module | 394 lines | RA12 |
| `requirements.ts` | new | new module | 124 lines | RA12 |
| `injector.ts` | new | new module (RD9) | 148 lines | RA12 |
| `revived/` (9 files) | see below | not ported (RD5) | — | — |

## `constants.ts`

- DR Node: `constants.ts`. DR Rust: split over `core/pins.rs` (game pins, ports, timings), `platform/paths.rs`
  (product name, folders), `platform/links.rs` (URLs) and `tauri.conf.json` (identifier).
- Changed lines: 94 (+60 −34).

Behavior to carry:

- Lines 4–25: identity and URLs. `APP_ID` `io.github.freezamotoqueiro.dauntlessreforgedrevival`, `SQUIRREL_NAME`
  `ReforgedRevivalLauncher`, `PROJECT_URL` the Reforged repository, `PROJECT_GIT_URL` and `PROJECT_FOLDER_NAME`
  `"Reforged"` (the Host page's clone, see `hostkit.ts`), license on `main`, issues and the three docs pages
  (`players.md`, `hosting.md`, `sdk-and-game-data.md`); the credits `REVIVED_URL`, `MYSTIC_URL`, `UNDAUNTED_URL`;
  the install guide, Legendary, Dumper-7, Visual Studio, Rust, Node.js and MongoDB pages. DR's
  `PROJECT_CONTRIBUTORS_URL`, `UPSTREAM_URL` and `UPSTREAM_CONTRIBUTORS_URL` go. RA1 (identifier), RA11 (links),
  RA12 (Host page URLs).
- Lines 27–32: direct installer downloads for the Host page's Requirements tab: Git for Windows,
  `vs_community.exe`, `rustup-init` (`win.rustup.rs/x86_64`), the Dumper-7 source zip. RA12.
- Lines 34–35: the update feed is `${PROJECT_URL}/releases/download/launcher-updates`. RA14, not this family.
- Lines 37–48: the game is 1.14.7: `GAME_BUILD` `rel-1.14.7-Archon`, `GAME_CHANGELIST` 647472,
  `GAME_VERSION_TEXT` `dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472`, `PINNED_EXE_SHA256`
  `f3f95fbf…fbb66b`, `EPIC_APP_NAME` `"Jackal"`. DR's 1.4.4 values go (`PINNED_EXE_SHA256` `d3d41e61…`,
  `PINNED_EXE_SIZE`, `EXPECTED_BUILD` in `core/pins.rs`, `PINNED_DLLS`, `DLL_RELATIVE_PATHS`). Reforged has no exe
  size pin: only the hash. RA9.
- Line 52: `VERSION_FILE` `Version.txt` (read by `game-folder.ts`). RA9.
- Lines 54–60: `RUNTIME_FILE_NAMES` `MysticParadox.dll`, `winmm.dll` (the only names ever written next to the game),
  `MAX_RUNTIME_FILE_BYTES` 64 MiB, `RUNTIME_MARKER_FILE` `.reforged-runtime.json`. RA7.
- Lines 62–67: `DEFAULT_RELAY_PORT` 61100 (DR: 61000). DR's `XMPP_PORT` 61099 goes: chat goes through the relay.
  RA6.
- Line 71: `DISK_MARGIN_BYTES` 256 MiB (DR: 1 GiB). DR's `DOWNLOAD_CONCURRENCY` goes. `VC_RUNTIME_FILES` moved to
  `system.ts`. RA7, RA9.
- Unchanged: `WIN64_RELATIVE_DIR`, `EXE_NAME`, `EXE_RELATIVE_PATH`, `CONNECT_TIMEOUT_MS` 5000, `STATUS_POLL_MS`
  15000.

## `engineini.ts`

- DR Node: `engineini.ts`. DR Rust: `platform/game_config.rs`.
- Changed lines: 136 (+82 −54). Row: RA9; the config folder inside the Proton prefix is RA10.

Behavior to carry:

- Lines 49–59, `systemSettingsLines`: `[SystemSettings]` is written only when a graphics level is forced; with the
  default (−1) the section is left out entirely. DR's four memory lines (`r.Streaming.PoolSize=3000` and the rest)
  were kept in Reforged only behind `memory`, which is 1.4.4-only, so the Rust port drops them (RD5). A forced level
  writes the eight `sg.*Quality` lines plus `sg.ResolutionQuality=100` and `r.ScreenPercentage=100` only; DR's
  `r.MipMapLODBias=0`, `r.MaxAnisotropy=16`, `r.Tonemapper.Sharpen=0.6` go. The exposure option goes completely
  (`ExposureMode`, `r.EyeAdaptation.MethodOverride=2`).
- Lines 61–65, `xmppLines`: the port has no default (DR: 61099); the caller passes the relay, so it is
  `ServerAddr="ws://127.0.0.1"`, `ServerPort=<relay port>`, `bUseSSL=false`.
- Lines 67–84, `rewriteEngineIniText(existing, host, port, graphics)`: the two owned sections are dropped as before;
  blank lines left at the top of the kept text are dropped (80–81); the `[SystemSettings]` block and its blank line
  are left out when empty (82–83).
- Lines 86–109: `PHOENIX_ENDPOINTS_SECTION` and `removeIniSection`: before every 1.14.7 launch, Game.ini's
  `[OnlineSubsystemPhoenix]` section (which DR's launcher writes with `127.0.0.1:61000` endpoints and which breaks the
  1.14.7 login) is removed, header through to the next header, matched case-insensitively and with spaces trimmed.
  `removed` counts the `key=` lines skipped (comments `;`/`#` and blanks not counted).
- Line 131: the temporary file is `<file>.rr-tmp` (DR: `.dr-tmp`).
- Lines 136–145: options are `host`, `port` (required), `graphics`, `configDir`, `safeWindow`,
  `clearPhoenixEndpoints`; `memory` is 1.4.4-only and not ported.
- Lines 150–162: with `clearPhoenixEndpoints`, a Game.ini that changed is first copied to `Game.ini.before-reforged`
  (overwritten each time), then written atomically in the same encoding rule as Engine.ini; a missing Game.ini is
  left alone.
- Line 176: the result also returns `phoenixEndpointsRemoved` (the controller logs it).
- Unchanged: `decodeIni`/`encodeIni` (ASCII, else UTF-16LE with BOM), `splitLines`, the GameUserSettings rewrite and
  display repair with its `.before-display-repair-<ms>` backup.

## `game-folder.ts`

- DR Node: `game-folder.ts`. DR Rust: `platform/game_folder.rs`.
- Changed lines: 176 (+146 −30). Row: RA9; the same detection on Linux is RA10.

Behavior to carry:

- Lines 1–18: the launcher never downloads the game; it finds the player's own 1.14.7 install.
- Line 26: `MAX_GAME_ROOT` 180 (DR: 150).
- Lines 35–45, `cleanGameFolderInput`: a non-string input is treated as empty. Off Windows any absolute POSIX path is
  accepted (DR: Linux only). Windows still accepts only drive-letter paths, checked before any file-system call.
- Lines 47–57, `gameRootCandidates`: a path to `Dauntless-Win64-Shipping.exe` itself (case-insensitive) is replaced
  by its folder first; the candidates are de-duplicated.
- Lines 59–66: `hasGameExe(root)`, the exe-is-a-file check, is its own function (used by the store detection).
- Lines 80–108, `Version.txt`: reads at most 256 bytes, strips a UTF-8 BOM, keeps the first line trimmed; text that is
  not 1–128 printable ASCII characters counts as none. `isSupportedVersionText` (87–90): equal to
  `GAME_VERSION_TEXT`, or contains `rel-1.14.7` (case-insensitive) and 647472 as a whole number.
- Lines 110–187, store detection (Windows only; returns nothing elsewhere today):
  - Legendary's `installed.json` in `LEGENDARY_CONFIG_PATH` and `%USERPROFILE%\.config\legendary`, and Heroic's
    copy in `%APPDATA%\heroic\legendaryConfig\legendary\installed.json`: entries whose `app_name` (or key) is
    `Jackal`, `install_path` a non-empty string of at most 260 characters, `version` cut to 64.
  - Epic Games Launcher manifests: `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item`, at most 500 files,
    each at most 1 MiB; Dauntless if `AppName` or `MainGameAppName` is `Jackal` or `DisplayName` is `Dauntless`;
    `InstallLocation` as the root, `AppVersionString` cut to 64.
  - JSON files over 4 MiB (1 MiB for `.item`) or not regular files are ignored; a BOM is stripped.
  - Every hit is run through `locateExistingGame`; misses (a lingering manifest) are dropped, duplicate roots
    (case-insensitive) are dropped, and the version falls back to `Version.txt`.

## `hostapi.ts`

- DR Node: `hostapi.ts`. DR Rust: `net/hostapi.rs`.
- Changed lines: 158 (+92 −66). Row: RA5; the runtime manifest (lines 164–216) is used by RA7.

Behavior to carry:

- Line 16: the key header is `x-reforged-user-api-key` (DR: `x-undaunted-user-api-key`); routes are under
  `/reforged/api` (DR: `/undaunted/api`).
- Lines 18–25: `ApiFlavor` and `REVIVED_FLAVOR` exist only for 1.4.4 servers: not ported (RD5). The Rust module
  hard-codes `/reforged/api` and the Reforged header.
- DR lines 14–26, 28–34, 275–278 removed: `checkDeviceSignals` (`/DeviceSignals`), `saveHuntRegion` (`/HuntRegion`)
  and `endPlaying` (`/PlayingEnded`). Delete `check_device_signals`, `save_hunt_region`, `end_playing` and their
  tests from `net/hostapi.rs`, and `normalize_device_signals` from `platform/system.rs`.
- Lines 31–43, `keyHeaders`: the same rule as DR's `statusHeaders` (a plausible key, and only over pinned TLS or to a
  private-mode host), now exported and used for GetUserInfo too (line 142). DR sent the key to GetUserInfo
  unconditionally; Reforged sends no header when the rule fails.
- Lines 147–150: `account_disabled` is treated like `account_banned`; with no `reason` there is no default detail
  (DR: "Contact support for details.").
- Lines 159–160: GetUserInfo's user id must match `^[A-Za-z0-9_-]{1,128}$` (it goes on the game's command line);
  otherwise `server_error`.
- Lines 164–216, the runtime manifest, replacing DR's `probeContent` and `fetchContentManifest`
  (`/content/v1/manifest`, removed):
  - `GET /content/v1/runtime/manifest`, 10 s, at most 64 KiB, no key header.
  - `parseRuntimeManifest`: `version` a clean string of at most 48; `files` exactly the two names in
    `RUNTIME_FILE_NAMES`, each once, `sha256` lowercase 64 hex, `size` a safe integer from 1 to 64 MiB; anything else
    rejects the whole manifest. Files are sorted by name. `build.changelist` is kept when it is a safe integer, else
    null.
  - Results: `ok`, `none` (404 or 503: the host has not built a runtime), `bad_answer`, `cert_mismatch` (pin error),
    `unreachable`.
- Unchanged in substance: ServerStatus with its `/dauntless-status` fallback, Register, news and the art pack (lines
  218–304).

## `http.ts`

- DR Node: `http.ts`. DR Rust: `net/http.rs`.
- Changed lines: 72 (+72 −0). Row: RA5 (used by RA7).

Behavior to carry:

- Lines 133–203, `requestToFile(ep, path, dest, opts)`: a streamed GET into a file for the runtime files. Same rules
  as `request()`: only the invite's host, pinned TLS in public mode, plain HTTP only where `request()` allows it, no
  redirects. Specifics:
  - `accept: application/octet-stream` and the `host` header as `request()` sends it.
  - `timeoutMs` is an idle timeout, reset on the response and on every chunk ("download stalled").
  - A non-200 status drains the body and resolves with the status; nothing is written.
  - A `content-length` above `maxBytes` fails before the file is opened; a body that grows past `maxBytes` fails
    while streaming (`too_large`).
  - An abort signal fails with `aborted`; pin failures map to `pin`, other socket errors to `network`.
  - `onBytes` reports each chunk's length; resolves 200 once the file is flushed.
- DR's Rust `request()` buffers the body; RA5 adds a streaming variant next to it (or RA7 uses `downloader.rs`'s
  stream code, see Open points).

## `keystore.ts`

- DR Node: `keystore.ts`. DR Rust: `platform/keystore/mod.rs`, `os.rs`, `legacy.rs`.
- Changed lines: 14 (+8 −6). Row: RA8.

Behavior to carry:

- Lines 44–46: the decrypted key file reads `RRK2\n<fingerprint or ->\n<key>` (DR: `DRK2`). A bare key (no header)
  is still read and is bound to no certificate. The Rust `FORMAT` constant changes; existing Reforged 1.5.x files use
  `RRK2`, so `legacy.rs` must accept it.
- Line 159: the backup file's title is "Dauntless Reforged Revival account key".
- Line 168: its hint names "Dauntless Reforged Revival Launcher > Register > I already have a key > Load a backup
  file". These strings move to the language files (RA3).
- Lines 1–3: Reforged's Electron build stores keys through DPAPI only (it never shipped on Linux), so the
  safeStorage reader has no Linux keys to read; DR's Linux Secret Service and KWallet storage is still ported for new
  keys (RD4).
- Unchanged: one file per server id, the plain recovery copy in Documents (`<server id>.txt`), `rebind`,
  `certificateOf`.

## `launch.ts`

- DR Node: `launch.ts`. DR Rust: `platform/launch.rs`.
- Changed lines: 112 (+56 −56). Row: RA9; the Linux launch is RA10.

Behavior to carry:

- Lines 15–18: launch settings drop `host` and `port` and gain `userId` and `username` (from GetUserInfo).
- Lines 33–39: `userId` must match `^[A-Za-z0-9_-]{1,128}$` and `username` `^[A-Za-z0-9_]{1,32}$`; the key keeps its
  own check.
- Lines 40–56: the 1.14.7 command line, in this order: `-DisableEOS`, `-EpicPortal`, `-NoEAC`,
  `-AUTH_TYPE=exchangecode`, `-AUTH_LOGIN=<userId>`, `-AUTH_PASSWORD=<key>`, `-epicapp=Archon`, `-epicenv=Prod`,
  `-epicusername=<username>`, `-epicuserid=<userId>`, `-epicaccountid=<userId>`, `-epicsandboxid=jackal`,
  `-epicdeploymentid=53565ba467df4edbb6f5a3d939a8b4f2`, `-epiclocale=<locale>` (unknown locales fall back to
  `en-US`), then `-windowed -ResX=1280 -ResY=720` when windowed (lines 57–58, as DR). DR's leading
  `<host>:<port>` argument, its placeholder values and `-RamsStore` go. `maskArgs` still
  hides `-AUTH_PASSWORD`.
- Lines 69–77, `launchEnvironment(base, backendUrl, profile)`: `backendUrl` must match
  `^http://127\.0\.0\.1:[1-9][0-9]{0,4}$`; every inherited variable starting `REFORGED_` or `MYSTICPARADOX_`
  (case-insensitive) is removed; then `REFORGED_BACKEND_URL` and `MYSTICPARADOX_RUNTIME_PROFILE` are set.
- Lines 92–93: `pid` is null when no game runs.
- Lines 100–109: `start(win64Dir, args, env)` spawns the exe directly with that environment. DR's `LaunchRuntime`
  wrapper and `extraEnv` (`DR_SHOW_CONSOLE`) are gone in Reforged because it is Windows-only. **On Linux the Rust
  port keeps DR's `LaunchRuntime`** (Proton or Wine as the command) and adds `WINEDLLOVERRIDES` for `winmm` (RA10,
  section 2).
- DR lines 129–142 removed: the Linux `pgrep -f` branch of the running-game check. **Keep it in Rust for Linux**
  (RD4).
- Lines 157–167: `STATUS_CONTROL_C_EXIT` (0xC000013A) marks a game the player closed, not a failure; `gameExitDetail`
  formats an exit code as `0x` plus eight uppercase hex digits.

## `links.ts`

- DR Node: `links.ts`. DR Rust: `platform/links.rs`.
- Changed lines: 89 (+41 −48). Row: RA11.

Behavior to carry:

- Lines 35–57, `FIXED_LINKS`: `tailscale_download`, `vc_redist`, `project_source`, `project_license`,
  `project_issues`, `project_docs_players`, `project_docs_hosting`, `project_docs_sdk`, `installing_guide`,
  `legendary`, `dumper7`, `visual_studio`, `rust`, `nodejs`, `mongodb`, `git`, `visual_studio_installer`,
  `rust_installer`, `dumper7_zip`, `revived_source`, `mystic_source`, `undaunted_source`. DR's `directx_runtime`,
  `project_contributors`, `upstream_source`, `upstream_contributors`, the six `eugamehost*`, `discord` and `patreon`
  go.
- Lines 66, 81–87, `isAllowedStaticUrl`: plain https, no user info, no port; allowed only if the URL is exactly one
  of `FIXED_LINKS`, or its host is `login.tailscale.com` (a private invite's share link). DR's per-host switch
  (any `tailscale.com` page, `aka.ms/vs/*`, the GitHub page set) goes.
- Unchanged: `fixedLinkUrl`, and the host's own `sourceUrl` allowed with an explicit port (line 92).

## `log.ts`

- DR Node: `log.ts`. DR Rust: `platform/log.rs`.
- Changed lines: 8 (+4 −4). Row: RA8.

Behavior to carry:

- Lines 11, 16, 22: account keys are `RRK_…` (DR: `UUK_…`), redacted as `RRK_<hidden>`; a registered secret is
  stored without its `RRK_` prefix too.
- Line 24: the header redacted is `x-reforged-user-api-key`.

## `relay.ts`

- DR Node: `relay.ts`. DR Rust: `net/relay.rs`, `net/relay_owner.rs`.
- Changed lines: 137 (+87 −50). Row: RA6.

Behavior to carry:

- Lines 22–25: the port is 61100 from `constants.ts` (DR: 61000). DR Rust's `relay_port_override` stays as is.
- Lines 27–32: `QOS_PONG` is exactly `<!DOCTYPE html><html><body>pong</body></html>`; `isQosPath` matches
  `^/qos([/?]|$)` case-insensitively.
- Lines 34–36, 66, 171–181: the target is `{host, port, fingerprint | null}`. A null fingerprint is private mode:
  plain HTTP to the host, allowed only when `isPrivateModeHost(host)` (Tailscale or loopback), otherwise the relay
  refuses to start (`listen_failed`). The forwarded `Host` header omits the port only for pinned TLS on 443. DR's
  relay is public mode only.
- Lines 68–69, 220–241, 248–253: a second, best-effort listener on `127.0.0.1:80` (`qosPort`, null turns it off)
  answers the QoS page for `/qos…` and `/` after the same loopback checks (`localRequestProblem`), 404 otherwise. A
  busy port is logged and ignored. It closes with the relay.
- Lines 150–155, 281–284: on the relay port, `GET` or `HEAD` to a QoS path is answered locally with `QOS_PONG`
  (200, `text/html`, `content-length`, `cache-control: no-store`; no body for HEAD) and never forwarded. Both
  listeners count `qosAnswers`.
- Lines 214–215: the start log line names the mode ("pinned TLS" or "plain HTTP, private mode").
- Line 368: WebSocket upgrades use the pinned connection in public mode and a plain socket (`agent: false`) in
  private mode.
- DR lines 53, 134, 167–214 removed: the `authorizeConnection` hook (the game-process owner check, DR
  `relay-owner.ts`, Rust `relay_owner.rs`). Reforged has no owner check. RA6 lists one (see Open points).
- `/__origin/<host><path>` requests pass through unchanged: the relay forwards the path as the runtime sent it and
  the gateway and backend unwrap it (`ReforgedGateway/src/policy.ts`). Nothing to add in the relay for it.

## `settings.ts`

- DR Node: `settings.ts`. DR Rust: `platform/settings.rs`.
- Changed lines: 86 (+58 −28). Row: RA8.

Behavior to carry:

- Line 20, 105: each stored server has `game` (`"revived"` or `"reforged"`, missing means `"reforged"`). 1.4.4 is
  dropped (RD5), so see Open points for stored `revived` servers.
- Removed from DR: `huntRegion` (and its `aus` to `us` migration), `installDir`, `verifiedDir`, `exposure`,
  `showConsole`. Delete them from `platform/settings.rs`; a stored value is ignored.
- Lines 34–36, 139–141: `gameRoot` (the folder that holds `Archon\`). `revivedRoot` and `revivedVerified` are 1.4.4
  only: not ported.
- Lines 42–43, 65–66, 145, 148: `runtimeProfile` (one of `RUNTIME_PROFILES`, default `"production"`) and `hostTools`
  (true only when stored as `true`).
- Lines 46–49, 69–72, 155–164: `trustedRuntimes`, per server id (24 lowercase hex), a list of runtime set ids (64
  lowercase hex), de-duplicated, the last 8 kept; `localRuntimeDir` (a valid folder under 260 characters),
  `hostWorkspace` (under 200), `dumperDll` (under 260, ending `.dll`).
- Lines 80–88: `validDir(value, platform, max = 200)`: longer than 3 and shorter than `max`, no NUL; a drive-letter
  path on Windows, else any absolute POSIX path (DR: Linux only).
- Line 150: stored usernames must pass `isValidUsername` (the registration rule). DR accepted any clean display text
  up to 32 characters, for names an admin renamed.
- Unchanged: language, game language, graphics, windowed, streamer mode, `backupOffered`, the servers list rules.

## `system.ts`

- DR Node: `system.ts`. DR Rust: `platform/system.rs`.
- Changed lines: 8 (+6 −2). Row: RA9.

Behavior to carry:

- Lines 5–9: `VC_RUNTIME_FILES` lives here and is `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`, `MSVCP140.dll` (DR, in
  `core/pins.rs`: `MSVCP140.dll`, `VCRUNTIME140_1.dll`). A missing file is a warning with a download link, never a
  reason to refuse Play: the runtime links its C runtime statically, so only the game's needs count.
- Drop `normalize_device_signals` and its tests from `platform/system.rs` (see `hostapi.ts`).

## `verify.ts`

- DR Node: `verify.ts`. DR Rust: `net/verify.rs`.
- Changed lines: 172 (+30 −142). Row: RA7.

Behavior to carry:

- DR lines 40–151, 168–194 removed: manifest verification (`checkFile`, `verifyInstall`, `removePartFiles`,
  `VerifiedCache`). It checked the 1.4.4 game against its manifest: not ported (RD5). `hash_file` and `Abort` stay.
- Lines 39–93, `HashCache`: remembers `{size, mtimeMs, sha256}` per absolute path, for the game exe so it is hashed
  once per change rather than on every start.
  - `cached(file)`: the stored hash if size and mtime still match, else null.
  - `hash(file)`: hashes and stores only if size and mtime were the same before and after reading; at most 32
    entries, the oldest dropped.
  - Saved as `{"files": {"<path>": {size, mtimeMs, sha256}}}` through a `.tmp` file and rename; entries without a
    64-hex hash are dropped on load.
  - Play hashes the exe afresh (the cache serves the status display); the hash is checked against
    `PINNED_EXE_SHA256`.

## `runtime-files.ts` (new)

- DR Node: none. DR Rust: none; a new module next to `net/downloader.rs`. Row: RA7.

What the Rust version must do:

- `runtimeSetId(files)`: SHA-256 hex of the lines `name:size:sha256`, sorted by name, joined by `\n`. This id is
  what `trustedRuntimes` stores and must match Reforged's so accepted runtimes stay accepted.
- `installedRuntime(dir)`: for each runtime name, the SHA-256 of the file, or null when missing, not a file or empty.
  Hashed afresh each time.
- `checkRuntime(win64Dir, manifest, trusted, localDir)` gives a state and whether it is playable:
  - A local runtime folder in use: `local`, playable when both files are there.
  - No manifest from the server: `ok` if both files are present (one build serves every server), else
    `unavailable`.
  - Installed hashes equal the manifest: `ok`. Not trusted: `untrusted`. Else `missing` (none present) or
    `outdated`.
- `installRuntime`, for each manifest file:
  - Old files moved aside earlier are swept first. A file whose hash already matches is skipped.
  - Download to `<target>.rr-download` with `requestToFile`
    (`/content/v1/runtime/files/<name, URL-encoded>`, the key headers, 30 s idle timeout, `maxBytes` = the
    manifest size).
  - Errors: 401/403 `key_rejected`; 404 `not_found`; other status or network `download_failed`; pin
    `cert_mismatch`; too large `different`; `ENOSPC`, `EACCES`, `EPERM` `disk_error`; abort `cancelled`.
  - Size and hash checked before the replace; mismatch is `different`.
  - Replace with `replaceFile`, then clear the `Zone.Identifier` stream and hash the target again
    ("changed after copying (antivirus?)").
  - Progress reports bytes and files. At the end the marker `.reforged-runtime.json` (note, version, files,
    `installedAt`) is written, best effort.
- `replaceFile(tmp, target)`: rename over the target; if that fails (a running Ramsgate or hunt server on a host's PC
  has the DLL loaded), rename the old file to `<target>.rr-old-<ms>`, rename the new one in, then try to delete the
  old one; if the second rename fails, put the old file back. Returns `in_use` when the old file had to be moved;
  the controller reports those names (`replacedInUse`).
- `sweepAside(dir)`: delete files named `<runtime name>.rr-old-*`, ignoring failures.
- `installLocalRuntime(localDir, win64Dir)`: copy both files from a host's own build the same way (skip when the hash
  matches, copy to `.rr-download`, re-hash, `replaceFile`, unblock).
- `isCompleteRuntimeDir(dir)`: both files present and non-empty.
- On Linux `unblock` is a no-op (no alternate streams); the Win64 folder is the game's inside the Proton prefix
  (RA10).

## `hostkit.ts` (new)

- DR Node: none. DR Rust: none; a new module. Row: RA12 (Windows; Linux is RA12a).

What the Rust version must do (behavior as written for today's Node host kit; see Open points for what RD1, RD3 and
RD8 change):

- Workspace layout, lines 6–17 and 92–115: `ReforgedHost\package.json` marks a workspace;
  `.workspace\Mystic-Paradox\` holds `ParadoxRuntime\` (`dllmain.cpp`, `SDK.hpp`, `SDK\`, `MysticParadox.sln`,
  `x64\Release\MysticParadox.dll`), `tools\RuntimeLoader\target\release\winmm.dll`, `tools\CatalogExporter\`
  (`.vcxproj`, `x64\Release\CatalogExporter.dll`), `ParadoxBackend\game-data`, `ParadoxDirector\game-data`,
  `Items_Analysis\`, and `.reforged-patches.json` as the patch marker; `.workspace\runtime\` is what the content
  server publishes; `.workspace\logs\` holds build logs; `.workspace\config\host.json` the server's config.
- Lines 28–46: the seven backend and seven Director game-data file names.
- Lines 50–66: `EXPORT_FLAGS`, the CatalogExporter flags file written to the game's working directory, CRLF, exactly
  these keys and values.
- Lines 133–159: `countFunctionFiles` (`*_functions.cpp` in `SDK\`); `detectGameData`: `missing` if any file is
  absent, `synthetic` if any equals its `*.example.json` byte for byte, else `real`.
- Lines 161–181: `findDumps(root = C:\Dumper-7)`: at most 200 entries, each `<name>\CppSDK` with an `SDK.hpp`,
  newest first, with its modification time.
- Lines 183–237: `findToolchain` (Windows only): `vswhere -latest -products * -requires
  Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json -utf8` for the name, version and major version;
  `vswhere … -requires Microsoft.Component.MSBuild -find MSBuild\**\Bin\MSBuild.exe` for MSBuild; `cargo.exe` in
  `%USERPROFILE%\.cargo\bin` or `%CARGO_HOME%\bin`. `platformToolset`: `v145` for Visual Studio 18 and later, else
  `v143`. `msbuildArgs`: `/m /p:Configuration=Release /p:Platform=x64 /p:PlatformToolset=<t> /nologo
  /verbosity:minimal`. External commands time out after 20 s with 4 MiB of output.
- Lines 241–260: `readServerConfig`: `host.json`'s `name` (cut to 80), `mode` (`private`/`public`, else not
  configured), `host` (cut to 255), and for private servers `multipliers` `{xp, rewards}`, each finite, from 1 to
  `MAX_MULTIPLIER`, left out when both are 1.
- Lines 262–293: `detectHostKit` assembles the page's view from the checks above.
- Lines 298–322: `copyTree` (files and folders only, links skipped); `importSdk(dumpDir, runtimeSource)` requires
  `SDK.hpp` in the dump and `dllmain.cpp` in the source, copies the tree, and fails if no `*_functions.cpp` arrived.
- Lines 330–380: game-data pack import: gzip JSON with `format` `dauntless-reforged-revival-gamedata`, `version` 1,
  `game` `1.14.7 (647472)`, `files` of `{path, sha256, data}`. Only the paths from `gameDataPackEntries` (backend,
  director, `backend/raw/progression_config.source.json`, `backend/raw/ladyluck_store.source.json`). The whole pack
  is checked first: errors are `pack_invalid`, `pack_game`, `pack_damaged` (hash over the UTF-8 text, and the text
  must parse as JSON). An existing different file is renamed `<name>.<ISO time with : and . as ->.bak`; identical
  files count as unchanged.
- Lines 382–393: `useSyntheticData`: copy each `*.example.json` only where the real file is missing.
- Lines 395–421: `publishRuntime`: copy the two built DLLs into `.workspace\runtime\` (via `.tmp` and rename), hash
  them, and write `runtime.json` `{version: "<yyyymmdd>-<first 8 hex of sha256(hashes joined by :)>", build:
  {changelist: 647472}, files}`. RD3 lists `publish-runtime` as a `reforged-host` command, so this may move there.
- Lines 424–459: `runLogged`: run a build tool with output to a log file (header line with the command and folder,
  footer with the exit code); return the code and the last non-empty output line (at most 300 characters).

## `hostserver.ts` (new)

- DR Node: none. DR Rust: none; a new module. Row: RA12.

What the Rust version must do:

- Lines 18–24: the host kit contract: `REFORGED_CONTROL=stdin` makes `start` read a `stop` line on stdin; the line
  `REFORGED_SERVER_READY` on stdout means running (it is not shown); something listening on 127.0.0.1:61000 (the
  backend) means a server runs; stop waits 45 s before ending the tree; the page keeps the last 300 lines.
- Lines 39–59: `findNode` on `PATH`, `%ProgramW6432%`/`%ProgramFiles%\nodejs`, `NVM_SYMLINK`, with npm's
  `npm-cli.js` beside it. Under RD3 this becomes finding `reforged-host` (see Open points).
- Lines 62–74: `cleanLine` strips ANSI escapes and carriage returns, trims the end, cuts to 400; `failureLine` takes
  the last `Error: …` line, else the last non-empty one, cut to 300.
- Lines 96–106: `hostKitNeedsBuild`: `dist\cli.js` missing or older than any file under `src\` (Node host kit only).
- Lines 188–216: page updates at most every 250 ms, immediately on a state change.
- Lines 224–234: child environment: `NO_COLOR=1`, `FORCE_COLOR=0`, node's folder first on `PATH`, `npm_execpath`
  set, every `ELECTRON_*` removed.
- Lines 237–272: run a program with stdin, stdout and stderr piped, `windowsHide`, output appended to a log file
  with a header, split into lines for the page.
- Lines 276–313: `prepare` (npm install when `node_modules` is missing or asked, then `npm run build`, one at a time;
  errors `workspace`, `node_missing`, `npm_missing`, `hostkit_build`); `command(workspace, args, logName)` runs a
  one-shot command (`setup`, `gamedata`, `invite`) in `ReforgedHost` with `INIT_CWD`, logging to
  `.workspace\logs\launcher-<logName>.log`.
- Lines 316–324: `program` runs another tool (git) the same way with `GIT_TERMINAL_PROMPT=0`.
- Lines 327–369: states `stopped`, `starting`, `running`, `stopping`, `failed`, `external`. `refresh` and `start`
  probe port 61000 (700 ms); an answer there that the launcher did not start is `external` and `start` does nothing.
  `start` logs to `launcher-server.log`; the process exiting is `stopped` if a stop was asked, else `failed` with the
  failure line or `exit <code>`.
- Lines 372–393: `stop` writes `stop\n`, closes stdin, waits up to 45 s, then ends the tree (`taskkill /PID <pid> /T
  /F` on Windows, `SIGKILL` elsewhere) and waits up to 5 s more. Closing the launcher closes stdin, which also stops
  the server.

## `requirements.ts` (new)

- DR Node: none. DR Rust: none; a new module. Row: RA12.

What the Rust version must do: report, in this order, each requirement as `ok`, `missing` or `problem` with a short
detail and a problem code (lines 83–124):

- `dauntless`: from the game check.
- `git`: `git.exe` on `PATH` or `<ProgramW6432|ProgramFiles|LOCALAPPDATA\Programs>\Git\cmd`; detail from
  `git --version` without `git version`, cut to 40 (Windows only).
- `node`: `findNode`; `problem`/`too_old` below 20.19 (lines 13, 26–31).
- `mongodb`: `mongod.exe` on `PATH` or the newest `%ProgramFiles%\MongoDB\Server\<version>\bin` (numeric sort);
  `problem`/`service_running` when `sc query MongoDB` says `RUNNING` (the installer's service holds 27017 without a
  replica set).
- `tailscale`: found or not.
- `visual_studio`: `ok` with the C++ workload; `problem`/`no_cpp` when `vswhere` finds a Visual Studio without it.
- `rust`: `cargo` found.
- `dumper7`: the picked DLL exists; detail its file name.
- `vc_redist`: no missing `VC_RUNTIME_FILES`.

Which of these stay after RD3, RD8 and RD15 is in Open points.

## `injector.ts` (new)

- DR Node: none. DR Rust: none; a new module on the `windows` crate (RD9). Row: RA12; Linux is RA12a; checklist RX3.

What the Rust version must do (today a PowerShell script with a C# helper, lines 15–81):

- Refuse unless on Windows, the pid is a positive integer and the game this launcher started, and the DLL path is
  absolute and ends `.dll` (lines 112–115); fail `dll_missing` if the file is not there.
- Wait for the process to show a main window, polling every 500 ms for up to 180 s (`waitWindowSeconds`), then wait
  15 s more (`extraDelaySeconds`); fail `process_gone` if it exits at either point.
- Inject: `OpenProcess` with `PROCESS_CREATE_THREAD | QUERY_INFORMATION | VM_OPERATION | VM_WRITE | VM_READ`;
  `VirtualAllocEx` (`MEM_COMMIT | MEM_RESERVE`, `PAGE_READWRITE`) for the UTF-16 path with its NUL;
  `WriteProcessMemory`; `CreateRemoteThread` at `kernel32!LoadLibraryW`; wait up to 60 s; a thread exit code of 0 is
  `load_failed`. Free the memory and close every handle on every path.
- Report progress as events `waiting` (`window`, then `settle`), `injected` (the path) or `failed`
  (`open_process_failed:<err>`, `alloc_failed:<err>`, `write_failed:<err>`, `no_loadlibrary`, `thread_failed:<err>`,
  `load_failed`, `windows_only`, `bad_pid`, `bad_dll`, `no_answer`); details cut to 300. The whole call is bounded at
  10 minutes.

## `revived/` (not ported, RD5)

| Reforged file | One line | DR counterpart |
| --- | --- | --- |
| `revived/api.ts` | 0.1.35's game manifest, hunt region and end-of-session calls | `hostapi.ts`, `autoregion.ts` |
| `revived/constants.ts` | 0.1.35's 1.4.4 pins (exe, DLLs, manifest build) | `constants.ts` |
| `revived/dauntless-1.4.4.json` | The 1.4.4 file manifest (2,056 lines) | `UndauntedContent/data/dauntless-1.4.4.json` |
| `revived/detect.ts` | Finds a 1.4.4 already downloaded by DR's launcher | none |
| `revived/dlls.ts` | Downloads and pins `dxgi.dll` and `UndauntedInternalServer.dll` | `dlls.ts` |
| `revived/downloader.ts` | 1.4.4 download from a DR content server | `downloader.ts` |
| `revived/install-check.ts` | 1.4.4 folder check against the manifest | `verify.ts` |
| `revived/launch.ts` | The 1.4.4 command line | `launch.ts` |
| `revived/manifest.ts` | The compiled-in 1.4.4 manifest | `manifest.ts`, `game-manifest.ts` |

## DR files with no Reforged counterpart

Listed so RA1's copy does not bring them in by accident.

| DR Node file | DR Rust | Disposition |
| --- | --- | --- |
| `autoregion.ts` | `net/autoregion.rs` | Not ported (RD5, RA1) |
| `client-mods.ts` | `platform/client_mods.rs` | Not ported (RA1) |
| `device-signals.ts` | `platform/system.rs` (`normalize_device_signals`) | Removed (Reforged dropped it) |
| `dlls.ts`, `manifest.ts`, `game-manifest.ts` | `core/manifest.rs`, `core/manifest_check.rs`, `core/pins.rs` | 1.4.4 only (RD5) |
| `downloader.ts` | `net/downloader.rs` | See Open points |
| `relay-owner.ts` | `net/relay_owner.rs` | See Open points |
| `windows-runtime.ts` | — | DirectX file check; Reforged dropped it with the DirectX link |
| `linux-runtime.ts`, `linux-dependencies.ts` | `platform/linux_runtime.rs` | Kept for Linux (RD4, RA10) |
| `linux-updater.ts` | `platform/updater.rs` | RA14 |
| `local-stack*.ts`, `offline-*.ts` | `offline/`, `controller/offline.rs` | Not ported (RA1) |
| `bridge.ts` | — | DR's own Electron bridge; Reforged writes its own (RA15) |

## Open points

1. **Relay owner check.** RA6 says "owner check", and DR's Rust relay has `relay_owner.rs` (only connections from the
   game's process are served). Reforged removed it (DR `relay.ts` lines 53, 167–214). Decide whether the Rust relay
   keeps DR's check (stricter than Reforged today, and it must find the game's process under Proton too) or drops it
   to match Reforged.
2. **`/__origin/` in the relay.** Section 2 says the relay does `/__origin/` routing. Reforged's relay forwards paths
   unchanged; the gateway and backend unwrap them. The plan's wording should say "passes through" unless the relay is
   meant to change.
3. **Downloader.** Section 2 reuses `net/downloader.rs` for "the runtime's two files only". Reforged does not use a
   downloader for them: `runtime-files.ts` streams each file with `http.requestToFile`. Either RA7 writes a small
   `request_to_file` in `net/http.rs` and drops `downloader.rs` (914 lines with pause, resume, backoff and a speed
   meter that two small DLLs do not need), or it keeps `downloader.rs` and accepts the extra code. Recommendation:
   the first.
4. **Stored 1.4.4 servers.** Settings can hold servers with `game: "revived"`, keys for them and `revivedRoot`. RA15
   warns before 2.0.0; RA8 must decide whether 2.0.0 drops those entries on load, keeps them hidden, or shows them
   disabled with the RD5 message. Their keys should stay on disk either way.
5. **Host page under RD1, RD3 and RD8.** `hostkit.ts`, `hostserver.ts` and `requirements.ts` are written against
   today's layout: a git clone with `ReforgedHost` (Node, built with npm), `.workspace\Mystic-Paradox` from
   `npm run upstream`, MongoDB, and the host kit started as `node dist/cli.js`. After RD1 the runtime source is in
   `native/` and `paradox/`; after RD3 the commands are one `reforged-host` binary; after RD8 that binary comes from a
   release, not a build; after RD15 the database is Postgres. RA12 needs to know which layout it targets: the
   paths, the host-kit build step, `findNode`, the `node` and `mongodb` requirements (Postgres instead), and
   whether `rust` (cargo, for `RuntimeLoader`) stays a requirement. The process contract (`REFORGED_CONTROL=stdin`,
   `REFORGED_SERVER_READY`, port 61000 probe) should be kept by `reforged-host` (RH rows).
6. **`publishRuntime` placement.** RD3 lists `publish-runtime` as a `reforged-host` command; the Host page does it in
   the launcher today. Decide whether RA12 calls the command or keeps its own copy (the version format and
   `runtime.json` must be the same in both).
7. **Linux for files Reforged made Windows-only.** `launch.ts` lost DR's `LaunchRuntime` and the `pgrep` check;
   `game-folder.ts` store detection, `engineini.ts`'s config folder and `runtime-files.ts`'s `unblock` are written for
   Windows. RA9 should keep DR's Linux branches as they are and leave the Linux values (prefix paths, Heroic and
   Legendary on Linux) to RA10.
8. **Keystore format tag and folder.** Reforged keys use `RRK2`; nothing on a Reforged PC uses `DRK2`, so the Rust
   reader needs `RRK2` and bare keys only. The `userData` folder, the recovery folder's name in Documents and the
   scheme rename (`dauntless-revived` to `reforged-revival` in `platform/invite_link.rs`) are set in `main.ts`, which
   is in the controller family's delta file; RA8 and RA11 need both files.
9. **Usernames in settings.** Reforged validates stored usernames with the registration rule; DR kept any clean
   display text for names an admin renamed. If the Rust backend can rename users (RB rows), DR's looser rule is
   safer. Keep Reforged's unless that is decided.
