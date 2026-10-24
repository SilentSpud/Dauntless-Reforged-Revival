<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Migration guide: where every upstream piece lives in the Rust codebase

The map from the code this repository came from to the code it is now, and the method for carrying the next
upstream change across. It is the backbone for every later port: a change in Dauntless Revived's, Mystic Paradox's
or Freeza's line is first placed with section 4, then moved with section 5, and the row that makes a new module
adds its line to section 2. Written 2026-10-24 against `plan` at `cb3d7e5`.

[`MIGRATION_PLAN.md`](../../MIGRATION_PLAN.md) says what is done and why (the RD decisions, the RS/RH/RA/RB rows);
[`AGENTS.md`](../../AGENTS.md) says how work lands. This guide does not repeat them. It answers two questions those
documents leave to the reader: *where did this upstream file go*, and *where does the next upstream change go*.

How to read the tables:

- "DR" is Dauntless Revived at `e1c5fec`, the tip of its `plan` branch, where its own Rust port stopped on
  2026-10-22. "DR Node" means its Node sources (`legacy/Undaunted*/` there), "DR Rust" its `rust/` and
  `legacy/UndauntedLauncher/src-tauri/` there.
- "Reforged Node" means Freeza's Node projects, now under [`legacy/`](../../legacy) here, as they were when the Rust
  copy was made (`main` at `77fb622`, 2026-10-20).
- Paths without a repository are paths on `plan` in this repository. `rust/` paths are relative to `rust/`.
- Rows in the plan are named as there: RS3 is the gateway row, RA13 the controller row, and so on.

## 1. The sources

