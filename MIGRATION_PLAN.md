<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Dauntless Reforged Revival migration plan: one repository, Rust around a forked Mystic Paradox

Status: draft, written 2026-10-22; the decisions in section 3 were made the same day. No work rows have started.

This plan replaces Dauntless Revived's Rust migration, which stopped on 2026-10-22 at `e1c5fec` on its `plan` branch
([its plan](https://github.com/mixutin/dauntless-revived/blob/plan/MIGRATION_PLAN.md)) and is not continued.
Most of what it built was written for code this repository already shares: Reforged's launcher, gateway, allowlist
helper and content server all started as Dauntless Revived's. This plan takes that Rust code over and fits it to
1.14.7, rather than porting Reforged's Node code a second time. Mystic Paradox stops being a pinned upstream with
patches and becomes part of this repository.

Row ids: **RA** = launcher, **RS** = gateway, allowlist and content, **RH** = host kit, **RP** = the Paradox fork,
**RR** = repository and CI, **RX** = cross-cutting. **RD** = decisions. "DR" means Dauntless Revived at `e1c5fec`;
DR row ids (L27, S2, X4, …) point into its plan, where the method and the checks for each piece are written down.

## 1. End state

| Component | Today | After |
| --- | --- | --- |
| `ReforgedLauncher` | Electron 44, Node main process (~8.2k lines with `main.ts` and `preload.ts`), Squirrel, Forge, Windows only | Tauri 2 on DR's launcher backend (`src-tauri`, ~20k lines), the same TS page, isolation pattern, `tauri-plugin-updater`; Windows (NSIS) and Linux (AppImage, deb, rpm) (RD4) |
| `ReforgedGateway` (+ allowlist helper) | Node, `node-forge` (~2.8k lines) | DR's Rust `gateway` (bins `gateway`, `allowlist`) with Reforged's routes, ports and nft changes |
| `ReforgedContent` | Node (~1.4k lines) | DR's Rust `content` with Reforged's runtime manifest added |
| `ReforgedHost` | Node TS CLI (~4.8k lines with `linux/`) | Rust bin `reforged-host`, same commands (RD3) |
| ParadoxBackend, ParadoxDirector | Mystic Paradox at a pinned commit + 33 patches, downloaded by `setup` | Our own Node code in this repository, with Paradox's history (RD1); patched by ordinary commits |
| ParadoxRuntime, RuntimeLoader (`MysticParadox.dll`, `winmm.dll`) | C++, patched upstream, built by each host | The same, in `native/`; still built by each host from their own SDK |
| `upstream/` | Pin + patches + `store-captures.json` | Gone; `store-captures.json` moves next to the backend, attribution in `NOTICE.md` |
| Releases | Launcher (Squirrel) only | Launcher (Tauri feed) and a server release: Rust binaries plus the built backend and Director, for Windows and Linux (RD8) |

## 2. What is reused from DR

Measured 2026-10-22 by diffing Reforged's Node sources against DR's at `e1c5fec`. "Delta" is the Reforged change
that has to be carried onto the Rust code; line counts are changed lines in the Node files, a rough guide only.

| DR piece (at `e1c5fec`) | Reforged counterpart | Reuse | Delta to carry over |
| --- | --- | --- | --- |
| `rust/crates/protocol` (invite, status, manifest, account) | `src/shared/invite.ts`, `status.ts`, `types.ts` | Most | Second scheme `reforged-revival://` beside `dauntless-revived://`; `RRK_` keys; a flavor on status and register (Reforged's `hostapi.ts` already has `REVIVED_FLAVOR`); the runtime manifest type |
| `rust/crates/proxy` (hop-by-hop, WebSocket tunnel) | relay and gateway proxying | Whole | None expected; `/__origin/<host><path>` rewriting stays in the relay |
| `rust/crates/service-kit` (axum setup, pino logs, env config, the X7 service contract) | gateway, content and host kit start-up | Whole | None |
| `rust/crates/store` (Diesel, SQLite) | none: Reforged's data is in MongoDB | Not brought in | — |
| `rust/services/gateway` (TLS, policy, IP checks, limits, redaction, feed, `cert.rs`, `allowlist` bin) | `ReforgedGateway/src` (allowlist `state`, `sync`, `ip`, `ratelimit`, `redact`, `log` identical) | Most | `policy.ts` (~108 lines: `/reforged/api` admin routes, Paradox's host-only routes and headers), `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`; new `allowlist/nft.ts` and `ports.ts` (the game's UDP range from `ALLOWLIST_PORTS`, which `--max-hunts` widens); `tools/make-cert.js` becomes `gateway cert` |
| `rust/services/content` (ranges, news, branding, auth) | `ReforgedContent/src` (`branding`, `news`, `limits`, `log` identical) | Most | New `runtime.ts` (the two-file runtime manifest and download); `app.ts` (~218 lines: no 11 GB game zip), `auth.ts` against the backend's key check, `config.ts` |
| `rust/services/metagame`, `deploy`, `discord-bot` | ParadoxBackend, ParadoxDirector, none | Not brought in (RD2) | — |
| DR's server kits' Rust subcommands and `server-release.yml` (M38, S6) | `ReforgedHost`, `ReforgedHost/linux/` | Method and release workflow | The host kit's commands are Reforged's own (RH rows) |
| Launcher `src-tauri`: `net/pinned.rs`, `net/http.rs`, `net/relay.rs`, `net/hostapi.rs`, `net/verify.rs`, `net/downloader.rs` | `pinned.ts` (identical), `http.ts`, `relay.ts`, `hostapi.ts`, `verify.ts`, `revived/downloader.ts` | Most | Relay: port 61100, `/__origin/` routing, `/QoS` answered locally; downloader: also the runtime's two files; verify: the 1.14.7 executable hash |
| Launcher `platform/keystore` (Windows DPAPI, Linux Secret Service and KWallet, `legacy.rs` safeStorage reader; DR L27, D7) | `keystore.ts` (~14 lines changed) | Whole | One key per server, bound to the certificate fingerprint; Reforged's `userData` folder and app name |
| Launcher `platform/linux_runtime.rs` (Proton and Wine, DR L32) | none | Whole | Run 1.14.7 under Proton with the runtime's `winmm.dll` loaded (`WINEDLLOVERRIDES`), `REFORGED_BACKEND_URL` passed through, Engine.ini/Game.ini inside the prefix (RA10) |
| Launcher `platform/settings.rs`, `log.rs`, `system.rs`, `window.rs`, `invite_link.rs`, `game_folder.rs`, `game_config.rs`, `launch.rs` | `settings.ts`, `log.ts`, `system.ts`, `links.ts`, `game-folder.ts`, `engineini.ts`, `launch.ts`, `display-repair.ts` | Most | Engine.ini XMPP section and Game.ini `[OnlineSubsystemPhoenix]` removal; the 1.14.7 command line and `REFORGED_BACKEND_URL`; Legendary/Heroic/Epic detection |
| Launcher `controller/`, `bridge/` (typed commands, generated TS types, isolation hook) | `controller.ts` (~2.2k lines changed), `ipc-validate.ts`, `preload.ts` | Structure | The largest delta: Host page state, runtime acceptance, two game flavors |
| Launcher `platform/updater.rs`, Squirrel bridge (DR L45–L50), Linux bundles (L47) | `update-electron-app` + Squirrel | Whole | Feed URL and version line (RD7) |
| Launcher `net/autoregion.rs` | `revived/api.ts` (hunt region) | Whole | Only for the Dauntless Revived flavor |
| Launcher `offline/`, `controller/offline.rs` | none | Not brought in: Reforged has no offline mode | — |
| Launcher `platform/client_mods.rs` (UE4SS mods for 1.4.4) | none | Not brought in | Reforged's DR flavor plays 0.1.35's setup, which has no client mods |
| Launcher page split per language (DR L13–L15) | `src/shared/i18n.ts` (1.6k lines, en + pt-BR) | Method | Two languages instead of nine |
| CI by family, `cargo deny`, fuzz targets, `SECURITY_REVIEW.md` checklists (DR R8, X6, X9) | `ci.yml`, `launcher-release.yml` | Most | No metagame or bot jobs; a Node job for the Paradox fork |

Not in DR at all, so written new in Rust: the Host page backend (`hostkit.ts`, `hostserver.ts`, `requirements.ts`,
~980 lines), the DLL injector (`injector.ts`, RD9), the runtime files (`runtime-files.ts`, ~290 lines) and the
whole host kit (RH rows).

## 3. Decisions

Made 2026-10-22.

| # | Topic | Decision | Consequence |
| --- | --- | --- | --- |
| RD1 | Mystic Paradox | **Forked into this repository with its history**: Paradox's repository is merged in as a subtree at `paradox/` from the pinned commit `00d3be8`, then the 33 patches are applied as one commit each, in order, with their patch text as the message. ParadoxRuntime and RuntimeLoader then move to `native/` with `git mv`. From then on Paradox is changed by ordinary commits; `upstream/`, `npm run upstream` and setup's download step go away. Paradox's `ADDITIONAL_TERMS.md` stays with its code and its attribution moves into `NOTICE.md` | Hosts no longer download anything from Paradox's repository. Upstream fixes can still be merged in from Paradox's history when they are wanted; nothing forces it |
| RD2 | Backend and Director language | **Stay Node and MongoDB**, now as our code. DR's Rust metagame (SQLite, 1.4.4 routes) does not fit them | Hosts still need Node.js and MongoDB, but no Git or npm once the server release ships them built (RD8) |
| RD3 | Host kit | **Ported to Rust** as `rust/services/host`, bin `reforged-host`, with the same commands (`setup`, `start`, `invite`, `doctor`, `gamedata`, `preset`, `multipliers`, `publish-runtime`, `update`) and the same files it writes (`host.json`, the `.env` files). It starts the Rust services and the Node backend and Director under one process tree (Windows job object, Linux process group). `ReforgedHost/linux/` (`setup-vps.sh`, `Deploy-FromWindows.ps1`) is reduced to fetching the release and calling `reforged-host` | One binary drives the whole server; the Host page runs it instead of `node dist/cli.js` |
| RD4 | Launcher platforms | **Windows and Linux.** DR's Linux code comes with it: Proton/Wine runtime, Secret Service and KWallet keystore, WebKitGTK 4.1 (DR D3), AppImage, deb and rpm. The Host page (SDK, game data, injector) stays Windows-only in the first Linux release | 1.14.7 and the runtime have to be shown to work under Proton first (RA10, a spike); Linux CI and release jobs |
| RD5 | DR's 1.4.4 servers | **Kept**: `dauntless-revived://` invites play 1.4.4 with launcher 0.1.35's pins (`src/main/revived/`). The flavor is a field of the stored server, not a second code path through the controller | `protocol` gains the flavor; DR's 0.1.39 pins and client mods are not used |
| RD6 | How DR's Rust code comes in | **Snapshot copy** at `e1c5fec` in one commit, with the source commit in the message and in `NOTICE.md`. DR is not continued, so nothing flows either way afterwards | From the copy on, the code is Reforged's own |
| RD7 | Launcher updates | DR's design (D2): `tauri-plugin-updater` with ed25519 signatures. Existing installs take a one-time bridge, Electron **1.6.0**, which downloads the Tauri **2.0.0** installer, checks its SHA-256 compiled into the bridge, runs it and removes the Squirrel install. The Squirrel feed is retired with 3.0.0; the Electron key copies are kept until then so a rollback works | One extra Electron release; 1.5.x installs that skip it before 3.0.0 reinstall by hand |
| RD8 | Server binaries | **Released by CI**: a server release workflow (from DR's `server-release.yml`) builds `gateway`, `allowlist`, `content` and `reforged-host` for `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu`, plus the built backend and Director with their production `node_modules`, in one archive per OS, with `SHA256SUMS.txt` and provenance attestations. `setup` and `update` download and check it | No Rust toolchain on hosts; `update` stops being a `git` fast-forward of a checkout |
| RD9 | DLL injector | **Rust on the `windows` crate** (`OpenProcess`, `VirtualAllocEx`, `CreateRemoteThread(LoadLibraryW)`), only into the game process the launcher started, only a DLL the host picked | No PowerShell or C# step; its own security checklist (RX3) |
| RD10 | Security | **DR's full pass** (D26, X9): a checklist per trust-boundary component in `SECURITY_REVIEW.md`, `cargo-fuzz` targets for every parser of untrusted input run briefly on every PR, a 24-hour fuzz run and a signed checklist before each component's cutover | Replaces a second reviewer where a mistake costs most |
| RD11 | Repository layout | **DR's monorepo** (section 4.1): `rust/` (one cargo workspace), `native/` (C++ runtime), `paradox/` (the Node backend and Director), `legacy/` for the Node projects being replaced until each is deleted; `preset/`, `brand/`, `docs/` stay | Moves use `git mv` so history follows |
| RD12 | Order | **Both tracks in parallel**, launcher and servers, synced on the `protocol` crate (RS1) and on one shared check: the Tauri launcher against the Rust gateway and content (section 5) | Faster; each track's cutover still waits for its own checks |
| RD13 | Languages | en and pt-BR, as today, split one file per language as DR did (L13), so a third language is data only | — |

## 4. Work

### 4.1 Repository and CI (RR)

Target layout once the Node projects are gone:

```text
rust/                    one cargo workspace
  crates/protocol, proxy, service-kit
  services/gateway       bins gateway, allowlist
  services/content       bin content
  services/host          bin reforged-host
  launcher/src-tauri     launcher backend; launcher/ui: the TS page
native/
  paradox-runtime/       MysticParadox.dll (was ParadoxRuntime)
  runtime-loader/        winmm.dll (was RuntimeLoader)
paradox/
  backend/               ParadoxBackend (Node, MongoDB)
  director/              ParadoxDirector (Node)
preset/  brand/  docs/  .github/
```

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RR1 | Copy `rust/` (workspace files, `rust-toolchain.toml`, `deny.toml`, `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`, `crates/protocol`, `crates/proxy`, `crates/service-kit`, `fuzz/` targets for those crates) in one commit (RD6) | R4, X6 | `cargo test --workspace`, `cargo deny check`, `cargo clippy` pass on Windows and Linux |
| RR2 | `git mv` `ReforgedGateway`, `ReforgedContent`, `ReforgedLauncher`, `ReforgedHost` to `legacy/` | R5 | Today's npm tests pass from `legacy/` |
| RR3 | CI by family behind one required check: Rust (fmt, clippy, test, deny, short fuzz) on Windows and Linux, the launcher page, the Paradox fork's Node tests, legacy Node | R8 | A Rust-only PR skips the Node jobs and the other way round |
| RR4 | Dependabot entries for `cargo` (`/rust`), the page and `paradox/*`, under the current policy; each npm entry under `legacy/` removed with its folder | X6 | Dependabot opens a `cargo` PR |
| RR5 | Move the launcher page to `rust/launcher/ui` once Electron is deleted (RA16); path sweep for `legacy/` and `upstream/` | R7, R10 | No reference to a deleted path |

### 4.2 The Paradox fork (RP)

| # | What | Verify |
| --- | --- | --- |
| RP1 | Merge Paradox's history as a subtree at `paradox/` from `00d3be8` | `git log paradox/` shows Paradox's commits; the tree equals the pinned archive |
| RP2 | Apply patches 0001–0033 as 33 commits, in order | The tree equals what `npm run upstream` produces today (`.reforged-patches.json` set) |
| RP3 | `git mv` the backend and Director to `paradox/backend`, `paradox/director`, the runtime and loader to `native/`; Paradox's top-level files (`ADDITIONAL_TERMS.md`, licence, docs) stay under `paradox/` | The backend and Director tests pass; the runtime builds from `native/` with a host's SDK |
| RP4 | `ReforgedHost` (still Node) builds from `paradox/` instead of downloading; delete `upstream/` and `npm run upstream`; move `store-captures.json` to `paradox/backend`; attribution in `NOTICE.md` and the READMEs | `npm run setup` on a clean host with no network access to Paradox's repository |

### 4.3 Gateway, allowlist and content (RS)

| # | What | From DR | Delta | Verify |
| --- | --- | --- | --- | --- |
| RS1 | `protocol`: Reforged invite scheme, `RRK_` keys, flavor on status and register, runtime manifest type | X4 | §2 | DR's protocol tests plus Reforged's `invite.test.ts` cases as Rust tests |
| RS2 | Content server: copy `services/content`, drop the game zip, add the runtime manifest and the two-file download, auth against the backend's key check | S1 | `runtime.ts`, `app.ts`, `auth.ts`, `config.ts` | Reforged's content tests ported; the Electron launcher downloads a runtime from the Rust server |
| RS3 | Gateway: copy `services/gateway`, apply `policy.ts` (Paradox and `/reforged/api` admin routes, host-only headers), config, feed and server deltas; `gateway cert` replaces `tools/make-cert.js`; **load the existing certificate, never regenerate** | S2 | `policy.ts`, `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`, `make-cert.js` | Reforged's gateway tests ported; DR's parity test re-pointed at Reforged's Node gateway; an existing invite's `fp` still matches |
| RS4 | Allowlist helper: DR's `allowlist` bin with Reforged's game-port range (`ports.ts`) and its nftables variant (`nft.ts`) for the Linux VPS | S3 | `ports.ts`, `nft.ts`, `config.ts`, `firewall.ts` | Rule contents after login and logout on Windows and on Debian 13 (nft) |
| RS5 | Catch-up of Reforged's Node gateway and content commits since the copy (RX2), then delete `legacy/ReforgedGateway` and `legacy/ReforgedContent` | X3 | — | Every commit on the list ported or marked not applicable |

### 4.4 Host kit (RH)

| # | What | Verify |
| --- | --- | --- |
| RH1 | `services/host` skeleton on `service-kit`: `paths`, `envfile`, `hostconfig` (`host.json`), `clap` commands with today's names and options | Reforged's host kit tests for config and paths ported; a `host.json` written by the Node kit reads back unchanged |
| RH2 | `start`: the Rust services with `--env-file` (DR X7), the Node backend and Director, MongoDB when managed, the allowlist helper in public mode; one process tree, clean stop on Ctrl+C including the game servers | Every service up; nothing left running after stop, on Windows and Linux |
| RH3 | `setup`, `invite`, `doctor`, `multipliers`; MongoDB replica-set setup (`mongo.ts`) | A fresh private and a fresh public setup on Windows |
| RH4 | `gamedata`, `preset`, `progression`, `publish-runtime`, `gamepack`, `captures` | The generators give byte-identical files to the Node kit's from the same export |
| RH5 | `update` and setup's install step from the server release (RD8); `linux.ts`, `wine.ts` and `ReforgedHost/linux/` on the Linux release | `docs/hosting-linux.md` followed on a clean Debian 13; an existing Node-kit host moves over with `update` |
| RH6 | Server release workflow (RD8) | DR M38 | A run without publishing; its archives install on Windows and Debian 13 and pass the smoke list (section 6) |
| RH7 | Delete `legacy/ReforgedHost` | — | — |

### 4.5 Launcher (RA)

Phases follow DR's Part A. The DR rows in brackets hold the method; the Reforged work is the delta.

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RA1 | Copy `src-tauri` without `offline/`, `controller/offline.rs`, `client_mods.rs`; the app's identifier, name and `userData` folder are Reforged's | L4–L7 | It builds on Windows and Linux and opens today's page with a stub bridge |
| RA2 | Bridge, generated TS types and isolation hook re-generated for Reforged's command list (`ipc-validate.ts`, `preload.ts`) | L6, L7, L9 | Every command the page uses is in the bridge spec; a refused command never reaches Rust |
| RA3 | Language files split (en, pt-BR) and the backend's text built from them | L13–L15 | Reforged's `i18n.test.ts` passes on the split files |
| RA4 | Invite, status, username on `protocol` (RS1) | L11, L12 | Shared test tables with the servers |
| RA5 | Pinned TLS, HTTP client, host API with both flavors | L19–L21 | Reforged's `pinned.test.ts` cases; register and status against the Rust gateway |
| RA6 | Relay on 61100 with `/__origin/` routing, chat WebSocket and local `/QoS`; owner check | L22, L33 | Reforged's `relay.test.ts` cases |
| RA7 | Downloader and verify: the runtime's two files (accept once per set, check before every launch) and 1.4.4's manifest for the DR flavor | L23–L25 | Reforged's `runtime.test.ts`, `revived.test.ts` cases |
| RA8 | Settings, log, folders; keystore with the safeStorage reader for Reforged's existing Windows keys (per server, bound to `fp`) | L26–L29, LB1 | A real 1.5.1 profile's keys read back and work |
| RA9 | Game config (Engine.ini, Game.ini, display repair), game folder detection (Legendary, Heroic, Epic, folder), launch with `REFORGED_BACKEND_URL` and the 1.14.7 command line; DR flavor with 0.1.35's DLL pins | L30, L31 | Reforged's `game.test.ts`, `launch.test.ts`, `display-repair.test.ts` cases |
| RA10 | **Linux spike, then port**: 1.14.7 under Proton with the runtime loaded through `winmm.dll`, reaching the relay; then DR's Proton/Wine runtime fitted to it, game detection through Heroic and Legendary on Linux, the DR flavor under Proton | L32 | Spike: Ramsgate loads and a hunt starts on Linux. Port: the same from the packaged launcher |
| RA11 | Links, dialogs, window, scheme registration for both schemes on both OSes | L5, L34–L36 | Clicking either scheme opens the running launcher |
| RA12 | Host page backend (Windows): workspace detection (`hostkit`), running `reforged-host` (RH), requirements, injector (RD9) | — | Reforged's `hostkit.test.ts`, `hostserver.test.ts` cases; the injector loads CatalogExporter into a 1.14.7 client |
| RA13 | Controller ported and every page command forwarded | L38, L39 | Reforged's `controller.test.ts` and `ui.test.ts` cases |
| RA14 | Package and sign updates: NSIS, AppImage, deb, rpm; release workflow; 2.0.0-beta | L45–L48 | Install, update and uninstall on a clean Windows VM; deb and rpm on Ubuntu 22.04/24.04, Debian 12, Fedora; the AppImage on SteamOS |
| RA15 | The Electron 1.6.0 bridge (RD7) | L50 | 1.5.1 → 1.6.0 → 2.0.0 on a clean VM keeps keys and settings |
| RA16 | 3.0.0: retire the Squirrel feed, delete the Electron key copies, the Electron main process and Forge | L50a, L51 | — |

### 4.6 Cross-cutting (RX)

| # | What |
| --- | --- |
| RX1 | Per-file delta lists: for each Reforged Node file, `diff` against DR's Node file at `e1c5fec`, written into the RS/RA/RH row that ports it, so nothing Reforged changed is lost in the Rust copy |
| RX2 | Catch-up lists from the copy commit: Reforged's own Node commits to the launcher, gateway, content and host kit, ported before each one's cutover |
| RX3 | Security pass (RD10): gateway policy and TLS, allowlist helper and its IPC, relay, pinned TLS, keystore and legacy reader, invite parsing, runtime download, the injector, the host kit's handling of secrets; fuzz targets for every parser |

## 5. Order

Two tracks in parallel (RD12). The Paradox fork and the repository rows come first because both tracks build on them.

| Step | Servers and host kit | Launcher |
| --- | --- | --- |
| 1 | RR1–RR4, RP1–RP4, RS1 | RA1–RA3 |
| 2 | RS2 content, RH1 | RA4–RA7 |
| 3 | RS3 gateway, RS4 allowlist, RH2–RH3 | RA8–RA9, RA10 spike |
| 4 | Shared check: the Tauri debug build against the Rust gateway and content on a test host | (same) |
| 5 | RH4–RH6; cutover on one test host, then every host through `update`; RS5, RH7 | RA10 port, RA11–RA13 |
| 6 | — | RA14 2.0.0-beta, RA15 bridge |
| 7 | — | RA16, RR5 |

The Electron 1.5.x launcher keeps working against the Rust servers, so the server cutover does not wait for 2.0.0.

## 6. Things that must not change

- Launcher: the account keys already stored (read once, then re-stored), their binding to the certificate
  fingerprint, invite formats v1/v2 of both schemes, relay port 61100, the runtime acceptance per server and the
  check before every launch, the 1.14.7 executable hash, the page's look and both languages word for word; the key
  never crosses the bridge or reaches a log.
- Servers: the gateway's certificate and so every invite's `fp`; every route the game, the runtime and the launcher
  use; the host-only routes and headers stay refused; the content server's file layout; the MongoDB data.
- Host kit: its command names and options, `host.json` and the `.env` files it writes, so an existing host moves
  over with `update`.
- Smoke list at every step: register with an invite, login, Ramsgate, a hunt, chat in the City room, runtime
  download, ban, clean stop.

## 7. Open questions

- The first Linux release's Host page: Windows-only as planned (RD4), or SDK and game-data generation under Wine
  too?
- Does the server release archive bundle a Node runtime, or do hosts keep installing Node.js themselves (RD8)?