| Source | Taken at | What it is | How it came in |
| --- | --- | --- | --- |
| [Dauntless Revived](https://github.com/mixutin/dauntless-revived) (mixutin) | `plan` at `e1c5fec` | Node services for Dauntless 1.4.4 (`UndauntedGateway`, `UndauntedContent`, `UndauntedMetagame`, `UndauntedDeployServer`, `UndauntedDiscordBot`), an Electron launcher, and a half-finished Rust port of all of them (`rust/`, the Tauri launcher in `src-tauri`) | One snapshot copy of its Rust code (RD6: commit `5281638` for `rust/`, `c9f9d6a` for the gateway and content server, `a6e1870` for the launcher), fitted to Reforged. Nothing flows from DR afterwards |
| Reforged's own Node (Freeza's line, [`main`](https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival)) | `main` at `77fb622` for the deltas; merged into `plan` since | `ReforgedGateway`, `ReforgedContent`, `ReforgedHost`, `ReforgedLauncher`: DR's Node code modified for 1.14.7, plus the host kit, which has no DR counterpart | Moved to `legacy/` (RR2, `b67966b`). What it changed against DR is in the delta lists (RX1) and was carried onto the DR Rust copy; what changes after the copy goes on the catch-up lists (RX2) |
| [Mystic Paradox](https://github.com/pranav158/Mystic-Paradox) (Pranav Karande) | Base `00d3be8`, now `21b9ff5` | The 1.14.7 port: Node backend and Director, the C++ game runtime (`MysticParadox.dll`), its loader (`winmm.dll`, Rust) and the CatalogExporter | Forked with its history through [SilentSpud/Mystic-Paradox](https://github.com/SilentSpud/Mystic-Paradox) (`reforged` branch at `24e60c8`) and merged as a subtree into `paradox/` and `native/` (RD1, `e34cf60`). Updated to `21b9ff5` by replaying the Reforged patches (`ab1830a`) |
| Freeza's patches 0001–0039 | `upstream/patches/` on `main` | The Reforged changes to Mystic Paradox, kept as `git diff` files applied by the Node host kit | One commit each in the fork's `reforged` branch (0001–0033), then ordinary commits to `paradox/` and `native/` (0034–0039). [`docs/paradox-patches.md`](../paradox-patches.md) says what each one did and what is left of it after `21b9ff5` |
| [Undaunted](https://github.com/SyST3MDeV/Undaunted) (gwog) | — | The project both DR and Mystic Paradox are modified versions of | Nothing taken directly; it is in both lines' history. Attribution in [`NOTICE.md`](../../NOTICE.md) |

## 2. The map

### 2.1 Folders

| Upstream | Here | Note |
| --- | --- | --- |
| DR `rust/` (workspace files, `crates/`, `fuzz/`) | [`rust/`](../../rust) | Same layout (RD11). The workspace lists every planned member from the start, stubs included |
| DR `rust/services/gateway`, `rust/services/content` | `rust/services/gateway`, `rust/services/content` | Fitted to Reforged; sections 2.3 and 2.4 |
| DR `rust/services/metagame`, `deploy`, `discord-bot` | not copied | Reforged's equivalents are Mystic Paradox's backend and Director (`paradox/`) and the new admin service. The RB rows take pieces of `metagame` by hand; section 2.7 |
| DR `legacy/UndauntedLauncher/src-tauri`, `isolation/` | [`rust/launcher/src-tauri`](../../rust/launcher/src-tauri), `rust/launcher/isolation` | Section 2.6 |
| DR `legacy/Undaunted*/` (the Node services) | not copied | Reforged's Node code is DR's modified; it is under `legacy/` here instead |
| DR `deploy/` (Windows, Linux and OpenBSD kits) | not copied | Replaced by the host kit (section 2.5). Ideas taken earlier into the Node kit: the nftables allowlist table (`deploy/unix-common/allowlist-core.mjs`), the Wine runner on `xvfb-run`, one systemd unit per capability |
| DR `.github/workflows/server-release.yml`, `ci.yml` | [`.github/workflows/server-release.yml`](../../.github/workflows/server-release.yml), `ci.yml` | Method reused (RH6, RR3); Reforged's binaries and families |
| Reforged `ReforgedGateway/`, `ReforgedContent/`, `ReforgedHost/`, `ReforgedLauncher/` | `legacy/<same name>/` | Unchanged by the move. Never extended; deleted by RS5, RH7 and RA16 |
| Reforged `upstream/` (the Paradox pin, patches, `store-captures.json`) | gone | `store-captures.json` is [`paradox/backend/store-captures.json`](../../paradox/backend/store-captures.json); the patch texts are commit messages and [`docs/paradox-patches.md`](../paradox-patches.md) |
| Mystic Paradox `ParadoxBackend/`, `ParadoxDirector/` | [`paradox/backend`](../../paradox/backend), [`paradox/director`](../../paradox/director) | Still Node, until the RB rows. Any change is allowed, fixes are the baseline (RD2) |
| Mystic Paradox's hunt-manifest module (new at `21b9ff5`) | [`paradox/shared/hunt-manifest`](../../paradox/shared) | Imported by the backend and the Director |
| Mystic Paradox `ParadoxRuntime/` | [`native/paradox-runtime`](../../native/paradox-runtime) | C++, built by each host. Freeza's runtime patches keep landing here after the cut-off (RD16) |
| Mystic Paradox `tools/RuntimeLoader/` | [`native/runtime-loader`](../../native/runtime-loader) | A Rust crate of its own, outside the workspace |
| Mystic Paradox `tools/CatalogExporter/` | [`native/catalog-exporter`](../../native/catalog-exporter) | Loaded into the game through the runtime loader (RD9) |
| Mystic Paradox `ParadoxLauncher/`, `ParadoxTransport/`, its release workflows, `.gitleaks.toml`, `docs/SELF_HOSTING.md` | not taken | Reforged has its own launcher; the ICE transport belongs to Paradox's player-hosted realms |
| Mystic Paradox top-level `ADDITIONAL_TERMS.md`, `LICENSE`, `NOTICE.md`, `README.md`, `docs/` | `paradox/<same>` | Stay there after the Node code is gone (section 4.1 of the plan) |

### 2.2 Shared crates (from DR `rust/crates/`)

| Crate | DR origin | Changed here | Not used here |
| --- | --- | --- | --- |
| [`crates/protocol`](../../rust/crates/protocol) | DR X4: wire types the launcher and servers share | `reforged-revival://` is the only scheme, a `dauntless-revived://` invite parses to a typed refusal (RD5); `RRK_` keys (`account.rs`); the runtime manifest type (`manifest.rs`) serialising in the Node field order; `text.rs` new. Shared test tables in `tests/cases/*.json` | DR's 1.4.4 fields |
| [`crates/proxy`](../../rust/crates/proxy) | DR X5: hop-by-hop headers, idle cut, WebSocket tunnel | Nothing | — |
| [`crates/service-kit`](../../rust/crates/service-kit) | DR B1: Express-compatible routing, `JSON.stringify` output, pino logs, `--env-file` config, the X7 listen and stop cycle | Nothing | — |
| [`crates/store`](../../rust/crates/store) | DR D10, D11, D21: synchronous Diesel on a blocking pool, embedded migrations | Nothing yet. RB3 moves it to `PgConnection` (RD15) | The SQLite parts (PRAGMAs, online backup, Drizzle adoption, offline saves) go with RB3 |
| [`fuzz/`](../../rust/fuzz) | DR X9: `http_body`, `proxy_headers`, `gateway_policy`, `launcher_input` | Fourteen Reforged targets added, one per untrusted-input parser (RX3) | DR's `auth_token`, `bot_input`, `chat_frame`, `content_range` (metagame, bot and ranges) |

### 2.3 Gateway and allowlist helper (`rust/services/gateway`, rows RS3 and RS4)

Three-way: the Reforged Node file, the DR Node file it was modified from, the DR Rust module that ported the DR
file, and what Reforged changed. The per-line detail is in [`delta-servers.md`](delta-servers.md).

| Reforged Node (`legacy/ReforgedGateway/src/`) | DR Node (`legacy/UndauntedGateway/src/`) | Rust (`services/gateway/src/`) | Reforged's changes carried |
| --- | --- | --- | --- |
| `policy.ts` | `policy.ts` | `policy.rs` | `/reforged/api` with its four public routes; `/__origin/<host>` unwrapping; host-only prefixes `/admin`, `/launcher`, `/internal`, `/health`; Paradox's host-only headers; `backend` for `metagame`; only `POST /heartbeat` feeds the allowlist (Paradox has no `/game/login`, RD20) |
| `config.ts` | `config.ts` | `config.rs` | `GATEWAY_TLS=0` private mode on a Tailscale or loopback address; 443 or 61000 by mode; `GATEWAY_BACKEND_URL`; `GATEWAY_WS_URL` to the backend's own listener |
| `gateway.ts` | `gateway.ts` | `gateway.rs`, `guard.rs` | `x-reforged-gateway`; `x-forwarded-proto` by mode; plain HTTP in private mode |
| `feed.ts` | `feed.ts` | `feed.rs` | None; DR's helper-down backoff kept on purpose (RD20) |
| `server.ts` | `server.ts` | `server.rs`, `cli.rs` | Certificate loaded and checked in public mode only, never made; `mode` in the start-up log |
| `tools/make-cert.js` | `tools/make-cert.js` | `cert.rs` (`reforged-host gateway make-cert`) | Reforged's organization name and usage text |
| — | `capture.ts` (DR M2) | `capture.rs` | Ported into Rust for RB2 (RD19); off unless configured |
| `ip.ts`, `ratelimit.ts`, `redact.ts` | identical | `ip.rs`, `ratelimit.rs`, `redact.rs` | None |
| `log.ts` | identical | `service-kit` `linelog` | None |
| `allowlist/config.ts` | `allowlist/config.ts` | `allowlist/config.rs` | `ALLOWLIST_FIREWALL` (`windows` or `nftables`), `ALLOWLIST_NFT`, `ALLOWLIST_NFT_TABLE`, `ALLOWLIST_INTERFACE`; DR's `DR_*` names ignored |
| `allowlist/ports.ts` | (`CheckPorts` in `config.ts`) | `allowlist/ports.rs` | The game's UDP range from `ALLOWLIST_PORTS`, widened by the host kit's `--max-hunts` |
| `allowlist/nft.ts` | — (adapted from DR's `deploy/unix-common/allowlist-core.mjs`) | `allowlist/nft.rs`, `allowlist/firewall.rs` | Table `reforged_revival` swapped in one transaction, `lo` rule, nft at its configured path only (RD20) |
| `allowlist/firewall.ts` | `allowlist/firewall.ts` | `allowlist/firewall.rs`, `allowlist/windows.rs` | Reforged's Windows Firewall rule name and display name; the backend choice |
| `allowlist/helper.ts` | `allowlist/helper.ts` | `allowlist/helper.rs` | The backend choice |
| `allowlist/server.ts` | `allowlist/server.ts` | `allowlist/cli.rs` | nftables needs Linux and a working `nft list tables`; the administrator check is Windows-only |
| `allowlist/state.ts`, `allowlist/sync.ts` | identical | `allowlist/state.rs`, `allowlist/sync.rs` | None |
| tests | — | `tests/`, `tests/parity/` (recorded from the Node gateway) | Parity fixtures are the contract |

### 2.4 Content server (`rust/services/content`, row RS2)

| Reforged Node (`legacy/ReforgedContent/src/`) | DR Node (`legacy/UndauntedContent/src/`) | Rust (`services/content/src/`) | Reforged's changes carried |
| --- | --- | --- | --- |
| `runtime.ts` | — | `runtime.rs` (type in `protocol::manifest`) | New: the two-file runtime (`MysticParadox.dll`, `winmm.dll`), `runtime.json`, states `ok`/`none`/`invalid`, the 10 s re-check, the manifest written byte for byte for a stable ETag |
| `app.ts` | `app.ts` | `app.rs`, `input.rs` | `GET /content/v1/runtime/manifest` and `/runtime/files/<name>` in place of DR's game manifest and files; no HTTP ranges; `x-reforged-user-api-key`; no CORS headers; 256 KiB chunks |
| `auth.ts` | `auth.ts` | `auth.rs`, `client.rs` | Key lookup at `GET /reforged/api/GetUserInfo` |
| `config.ts` | `config.ts` | `config.rs` | Loopback binds only; `BACKEND_URL` (loopback `http:`); `CONTENT_RUNTIME_DIR`; `CONTENT_GAME_DIR`, `CONTENT_MANIFEST`, `CONTENT_DOWNLOAD_BASE_URL`, `CONTENT_ALLOW_ANY_BIND` gone; 4 streams per account |
| `server.ts` | `server.ts` | `server.rs`, `cli.rs` | No game-folder check at start; a missing runtime is a warning; `SIGBREAK` |
| `branding.ts`, `limits.ts`, `log.ts`, `news.ts` | identical | `branding.rs`, `limits.rs`, `log.rs`, `news.rs` | None |
| — | `gamedir.ts`, `manifest.ts`, `paths.ts`, `verify.ts`, `range.ts`, `data/dauntless-1.4.4.json` | dropped | The 1.4.4 game download. Only `if_none_match_hits` survives, in `app.rs` |

### 2.5 Host kit (`rust/services/host`, bin `reforged-host`, rows RH1–RH6)

No DR counterpart: DR's kits were PowerShell and shell scripts under `deploy/`. The Rust kit follows the Node kit's
files one to one; its tests run against fixtures generated from the Node kit (`tests/fixtures/node/`).

`reforged-host` is also the one server binary: each Rust server is a cargo feature of the host crate (all on by
default) and a subcommand (`reforged-host gateway|allowlist|content|admin ...`, `servers.rs`). Each service crate
keeps its entry point as `cli.rs` (`Cli` and `run`), where DR had `main.rs` or `bin/*.rs`.

| Reforged Node (`legacy/ReforgedHost/src/`) | Rust (`services/host/src/`) | Note |
| --- | --- | --- |
| `cli.ts` | `cli.rs`, `main.rs`, `commands/mod.rs` | The same commands and options: `setup`, `start`, `invite`, `doctor`, `gamedata`, `publish-runtime`, `preset`, `multipliers`, `update`, `backup`, and the Linux-only `gameserver` |
| `hostconfig.ts` (`host.json`) | `hostconfig.rs`, `hosttoml.rs` | `host.toml` with the same fields (RD24); a Node kit's `host.json` is read until the first write |
| `envfile.ts`, `paths.ts`, `run.ts` | `envfile.rs`, `paths.rs`, `run.rs` | `paths.rs` knows the `paradox/` and `native/` layout |
| `services.ts` (the process tree) | `services.rs`, `services/tree.rs`, `commands/start/` | One process tree; the Node backend and Director plus MongoDB until RB14; the environment cleared of `SERVER_PREFIXES` |
| `doctor.ts` | `doctor.rs`, `commands/doctor.rs` | Checks `paradox/` and `native/` instead of the patch marker |
| `invite.ts` | `invite.rs`, `commands/invite.rs` | Admin routes on loopback only |
| `mongo.ts` | `mongo.rs`, `mongo/wire.rs` | The replica set, until RB14 |
| `gamedata.ts`, `gamepack.ts`, `progression.ts`, `captures.ts` | `gamedata.rs`, `gamepack.rs`, `progression.rs`, `captures.rs`, `commands/gamedata.rs` | Runs the Node generators in `paradox/backend/scripts` until RB11 |
| `preset.ts` | `preset.rs`, `commands/preset.rs` | `preset/` hash-locked by `preset.json` |
| `publish.ts` | `publish.rs`, `commands/publish_runtime.rs` | The Host page calls this command (RD23) |
| `selfupdate.ts` | `selfupdate.rs`, `commands/update.rs` | From the server release (RD8), no longer a `git` fast-forward |
| `upstream.ts` | gone | RP4: nothing is downloaded from Mystic Paradox |
| `linux.ts`, `wine.ts`, `gameserver.ts` | `linux.rs`, `wine.rs`, `cli.rs` (`Gameserver`) | systemd units, Wine or Proton on `xvfb-run` |
| `linux/setup-vps.sh`, `linux/Deploy-FromWindows.ps1` | still only in `legacy/ReforgedHost/linux/` | RH5 reduces them to fetching the release and calling `reforged-host`; [`docs/hosting-linux.md`](../hosting-linux.md) is the host's view |
| — | `commands/setup/` (`database`, `firewall`, `install`, `options`, `paradox`, `secrets`), `commands/backup.rs`, `commands/multipliers.rs` | The Rust split of the Node `setup` and its helpers |

### 2.6 Launcher (`rust/launcher`, rows RA1–RA13)

The Rust backend is DR's Tauri launcher fitted to Reforged; the page is still Reforged's TypeScript page
(`legacy/ReforgedLauncher/src/renderer`, built by `rust/launcher/vite.config.ts`, talking to Rust through
`rust/launcher/page/bridge.ts` and the generated `page/generated/types.ts`). RR5 moves the page to
`rust/launcher/ui`. The per-line detail is in [`delta-launcher-platform.md`](delta-launcher-platform.md) and
[`delta-launcher-controller.md`](delta-launcher-controller.md).

| Reforged Electron (`legacy/ReforgedLauncher/src/`) | DR Rust module (`src-tauri/src/`) | Here | Reforged's changes carried |
| --- | --- | --- | --- |
| `main/controller.ts` | `controller/mod.rs`, `platform.rs`, `stored.rs` | the same, plus `controller/game.rs`, `runtime.rs`, `host.rs` | The 1.14.7 flow: find the game instead of downloading it, accept and install the runtime, the Host page; the relay in both modes; moved-server detection; the 1.4.4 path dropped (RD5) |
| `main/ipc-validate.ts`, `preload.ts`, `renderer/global.d.ts` | `bridge/spec.rs`, `commands.rs`, `generate.rs`, `isolation/hook.js` | the same | Reforged's command list; new argument kinds (index, fixed names, objects); `copy_text` a command (RD22) |
| `main.ts` | `lib.rs`, `window.rs`, `platform/dialogs.rs`, `invite_link.rs`, `art.rs` | the same, plus `platform/clipboard.rs` | Reforged's identity, `userData` folder, `reforged-revival://` scheme, `rr-art` art scheme |
| `shared/types.ts`, `status.ts`, `invite.ts`, `username.ts` | `bridge/types.rs`, `core/status.rs`, `core/invite.rs`, `core/username.rs` | the same, on `crates/protocol` | Snapshot shape; `/reforged/api`; one scheme |
| `shared/i18n/` (`en.ts`, `pt-BR.ts`) | `core/i18n.rs`, `core/text.rs`, `build.rs` (`gen/strings.json`) | the same | Two languages (RD14); the 1.4.4 strings removed, the refusal string added |
| `main/constants.ts` | `core/pins.rs`, `platform/paths.rs`, `platform/links.rs`, `tauri.conf.json` | the same | 1.14.7 build, changelist and executable hash; `Jackal`; relay 61100; the runtime's two file names; Reforged's URLs |
| `main/pinned.ts`, `http.ts`, `hostapi.ts` | `net/pinned.rs`, `connector.rs`, `http.rs`, `hostapi.rs` | the same, plus `net/http/to_file.rs`, `net/hostapi/parse.rs` | `/reforged/api`; `request_to_file` for the runtime files (RD22) |
| `main/relay.ts` | `net/relay.rs`, `net/relay_owner.rs` | the same, plus `net/relay/local.rs`, `net/relay_owner/table.rs` | Port 61100, `/__origin/` paths passed through, `/QoS` answered locally, both modes; DR's owner check kept on Windows (RD21) |
| `main/runtime-files.ts`, `verify.ts` | — , `net/verify.rs` | `net/runtime_files.rs` (+ `manifest.rs`), `net/verify.rs` (+ `cache_file.rs`) | New: accept a runtime set once per server, check before every launch; verify the 1.14.7 executable hash only |
| `main/keystore.ts` | `platform/keystore/{mod,os,legacy}.rs` | the same, plus `keystore/format.rs` | `RRK2` and bare keys only, bound to the certificate fingerprint; the Electron launcher's keys read once (RD22) |
| `main/settings.ts`, `log.ts`, `system.ts` | `platform/settings.rs`, `log.rs`, `system.rs` | the same | Reforged's folders; saved 1.4.4 servers read and shown disabled (RD22) |
| `main/engineini.ts`, `display-repair.ts`, `game-folder.ts`, `launch.ts` | `platform/game_config.rs`, `game_folder.rs`, `launch.rs` | the same | Engine.ini XMPP section to the relay, Game.ini `[OnlineSubsystemPhoenix]` removed; Legendary, Heroic, Epic or a folder; the 1.14.7 command line with `REFORGED_BACKEND_URL` |
| `main/links.ts` | `platform/links.rs` | the same | Reforged's URLs |
| `main/hostkit.ts`, `hostserver.ts`, `requirements.ts` | — | `platform/hostkit.rs` (+ `parse.rs`), `hostserver.rs`, `requirements.rs` | New: the Host page drives `reforged-host` from the release (RD23) |
| `main/injector.ts` | — | `platform/loader_session.rs` | Not ported as written: the runtime loader's `mystic_loader.ini` loads the host's chosen DLL (RD9) |
| `main/revived/` (nine files) | `net/autoregion.rs`, `platform/client_mods.rs`, `core/manifest.rs`, `manifest_check.rs`, `net/downloader.rs` | dropped | DR's 1.4.4 servers (RD5); the downloader went with them (RD22) |
| — | `offline/`, `controller/offline.rs` | dropped | Reforged has no offline mode |
| — | `platform/linux_runtime.rs`, `platform/updater.rs` | the same | Kept for RA10 (Proton) and RA14 (updates) |
| test cases | — | `src-tauri/src/*/tests.rs`, `rust/launcher/test/`, `protocol/tests/cases/` | Reforged's `*.test.ts` cases ported |

### 2.7 Backend and Director: Mystic Paradox now, Rust after the gate

| Mystic Paradox | Now | Rust home (RB rows, after RD17) |
| --- | --- | --- |
| `ParadoxBackend` (Express, `ws`, `ltx`, MongoDB) | `paradox/backend` | `rust/services/backend` (a stub today): one trait per Paradox repository on Postgres (RD15), routes by area (RB6–RB9) |
| `ParadoxBackend/scripts/` (game-data generators, admin bootstrap, data migrations) | `paradox/backend/scripts` | `backend gamedata`, `backend admin bootstrap`, folded into `backend db migrate` (RB11) |
| `ParadoxBackend/realtime/` (XMPP) | `paradox/backend/src/realtime` | DR's `rust/services/metagame/src/chat/` at `e1c5fec`, copied in by RB9 and fitted to Paradox's JIDs, rooms and auth |
| Express routing, JSON replies, middleware | — | `service-kit` (routing, `json`, `js`), DR metagame's `js.rs`, `origin.rs`, `redact.rs`, `bodylog.rs` and auth extractors, copied in by RB5 |
| `ParadoxDirector` | `paradox/director` | `rust/services/director` (a stub today), RB10 |
| Hunt tables, game data | `paradox/*/game-data`, `preset/` | `director hunt-tables import`, `backend gamedata` (RB11) |

Until then every backend or Director change is a commit to `paradox/`, kept small, and a new or changed route goes in
RB1's inventory once it exists (RD2 as changed on 2026-10-24).

### 2.8 Freeza's patches

Each patch's current state (kept, reduced, dropped because upstream now does the same) is the table in
[`docs/paradox-patches.md`](../paradox-patches.md). By where they live and who ports them:

| Patches | Live in | Carried into Rust by |
| --- | --- | --- |
| 0001 (runtime part), 0004, 0008, 0011–0016, 0020, 0024, 0026, 0027, 0029, 0031, 0035, 0037–0039 | `native/paradox-runtime` | Nobody: the runtime stays C++ and keeps taking Freeza's patches ([`paradox-runtime-patches.md`](../paradox-runtime-patches.md)) |
| 0002 (`/reforged/api`, `AUTH_MODE=APIKEY`, HTTP-only mode), 0018 (login queue) | `paradox/backend` | RB7 |
| 0005, 0007, 0010, 0017, 0023, 0025, 0028, 0030, 0036; 0009 and 0032 as upstream's own code | `paradox/backend` | RB8 (areas) and RB9 (chat: 0017, 0025) |
| 0003, 0021, 0022, 0033 | `paradox/director` | RB10 |
| 0006, 0019, 0034 | dropped | Upstream `21b9ff5` has the same fix |
| The RD25 kick route (`POST /reforged/api/Kick`) | `paradox/backend` | RB7, as the one post-fork backend route |

### 2.9 Not ported, on purpose

- DR's `metagame`, `deploy` and `discord-bot` services, its `deploy/` kits, `tools/`, `friend-kit/` and documentation
  site: Reforged's equivalents are Mystic Paradox, `reforged-host` and the admin service.
- DR's 1.4.4 launcher code (RD5): `legacy/ReforgedLauncher/src/main/revived/`, `net/autoregion.rs`,
  `platform/client_mods.rs`, the 1.4.4 manifest and DLL pins, `net/downloader.rs`, device signals, the DirectX check.
- DR's offline mode (`offline/`, `controller/offline.rs`), the `DRK2` key format and the `DR_*` allowlist names.
- Mystic Paradox's launcher and ICE transport, release workflows and self-hosting guide.
- Reforged's `injector.ts` (RD9) and `ReforgedHost/src/upstream.ts` (RP4).

### 2.10 New here, with no upstream

The admin service ([`rust/services/admin`](../../rust/services/admin), RD25); the whole host kit; the runtime manifest
and download (`content/src/runtime.rs`, launcher `net/runtime_files.rs`); the Host page backend and the loader session
(`platform/hostkit.rs`, `hostserver.rs`, `requirements.rs`, `loader_session.rs`); `protocol/src/text.rs`; the
nftables backend as Reforged wrote it; fourteen fuzz targets and the `docs/security/` checklists; `host.toml`;
`docker/` and `compose.yaml`.

## 3. Rename dictionary

What a DR name became, so a DR commit or a DR Rust module reads straight across. The Node side of each pair is in
`legacy/` here and in DR's `legacy/Undaunted*/`.

| DR | Reforged | Where |
| --- | --- | --- |
| `metagame` (the upstream, the config key) | `backend` | gateway `policy.rs`, `config.rs`; content `config.rs` |
| `/undaunted/api/...` | `/reforged/api/...` | gateway policy, content auth, launcher `net/hostapi.rs`, `protocol` |
| `x-undaunted-user-api-key` | `x-reforged-user-api-key` | content `input.rs`, gateway, launcher |
| `x-undaunted-gameserver-apikey` | `x-mysticparadox-gameserver-apikey` | gateway policy (host-only header) |
| `x-dauntless-gateway` | `x-reforged-gateway` | gateway `gateway.rs` |
| `GATEWAY_METAGAME_URL`, `METAGAME_URL` | `GATEWAY_BACKEND_URL`, `BACKEND_URL` | gateway and content `config.rs` |
| `GATEWAY_WS_URL` to port 61099 | the backend's own listener (61000) | gateway `config.rs` |
| `DR_NFT_BINARY`, `DR_NFT_TABLE`, `DR_FIREWALL_INTERFACE` | `ALLOWLIST_NFT`, `ALLOWLIST_NFT_TABLE`, `ALLOWLIST_INTERFACE` | `allowlist/config.rs` |
| nft table `dauntless_revived` | `reforged_revival` | `allowlist/nft.rs` |
| Windows Firewall rule `Dauntless Revived game ports (allowlist)` | `Dauntless Reforged Revival game ports (allowlist)`, name `ReforgedRevival-GamePorts-Allowlist` | `allowlist/windows.rs` |
| certificate organization `Dauntless Revived server` | `Dauntless Reforged Revival server` | gateway `cert.rs` |
| `/content/v1/manifest`, `/content/v1/files/<path>` | `/content/v1/runtime/manifest`, `/content/v1/runtime/files/<name>` | content `app.rs` |
| `CONTENT_GAME_DIR` (required) | `CONTENT_RUNTIME_DIR` (optional) | content `config.rs` |
| `dauntless-revived://join` | `reforged-revival://join`; the old scheme parses to a typed refusal | `protocol/src/invite.rs`, launcher `platform/invite_link.rs` |
| `DRK2` key files | `RRK2` | launcher `platform/keystore/format.rs` |
| `UUK_` account keys (DR's metagame) | `RRK_<48 hex>` (the JSON field is still `UUK`) | `protocol/src/account.rs` |
| relay on 61000, XMPP on 61099 | relay on 61100 carrying HTTP and chat, `/QoS` answered locally | launcher `net/relay.rs`, `core/pins.rs` |
| `.dr-tmp` | `.rr-tmp` | launcher `platform/game_config.rs` |
| `dr-art` scheme | `rr-art` | launcher `platform/art.rs` |
| Dauntless 1.4.4, `PINNED_EXE_SHA256` `d3d41e61…`, exe size pin | 1.14.7 `rel-1.14.7-Archon`, changelist 647472, `f3f95fbf…`, no size pin, `-epicapp=Archon`, Epic app name `Jackal` | launcher `core/pins.rs` |
| `[SystemSettings]` memory lines, exposure option | gone; `[OnlineSubsystemPhoenix]` removed from Game.ini before launch | launcher `platform/game_config.rs` |
| DR's nine languages | `en`, `pt-BR` | launcher `core/i18n.rs` |

## 4. Where the next upstream change goes

Place the change first; every row below names the method and where the port is recorded.

| The change is in | It lands in | How | Recorded in |
| --- | --- | --- | --- |
| Freeza's `main`: a runtime patch (`upstream/patches/NNNN-runtime-*.patch`) | `native/paradox-runtime`, `native/runtime-loader` | Paths rewritten and `git apply --3way`, one commit per patch named `NNNN-runtime-<name>`, as [`paradox-runtime-patches.md`](../paradox-runtime-patches.md) says. Taken before and after the cut-off (RD16) | A section in `docs/paradox-patches.md` |
| Freeza's `main`: a backend or Director patch | `paradox/backend`, `paradox/director` | The same rewrite (`ParadoxBackend/` to `paradox/backend/`), applied as an ordinary commit; a new or changed route goes in RB1's inventory. Not taken after the cut-off | `docs/paradox-patches.md`; the backend and Director catch-up list (RX2, wave 7) names what RB has to match |
| Freeza's `main`: `ReforgedGateway`, `ReforgedContent`, `ReforgedHost` or `ReforgedLauncher` | `legacy/<same>` by the merge of `main`, then the Rust module from section 2 | Merge `main` into `plan` as `02a8070` did. The Node side arrives in `legacy/`; it is never ported ad hoc from there. The family's catch-up list gets an entry, and the row that owns the family (RS5, RH7, RA13's successors) ports it before that family's cutover | [`catchup-gateway.md`](catchup-gateway.md), [`catchup-content.md`](catchup-content.md), [`catchup-host.md`](catchup-host.md); a `catchup-launcher.md` when the first launcher change comes |
| Freeza's `main`: `preset/`, game data, `store-captures.json` | the same paths (`paradox/backend/store-captures.json`) | Taken as they are, before and after the cut-off (RD16); `preset.json`'s hashes follow | The merge commit |
| Mystic Paradox's own `main` | `paradox/`, `native/` | Advance the fork's `reforged` branch, replay the Reforged patches one by one onto the new base, keep, reduce or drop each, as `ab1830a` did. `ParadoxLauncher`, `ParadoxTransport` and the workflows stay out | The "Updated to" section and table in `docs/paradox-patches.md`; `NOTICE.md` |
| Dauntless Revived | nothing | RD6: the copy is final. A DR Rust fix someone still wants is found by its module in section 2 and ported by hand, naming the DR commit | The commit body |
| A Rust service here (not a migration) | its crate | AGENTS.md's rules | — |
| A new Rust module from a row | — | Its line in section 2 of this guide, by the integration pass | This guide |

Two things never happen: `legacy/` is not extended by hand, and a Node fix found in `legacy/` is not ported outside
its catch-up list. The lists are what makes the cutover checkable: every commit on them is ported or marked not
applicable before the family ships.

## 5. How to port one change

1. **Place it** with section 4 and read its entry: the catch-up list for a Node change, Freeza's patch section for a
   Paradox change, the delta file for a file ported for the first time.
2. **Find the Rust home** in section 2 and open the Node file and the Rust module side by side. Code copied from DR
   keeps DR's structure, so a later reader can still diff it against `e1c5fec`; make the Reforged change and nothing
   else.
3. **Match Node where a client can see it.** Routes as Express matches them (`service-kit::routing`), JSON as
   `JSON.stringify` writes it (`service-kit::js`, `json`), the same status codes, headers and field order. When the Node
   and Rust answers differ, the Rust one is the bug unless a decision row says otherwise. Section 6 of the plan lists
   what must not change at all.
4. **Translate names** with section 3 before copying a DR change, and never the other way (a Reforged `.env` starts
   the Rust services unchanged).
5. **Port the tests with the code.** Reforged's `*.test.ts` cases become Rust tests; a reply the Node service made goes
   in the parity fixtures (`tests/parity/`, re-recorded only when the Node side changed on purpose, with the reason in
   the commit); an invite, status or username case goes in `protocol/tests/cases/*.json`, which the launcher and the
   servers both read. Tests that read the shared log take `serial()`; listeners bind `127.0.0.1:0`.
6. **A trust-boundary change** (gateway policy and TLS, allowlist, relay, pinned TLS, keystore, invite parsing, runtime
   download, the loader session, the host kit's secrets, the admin API) updates its fuzz target in `rust/fuzz/` and
   its checklist in `docs/security/<component>.md` in the same commit (RX3).
7. **A configuration variable** is written and read in the same commit: `services/host/src/hostconfig.rs` and
   `hosttoml.rs` write it, the service's `config.rs` reads it, a test round-trips it, an older `host.toml` without it
   keeps today's behaviour, its name carries a server prefix (`SERVER_PREFIXES` in `services/host/src/services.rs`),
   a secret goes in `secrets.env`, and the host docs' tables move with it (AGENTS.md, "Configuration").
8. **Launcher text** goes at the end of both language files with the same keys and placeholders, then
   `RR_UPDATE_GENERATED=1 npm test -- strings-export` regenerates `gen/strings.json`. A bridge command changed in
   `bridge/spec.rs` regenerates its TS types, hook list and permissions with `RR_UPDATE_GENERATED=1 cargo test -p
   launcher generated`. Generated files are never edited.
9. **Check** from `rust/`:

   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked --no-fail-fast
   ```

   `npm ci && npm test` in a `legacy/` or `paradox/` package the change touched; `bash docker/run-tests.sh` when
   what the containers run changed. The real nftables test needs `REFORGED_TEST_NFT=1` on Linux. Nothing here plays
   the 1.14.7 client: say so when only the tests passed.
10. **Record it.** The catch-up entry's `Rust:` line becomes `ported` with the file and lines (section 6's format).
    The commit subject names the row, the body says what changed and why and names the source commit (`From
    FreezaMotoqueiro/Dauntless-Reforged-Revival <path> at <commit>`). The branch is `{type}/{slug}`; push it,
    report the SHA, open no pull request; no AI attribution lines (AGENTS.md, "Commits").
11. **Leave the hands-on part to the maintainer**, written down: a runtime build with a host's SDK and a played hunt,
    the canary, a real `start` with MongoDB, signing and publishing.

## 6. Catch-up entry format

One entry per Node commit, in the family's `docs/port/catchup-<family>.md`, newest last, with the range kept at the
top of the file and the summary counts updated:

```markdown
### `<short sha>` <subject>

- **SHA:** `<full sha>`
- **Date:** <author date, ISO with offset>
- **Author:** <name>
- **Files:** <paths under legacy/..., or the patch files>
- **Behavior:** <what a player, host or the game sees differently; "none" for a rename or comment>
- **Rust:** ported in `<path>:<lines>` | needs porting (<which row, what is missing>) | not applicable (<why>)
```

A commit with parts in more than one status is counted by the part that needs the most work and lists each part
under its `Rust:` line, as `catchup-host.md` does for `0ba6016`.

## 7. Pins

| What | Commit | Date |
| --- | --- | --- |
| Dauntless Revived, `plan` (the copy; its Rust port stopped here) | `e1c5fec` | 2026-10-22 |
| Reforged `main`, the Node sources the delta lists diffed | `77fb622` | 2026-10-20 |
| Reforged `main` last merged into `plan` (`02a8070`, patches 0034–0039) | `aea8f64` | 2026-10-23 |
| Mystic Paradox base (Freeza's pin) | `00d3be8` | — |
| Mystic Paradox now in `paradox/` and `native/` | `21b9ff5` | 2026-10-18 |
| SilentSpud/Mystic-Paradox `reforged` (base + patches 0001–0033) | `24e60c8` | 2026-10-22 |
| Game build | `rel-1.14.7-Archon`, changelist 647472, Unreal Engine 4.26.2 | — |
| Cut-off date for Freeza's non-runtime changes (RD16) | not set | — |

## 8. Related documents

- [`MIGRATION_PLAN.md`](../../MIGRATION_PLAN.md): decisions, rows, what is left, what must not change.
- [`AGENTS.md`](../../AGENTS.md): how a row works, lints, tests, configuration, text, branches and commits.
- [`delta-servers.md`](delta-servers.md), [`delta-launcher-platform.md`](delta-launcher-platform.md),
  [`delta-launcher-controller.md`](delta-launcher-controller.md): the file-by-file Reforged-against-DR deltas (RX1).
- [`catchup-gateway.md`](catchup-gateway.md), [`catchup-content.md`](catchup-content.md),
  [`catchup-host.md`](catchup-host.md): Node commits since the copy and their Rust state (RX2).
- [`docs/paradox-patches.md`](../paradox-patches.md) and
  [`docs/paradox-runtime-patches.md`](../paradox-runtime-patches.md): Freeza's patches and how runtime patches are taken.
- [`docs/architecture.md`](../architecture.md): how the parts talk and what each trusts.
- [`docs/security/`](../security): one checklist per trust-boundary component.
- [`NOTICE.md`](../../NOTICE.md): where each part comes from, for attribution.
- Dauntless Revived's own plan, for the method behind the DR Rust code:
  [`MIGRATION_PLAN.md` on its `plan` branch](https://github.com/mixutin/dauntless-revived/blob/plan/MIGRATION_PLAN.md).
