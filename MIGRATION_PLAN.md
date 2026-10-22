<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Dauntless Reforged Revival migration plan: one Rust codebase, Mystic Paradox included

Status: draft, written 2026-10-22; the decisions in section 3 were made the same day and revised after a review the
same evening (RD5, RD15 changed; RD16 and RD17 added); section 5 was rewritten as parallel waves and RD18 added
afterwards. No work rows have started.

Completed rows will be ~~struck through~~ in the tables below, as in DR's plan, and a "What's left" table will be
added above section 1 once the first row closes. The decisions, the reuse table and section 6 are reference, not
work, and are never struck through.

This plan replaces Dauntless Revived's Rust migration, which stopped on 2026-10-22 at `e1c5fec` on its `plan` branch
([its plan](https://github.com/mixutin/dauntless-revived/blob/plan/MIGRATION_PLAN.md)) and is not continued.
Most of what it built was written for code this repository already shares: Reforged's launcher, gateway, allowlist
helper and content server all started as Dauntless Revived's. This plan takes that Rust code over and fits it to
1.14.7 instead of porting Reforged's Node code from scratch. Mystic Paradox stops being a pinned upstream with
patches and becomes part of this repository; its backend and Director are then ported to Rust too, so no Node is
left on a player's PC or a host.

This repository is a fork of [Freeza's](https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival), which keeps
shipping patches to Paradox (0025–0033 landed in October 2026). The plan diverges from it on a cut-off date (RD16):
until then upstream changes are merged as today; after it only the game runtime's C++ patches are taken.

Row ids: **RA** = launcher, **RS** = gateway, allowlist and content, **RH** = host kit, **RP** = the Paradox fork,
**RB** = backend and Director, **RR** = repository and CI, **RX** = cross-cutting, **RD** = decisions. "DR" means
Dauntless Revived at `e1c5fec`; DR row ids (L27, S2, X4, …) point into its plan, where the method and the checks
for each piece are written down.

## 1. End state

| Component | Today | After |
| --- | --- | --- |
| `ReforgedLauncher` | Electron 44, Node main process (~8.2k lines with `main.ts` and `preload.ts`), Squirrel, Forge, Windows only; plays 1.14.7 and, through `src/main/revived/`, Dauntless Revived's 1.4.4 servers | Tauri 2 on DR's launcher backend (`src-tauri`, ~20k lines), the same TS page, isolation pattern, `tauri-plugin-updater`; Windows (NSIS) and Linux (AppImage, deb, rpm) (RD4); 1.14.7 only (RD5) |
| `ReforgedGateway` (+ allowlist helper) | Node, `node-forge` (~2.8k lines) | DR's Rust `gateway` (bins `gateway`, `allowlist`) with Reforged's routes, ports and nft changes |
| `ReforgedContent` | Node (~1.4k lines) | DR's Rust `content` with Reforged's runtime manifest added |
| `ReforgedHost` | Node TS CLI (~4.8k lines with `linux/`) | Rust bin `reforged-host`, same commands (RD3) |
| ParadoxBackend | Mystic Paradox at a pinned commit + 33 patches, downloaded by `setup`; Node, Express, `ws`, `ltx`, `mongodb`, `@node-rs/argon2`, `jsonwebtoken`, `pino` (~28.2k lines, 54 test files, 25 Mongo repositories, ~2.5k lines of XMPP in `realtime/`, plus `scripts/`: game-data generators, admin bootstrap, data migrations) | Rust bin `backend`: `tokio`, `axum` on `service-kit`; data in **Postgres** through Diesel on `store`, moved once per host from MongoDB (RD15, RB14); `quick-xml` XMPP from DR, `argon2`, `jsonwebtoken`, `tracing`; the scripts as `backend` subcommands (RB11) |
| ParadoxDirector | Same pin; Node, Express, `jsonwebtoken`, `pino` (~2.6k lines, 5 test files) | Rust bin `director` on `service-kit`; starts the game servers as today |
| ParadoxRuntime, RuntimeLoader (`MysticParadox.dll`, `winmm.dll`) | C++, patched upstream, built by each host | The same, in `native/`; still built by each host from their own SDK; Freeza's runtime patches keep being applied here after the cut-off (RD16) |
| `upstream/` | Pin + patches + `store-captures.json` | Gone; `store-captures.json` moves next to the backend, attribution in `NOTICE.md` |
| Releases | Launcher (Squirrel) only | Launcher (Tauri feed) and a server release of Rust binaries only, for Windows and Linux (RD8). Hosts need Postgres, nothing else (MongoDB and Node.js until RB14) |

## 2. What is reused from DR

Measured 2026-10-22 by diffing Reforged's Node sources against DR's at `e1c5fec`. "Delta" is the Reforged change
that has to be carried onto the Rust code; line counts are changed lines in the Node files, a rough guide only.

| DR piece (at `e1c5fec`) | Reforged counterpart | Reuse | Delta to carry over |
| --- | --- | --- | --- |
| `rust/crates/protocol` (invite, status, manifest, account) | `src/shared/invite.ts`, `status.ts`, `types.ts` | Most | One scheme, `reforged-revival://` (a `dauntless-revived://` invite is refused with a message, RD5); `RRK_` keys; the runtime manifest type |
| `rust/crates/proxy` (hop-by-hop, WebSocket tunnel) | relay and gateway proxying | Whole | None expected; `/__origin/<host><path>` rewriting stays in the relay |
| `rust/crates/service-kit` (axum setup, pino logs, env config, the X7 service contract) | gateway, content, host kit, backend and Director start-up | Whole | None |
| `rust/crates/store` (Diesel pools, the synchronous closure API `store::read`/`store::write` on the blocking pool, embedded migrations, D10, D11, D21) | none: Reforged's data is in MongoDB today | Core, on Postgres (RB3) | The connection type becomes `PgConnection` (DR kept it to one alias for this, D21); the SQLite-only parts (PRAGMAs, online backup, Drizzle adoption, offline saves) are dropped; `pq-sys` with its `bundled` feature so no host or CI machine needs libpq installed, checked by a spike on MSVC first (RB3) |
| `rust/services/gateway` (TLS, policy, IP checks, limits, redaction, feed, `cert.rs`, `allowlist` bin) | `ReforgedGateway/src` (allowlist `state`, `sync`, `ip`, `ratelimit`, `redact`, `log` identical) | Most | `policy.ts` (~108 lines: `/reforged/api` admin routes, Paradox's host-only routes and headers), `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`; new `allowlist/nft.ts` and `ports.ts` (the game's UDP range from `ALLOWLIST_PORTS`, which `--max-hunts` widens); `tools/make-cert.js` becomes `gateway cert` |
| `rust/services/content` (ranges, news, branding, auth) | `ReforgedContent/src` (`branding`, `news`, `limits`, `log` identical) | Most | New `runtime.ts` (the two-file runtime manifest and download); `app.ts` (~218 lines: no 11 GB game zip), `auth.ts` against the backend's key check, `config.ts` |
| `rust/services/metagame`: `chat/` (XMPP over WebSocket on `quick-xml`: connection, MUC rooms, presence, nicks; ~3.9k lines), `js.rs` (JSON written as `JSON.stringify` does, `localeCompare`), `origin.rs`, `redact.rs`, `bodylog.rs`, the auth extractors | ParadoxBackend's `realtime/` (`ltx`, `ws`), its JSON replies, its middleware | Pieces, copied into `services/backend` (RB rows) | Paradox's own rooms (City rooms, patch 0025), JIDs and XMPP auth; the routes and game logic are Paradox's, not DR's 1.4.4 ones |
| `service-kit`'s Express-compatible routing and body parsers (DR M8) | Express in both Paradox services | Whole | Case-insensitive routes, trailing slashes and body limits as Express does them, so the game client sees the same answers |
| DR's shadow-traffic method (M2 capture at the gateway, M35 replay and diff) | none | Method and the gateway's capture option | Recorded against the Node backend on MongoDB, replayed against Node on MongoDB and Rust on a Postgres copy of the same snapshot (RB12) |
| `rust/services/deploy`, `discord-bot`, the rest of `metagame` | none | Not brought in | — |
| DR's server kits' Rust subcommands and `server-release.yml` (M38, S6) | `ReforgedHost`, `ReforgedHost/linux/` | Method and release workflow | The host kit's commands are Reforged's own (RH rows) |
| Launcher `src-tauri`: `net/pinned.rs`, `net/http.rs`, `net/relay.rs`, `net/hostapi.rs`, `net/verify.rs`, `net/downloader.rs` | `pinned.ts` (identical), `http.ts`, `relay.ts`, `hostapi.ts`, `verify.ts`, `revived/downloader.ts` | Most | Relay: port 61100, `/__origin/` routing, `/QoS` answered locally; downloader: the runtime's two files only (no game manifest); verify: the 1.14.7 executable hash |
| Launcher `platform/keystore` (Windows DPAPI, Linux Secret Service and KWallet, `legacy.rs` safeStorage reader; DR L27, D7) | `keystore.ts` (~14 lines changed) | Whole | One key per server, bound to the certificate fingerprint; Reforged's `userData` folder and app name |
| Launcher `platform/linux_runtime.rs` (Proton and Wine, DR L32) | none | Whole | Run 1.14.7 under Proton with the runtime's `winmm.dll` loaded (`WINEDLLOVERRIDES`), `REFORGED_BACKEND_URL` passed through, Engine.ini/Game.ini inside the prefix (RA10) |
| Launcher `platform/settings.rs`, `log.rs`, `system.rs`, `window.rs`, `invite_link.rs`, `game_folder.rs`, `game_config.rs`, `launch.rs` | `settings.ts`, `log.ts`, `system.ts`, `links.ts`, `game-folder.ts`, `engineini.ts`, `launch.ts`, `display-repair.ts` | Most | Engine.ini XMPP section and Game.ini `[OnlineSubsystemPhoenix]` removal; the 1.14.7 command line and `REFORGED_BACKEND_URL`; Legendary/Heroic/Epic detection |
| Launcher `controller/`, `bridge/` (typed commands, generated TS types, isolation hook) | `controller.ts` (~2.2k lines changed), `ipc-validate.ts`, `preload.ts` | Structure | The largest delta: Host page state and runtime acceptance |
| Launcher `platform/updater.rs`, Squirrel bridge (DR L45–L50), Linux bundles (L47) | `update-electron-app` + Squirrel | Whole | Feed URL and version line (RD7) |
| Launcher `net/autoregion.rs`, `platform/client_mods.rs`, the 1.4.4 manifest and DLL pins | `revived/api.ts`, `revived/constants.ts`, `revived/dlls.ts`, `revived/manifest.ts` | Not brought in (RD5) | — |
| Launcher `offline/`, `controller/offline.rs` | none | Not brought in: Reforged has no offline mode | — |
| Launcher page split per language (DR L13–L15) | `src/shared/i18n.ts` (1.6k lines, en + pt-BR) | Method | Two languages instead of nine |
| CI by family, `cargo deny`, fuzz targets, `SECURITY_REVIEW.md` checklists (DR R8, X6, X9) | `ci.yml`, `launcher-release.yml` | Most | No metagame or bot jobs; a Node job for the Paradox fork |

Not in DR at all, so written new in Rust: the Host page backend (`hostkit.ts`, `hostserver.ts`, `requirements.ts`,
~980 lines), the DLL injector (`injector.ts`, RD9), the runtime files (`runtime-files.ts`, ~290 lines), the whole
host kit (RH rows) and the whole backend and Director (RB rows).

Rough share of the work as planned: backend and Director (with the Postgres move) about half; launcher a quarter;
host kit and front door the rest.

## 3. Decisions

Made 2026-10-22; RD5 and RD15 changed and RD16, RD17 added after the review the same day.

| # | Topic | Decision | Consequence |
| --- | --- | --- | --- |
| RD1 | Mystic Paradox | **Forked into this repository with its history**, through the GitHub fork [SilentSpud/Mystic-Paradox](https://github.com/SilentSpud/Mystic-Paradox) (made 2026-10-22, cloned to `R:\source\Mystic-Paradox`, `upstream` = Paradox's repository). A `reforged` branch in the fork starts at the pinned commit `00d3be8`, not Paradox's `main` (68 commits later: realms, co-op hosting, its ICE transport and Tauri launcher; +127k lines), so the patches apply as they do today and the port stays at ~31k lines. The branch gets the 33 patches as one commit each, in order, with their patch text as the message; that branch is merged into this repository as a subtree at `paradox/`. ParadoxRuntime and RuntimeLoader then move to `native/` with `git mv`. From then on Paradox is changed by ordinary commits; `upstream/`, `npm run upstream` and setup's download step go away. Paradox's `ADDITIONAL_TERMS.md` stays with its code and its attribution moves into `NOTICE.md` | Hosts no longer download anything from Paradox's repository. Upstream changes can be cherry-picked from the fork's history when wanted; nothing forces it |
| RD2 | Backend and Director | **Ported to Rust** (RB rows), after the fork (RP) so the code being replaced is ours and has its history, and after the front door is live (RD17). Each Paradox repository becomes one Rust trait with a Postgres implementation (RD15); there is no MongoDB implementation. DR's metagame is not reused as a whole (SQLite, 1.4.4 routes), only its chat, JSON and auth pieces (section 2). The Node services keep running in production, receiving fixes only, until the Rust backend passes the shadow replay (RB12); their fixes are ported in one catch-up (RX2) | No Node on hosts. About half the plan, so it is its own track with its own gate (section 5) |
| RD3 | Host kit | **Ported to Rust** as `rust/services/host`, bin `reforged-host`, with the same commands (`setup`, `start`, `invite`, `doctor`, `gamedata`, `preset`, `multipliers`, `publish-runtime`, `update`) and the same files it writes (`host.json`, the `.env` files). It starts every service under one process tree (Windows job object, Linux process group): the Node backend and Director plus MongoDB until RB14, the Rust ones and Postgres after. `ReforgedHost/linux/` (`setup-vps.sh`, `Deploy-FromWindows.ps1`) is reduced to fetching the release and calling `reforged-host` | One binary drives the whole server; the Host page runs it instead of `node dist/cli.js`. Starting Node and `mongod` is a few dozen lines that RB15 removes again |
| RD4 | Launcher platforms | **Windows and Linux.** DR's Linux code comes with it: Proton/Wine runtime, Secret Service and KWallet keystore, WebKitGTK 4.1 (DR D3), AppImage, deb and rpm. The Host page on Linux (SDK and game-data generation against the 1.14.7 client under Proton, the injector loading Dumper-7 or CatalogExporter into that Wine process) is a spike with a kill criterion (RA12a): if the spike fails, the Host page stays Windows-only and this row is amended | 1.14.7 and the runtime have to be shown to work under Proton first (RA10, a spike); Linux CI and release jobs |
| RD5 | DR's 1.4.4 servers | **Dropped** (changed after the review; was "kept"). The Tauri launcher plays 1.14.7 only. `src/main/revived/` (0.1.35's DLL pins, the 1.4.4 manifest download, the hunt-region probe) and the `dauntless-revived://` scheme are not ported; such an invite is refused with a message. Dauntless Revived is abandoned, so nobody will keep running the 1.4.4 servers and content this code needs | One code path through the controller and the tests. Players who still use 1.4.4 servers lose them at 2.0.0; the 1.6.0 bridge (RD7) says so before it installs 2.0.0 |
| RD6 | How DR's Rust code comes in | **Snapshot copy** at `e1c5fec` in one commit, with the source commit in the message and in `NOTICE.md`. DR is not continued, so nothing flows either way afterwards | From the copy on, the code is Reforged's own |
| RD7 | Launcher updates | DR's design (D2): `tauri-plugin-updater` with ed25519 signatures. Existing installs take a one-time bridge, Electron **1.6.0**, which downloads the Tauri **2.0.0** installer, checks its SHA-256 compiled into the bridge, runs it and removes the Squirrel install. The Squirrel feed is retired with 3.0.0; the Electron key copies are kept until then so a rollback works | One extra Electron release; 1.5.x installs that skip it before 3.0.0 reinstall by hand |
| RD8 | Server binaries | **Released by CI**: a server release workflow (from DR's `server-release.yml`) builds `gateway`, `allowlist`, `content`, `backend`, `director` and `reforged-host` for `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu`, in one archive per OS, with `SHA256SUMS.txt` and provenance attestations. `setup` and `update` download and check it. Until RB14 the archive also carries the built Node backend and Director, and hosts keep their own Node.js | No Rust toolchain on hosts, and no Node once RB14 ships; `update` stops being a `git` fast-forward of a checkout |
| RD9 | DLL injector | **Rust on the `windows` crate** (`OpenProcess`, `VirtualAllocEx`, `CreateRemoteThread(LoadLibraryW)`), only into the game process the launcher started, only a DLL the host picked | No PowerShell or C# step; its own security checklist (RX3) |
| RD10 | Security | **DR's full pass** (D26, X9): a checklist per trust-boundary component in `SECURITY_REVIEW.md`, `cargo-fuzz` targets for every parser of untrusted input run briefly on every PR, a 24-hour fuzz run and a signed checklist before each component's cutover | Replaces a second reviewer where a mistake costs most |
| RD11 | Repository layout | **DR's monorepo** (section 4.1): `rust/` (one cargo workspace), `native/` (C++ runtime), `paradox/` (the Node backend and Director), `legacy/` for the Node projects being replaced until each is deleted; `preset/`, `brand/`, `docs/` stay | Moves use `git mv` so history follows |
| RD12 | Order | **Two tracks in parallel**, the front door with the host kit and the launcher, synced on the `protocol` crate (RS1) and on shared checks on the canary; the backend track starts only after the gate (RD17). Within a track the rows run in parallel waves (RD18, section 5) | Each track's cutover still waits for its own checks |
| RD13 | Test environment | **A canary run by the maintainer**, like DR's D23: a small server on the Linux kit with a copy of a real database and invited testers. The shared checks, the traffic capture (RB2), the shadow replay (RB12) and every first cutover run there; then the hosts | The Linux path is tested first |
| RD14 | Languages | en and pt-BR, as today, split one file per language as DR did (L13), so a third language is data only | — |
| RD15 | Database | **Postgres, directly** (changed after the review; was "MongoDB first, then Postgres"). The Rust backend has one data layer, Diesel on `store` with `schema.rs` from the migrations, so queries are checked at compile time; no MongoDB implementation is written. Each Paradox repository gets tables designed from its documents: real columns, keys and foreign keys for what the code queries or joins on (accounts, keys, characters, inventory items, loadouts, bounties, invites); `jsonb` only for opaque game blobs the backend passes through unread. A host's data moves once with `backend db migrate` (RB4) at the backend cutover (RB14); `backend db export-mongo` (RB13) writes it back for a rollback to Node during the rollback window. The shadow replay (RB12) compares Node on MongoDB with Rust on a migrated copy of the same snapshot; `backend db verify` tells migration diffs from code diffs. Postgres 17 or newer; the host kit installs and manages a local one (a service on loopback, a role and database for the backend), or takes `--postgres-url` for the host's own | One backend cutover instead of two, and no throwaway data layer. The replay has one more moving part (the migration), covered by `db verify`. Hosts lose the MongoDB replica-set setup and gain `pg_dump` backups; a Postgres service is heavier on a friend's PC than nothing, which is the price of a real database. After the rollback window, MongoDB is uninstalled and a host can no longer return to Node |
| RD16 | Upstream (Freeza) | **Fork with a cut-off date.** Until the cut-off, Freeza's changes (patches, launcher, gateway, content, host kit) are merged into this repository as today, into `paradox/` as commits once RP2 has landed. After the cut-off only two kinds of upstream change are taken: patches to the game runtime (`ParadoxRuntime`, `RuntimeLoader`), applied to `native/` by hand, since the runtime speaks HTTP to whichever backend answers; and game-data or preset changes. Backend, Director, launcher, gateway, content and host kit changes after the cut-off are not ported. The cut-off is `[to be set: date]`, at the latest the day RB2 (the traffic capture) starts, because the capture fixes what the Rust backend has to match | RX2's catch-up lists end at the cut-off. Hosts on Freeza's line and hosts on this one diverge from then on; the launcher stays able to join both only as long as the `/reforged/api` routes stay the same, which this plan does not promise |
| RD17 | Gate before the backend | **The Rust front door ships first.** No RB row starts, the route inventory included, until the Rust gateway, allowlist, content server and `reforged-host` are live on every host (RS5 and RH7 closed). The launcher track is not gated | The cheapest, mostly-reused part proves the canary, the release pipeline and the host kit's `update` path before the expensive part depends on them. DR wrote ~110k lines and shipped none; this gate is the answer to that |
| RD18 | How the rows are done | **One row, one agent, in parallel waves** (section 5). Each row is a brief for an independent agent working in its own worktree and branch and merged by PR. Rows that fix an interface (the seams: RR1, RS1, RA1–RA2, RH1, RB3, RB5) land first and register every planned module, command, route group and trait as a stub; the rows that fill them then run side by side and touch only their own files. Shared files have one owner (section 5.1). About eight rows in flight at once; past that the integration pass between waves, where a person reads every merge, is the bottleneck, not the agents | The order in section 5 is by dependency, not by track. A wave starts when the rows it needs are merged, not when the previous wave is finished. The canary runs, cutovers, spikes that need a machine with the game, signing and publishing, and the cut-off date stay with the maintainer |

## 4. Work

### 4.1 Repository and CI (RR)

Target layout once the Node projects are gone:

```text
rust/                    one cargo workspace
  crates/protocol, proxy, service-kit, store
  services/gateway       bins gateway, allowlist
  services/content       bin content
  services/host          bin reforged-host
  services/backend       bin backend (was ParadoxBackend)
  services/director      bin director (was ParadoxDirector)
  launcher/src-tauri     launcher backend; launcher/ui: the TS page
native/
  paradox-runtime/       MysticParadox.dll (was ParadoxRuntime)
  runtime-loader/        winmm.dll (was RuntimeLoader)
preset/  brand/  docs/  .github/
```

Until RB15, the Node backend and Director live at `paradox/backend` and `paradox/director` (RP3), with the rest of
Paradox's tree (`ADDITIONAL_TERMS.md`, licence, docs) under `paradox/`. Once they are deleted, `paradox/` keeps only
those top-level files, so the attribution stays next to the code that came from it.

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RR1 | Copy `rust/` (workspace files, `rust-toolchain.toml`, `deny.toml`, `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`, `crates/protocol`, `crates/proxy`, `crates/service-kit`, `crates/store`, `fuzz/` targets for those crates) in one commit (RD6) | R4, X6 | `cargo test --workspace`, `cargo deny check`, `cargo clippy` pass on Windows and Linux |
| RR2 | `git mv` `ReforgedGateway`, `ReforgedContent`, `ReforgedLauncher`, `ReforgedHost` to `legacy/` | R5 | Today's npm tests pass from `legacy/` |
| RR3 | CI by family behind one required check: Rust (fmt, clippy, test, deny, short fuzz) on Windows and Linux, the launcher page, the Paradox fork's Node tests, legacy Node | R8 | A Rust-only PR skips the Node jobs and the other way round |
| RR4 | Dependabot entries for `cargo` (`/rust`), the page and `paradox/*`, under the current policy; each npm entry under `legacy/` removed with its folder | X6 | Dependabot opens a `cargo` PR |
| RR5 | Move the launcher page to `rust/launcher/ui` once Electron is deleted (RA16); path sweep for `legacy/` and `upstream/` | R7, R10 | No reference to a deleted path |

### 4.2 The Paradox fork (RP)

| # | What | Verify |
| --- | --- | --- |
| RP1 | In the fork: branch `reforged` from `00d3be8`, patches 0001–0033 as 33 commits, in order; push it | The tree equals what `npm run upstream` produces today (`.reforged-patches.json` set) |
| RP2 | Merge the fork's `reforged` branch as a subtree at `paradox/` | `git log paradox/` shows Paradox's commits and the 33 patch commits |
| RP3 | `git mv` the backend and Director to `paradox/backend`, `paradox/director`, the runtime and loader to `native/`; delete what Reforged does not use (`ParadoxLauncher`, `tools/`, `scripts/` at the top level, Paradox's deployment docs); Paradox's top-level files (`ADDITIONAL_TERMS.md`, licence, `NOTICE.md`) stay under `paradox/` | The backend and Director tests pass; the runtime builds from `native/` with a host's SDK |
| RP4 | `ReforgedHost` (still Node) builds from `paradox/` instead of downloading; delete `upstream/` and `npm run upstream`; move `store-captures.json` to `paradox/backend`; attribution in `NOTICE.md` and the READMEs; a `docs/` note on how Freeza's runtime patches are applied to `native/` after the cut-off (RD16) | `npm run setup` on a clean host with no network access to Paradox's repository |

### 4.3 Gateway, allowlist and content (RS)

| # | What | From DR | Delta | Verify |
| --- | --- | --- | --- | --- |
| RS1 | `protocol`: the `reforged-revival://` scheme only, `RRK_` keys, runtime manifest type; a `dauntless-revived://` invite parses to a typed refusal the page can word | X4 | Section 2 | DR's protocol tests plus Reforged's `invite.test.ts` cases as Rust tests |
| RS2 | Content server: copy `services/content`, drop the game zip, add the runtime manifest and the two-file download, auth against the backend's key check | S1 | `runtime.ts`, `app.ts`, `auth.ts`, `config.ts` | Reforged's content tests ported; the Electron launcher downloads a runtime from the Rust server |
| RS3 | Gateway: copy `services/gateway`, apply `policy.ts` (Paradox and `/reforged/api` admin routes, host-only headers), config, feed and server deltas; `gateway cert` replaces `tools/make-cert.js`; the traffic-capture option kept for RB2; **load the existing certificate, never regenerate** | S2 | `policy.ts`, `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`, `make-cert.js` | Reforged's gateway tests ported; DR's parity test re-pointed at Reforged's Node gateway; an existing invite's `fp` still matches |
| RS4 | Allowlist helper: DR's `allowlist` bin with Reforged's game-port range (`ports.ts`) and its nftables variant (`nft.ts`) for the Linux VPS | S3 | `ports.ts`, `nft.ts`, `config.ts`, `firewall.ts` | Rule contents after login and logout on Windows and on Debian 13 (nft) |
| RS5 | Catch-up of Reforged's Node gateway and content commits since the copy (RX2), then delete `legacy/ReforgedGateway` and `legacy/ReforgedContent` | X3 | — | Every commit on the list ported or marked not applicable |

### 4.4 Host kit (RH)

| # | What | Verify |
| --- | --- | --- |
| RH1 | `services/host` skeleton on `service-kit`: `paths`, `envfile`, `hostconfig` (`host.json`), `clap` commands with today's names and options | Reforged's host kit tests for config and paths ported; a `host.json` written by the Node kit reads back unchanged |
| RH2 | `start`: the Rust services with `--env-file` (DR X7), the backend and Director (Node until RB14), the database when managed (MongoDB until RB14, Postgres after), the allowlist helper in public mode; one process tree, clean stop on Ctrl+C including the game servers | Every service up; nothing left running after stop, on Windows and Linux |
| RH3 | `setup`, `invite`, `doctor`, `multipliers`; the MongoDB replica-set setup (`mongo.ts`) until RB14, then a managed Postgres (install, `initdb`, a role and database for the backend, loopback only) or `--postgres-url`; `backup` with `pg_dump` | A fresh private and a fresh public setup on Windows |
| RH4 | `gamedata`, `preset`, `progression`, `publish-runtime`, `gamepack`, `captures`; until RB11 `gamedata` runs the Node generators in `paradox/backend/scripts`, after it the `backend gamedata` subcommands | The generators give byte-identical files to the Node kit's from the same export |
| RH5 | `update` and setup's install step from the server release (RD8); `linux.ts`, `wine.ts` and `ReforgedHost/linux/` on the Linux release. `update` no longer fast-forwards a checkout and `npm run upstream` is gone (RP4); every other command keeps its name and options | `docs/hosting-linux.md` followed on a clean Debian 13; an existing Node-kit host moves over with `update` |
| RH6 | Server release workflow (RD8, from DR M38) | A run without publishing; its archives install on Windows and Debian 13 and pass the smoke list (section 6) |
| RH7 | Delete `legacy/ReforgedHost` | — |

### 4.5 Backend and Director (RB)

Starts after the gate (RD17). DR's Part B is the method (its M rows in brackets): an inventory first, the shared
pieces, one area per PR, chat last, then shadow traffic before any cutover. The test files are ported with each area.

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RB1 | Route inventory of both services (method, path, auth, body, reply), checked in and kept current by CI | M3 | Every Express route in the inventory; CI fails on a new route without an entry |
| RB2 | Traffic capture: the Rust gateway's capture option (RS3) records redacted request/reply pairs and chat sessions on the canary, played on by invited testers against the Node backend; the cut-off date (RD16) is set no later than this | M2 | A corpus covering login, Ramsgate, a hunt, chat, store, loadouts, bounties, the Reforged API and the Director's game-server calls |
| RB3 | Postgres schema and `store`: `store` on `PgConnection` (a first spike checks `pq-sys` `bundled` builds on MSVC and Linux without libpq), the `0001_baseline` migration with every table, key, index and constraint designed from the 25 repositories (RD15), `schema.rs` from it | D10, D11, D21 | `cargo test` of `store` on both OSes; the schema reviewed against each repository's queries |
| RB4 | `backend db migrate --from <mongodb-uri>`: every collection read, documents validated and converted (ObjectIds kept as text ids, dates to `timestamptz`, nested arrays to child tables), written in one Postgres transaction; documents that fail validation are listed and stop the run unless `--skip-invalid` names them; the MongoDB database is left untouched. `backend db verify --against <mongodb-uri>` compares row counts per collection and a per-document checksum of the fields carried over. Built this early because every later test runs on migrated copies of real databases | — | Run on copies of the canary's and every host's database: `verify` finds no difference |
| RB5 | `services/backend` skeleton: config (`.env` as today), one trait per Paradox repository with its Postgres implementation on `store`, pino-compatible logs, health, the X7 service contract; a clock and an id generator passed in, not called from `std`, so the replay (RB12) can pin them | M5–M7 | A migrated database reads back field for field through the repositories |
| RB6 | Auth: API-key login (`AUTH_MODE=APIKEY`, key HMAC), argon2 hashes, JWTs, TOTP, account eligibility, launcher version checks, runtime authorization, the encryption key store | M9, M20–M22 | `test:auth-security` cases; tokens and keys issued by Node accepted by Rust and the other way round |
| RB7 | The Reforged launcher API (`/reforged/api`: ServerStatus, Register, invites, admin routes; patch 0002) and the login queue and maintenance status (0018) | — | `reforgedApiRules.test.ts`, `maintenanceStatus.test.ts`; the launcher registers against it |
| RB8 | Game areas, one PR each: characters and loadouts (incl. 0027), inventory and storage policy, progression and mastery XP (0005, 0009), bounties (0010, 0028), escalation, store, Lady Luck, platinum (0030), reforge (0032), session supplies (0023), multipliers (0007), matchmaking and parties, the game servers' XP and objective posts (0005) | M29–M31 | Each area's Node tests ported; its routes in the inventory answered |
| RB9 | Chat: XMPP over WebSocket from DR's `chat/`, fitted to Paradox's JIDs, XMPP auth, rooms, City rooms (0025), presence, direct chat, quick chat and emojis (0017) | M32–M34 | `test:realtime` cases; recorded chat sessions replay with the same stanzas |
| RB10 | Director: game-server instances, bind host (0003), training on demand and idle stop (0021, 0033), hunt memory guard (0022), hunt tables; the backend's calls to it | — | Its 5 test files; Ramsgate, the Training Grounds and a hunt start and stop |
| RB11 | The scripts, as subcommands: `backend gamedata` (progression config, Slayer's Path, Lady Luck store, reward cache, inventory storage policy, platinum store, from a CatalogExporter export and `store-captures.json`), `backend admin bootstrap`, the data migrations that still matter for a migrated database (banner instance ids, loadout slots, inventory storage, launcher approval) folded into RB4's conversion, `director hunt-tables import` | — | Each generator gives byte-identical output to its Node script from the same input (`preset/` is the fixture); the host kit's `gamedata` switches to them (RH4) |
| RB12 | Shadow replay: the RB2 corpus against Node on the MongoDB snapshot and Rust on `db migrate`'s Postgres copy of it, clock and ids pinned to the recorded values (RB5), a normalizer for what still legitimately differs (token signatures, ordering Express never promised); every diff explained as code, migration (`db verify` says which) or normalizer. Then the catch-up of Node fixes up to the cut-off (RX2) and a final replay | M35, M44 | No unexplained diff in values or in the JSON text the client parses |
| RB13 | Rollback: `backend db export-mongo --to <mongodb-uri>` writes the Postgres data back as Paradox's documents, so a host can return to the Node backend during the rollback window without losing what players did since the move. Rehearsed on the canary before any cutover: migrate, play, export back, Node passes the smoke list with the new data, migrate again, `verify` clean | M46 | The rehearsal passes twice in a row |
| RB14 | Cutover: the canary first, a week of play, then each host through `update`, which backs up MongoDB (`mongodump`), installs Postgres, runs RB4, starts the Rust backend and Director and keeps MongoDB installed but stopped for 30 days (the rollback window). The Node backend and Director stay in the release archive through the window | M42 | Smoke list (section 6) after each move; the RB2 corpus answers the same before and after |
| RB15 | One release after the last host's window: delete `paradox/backend`, `paradox/director`, the Node parts of the release archive, `export-mongo`, the host kit's MongoDB and Node handling, and MongoDB and Node from the docs | M43 | No `mongodb` in `cargo tree`; a fresh install from the docs alone needs only the release archive and Postgres |

### 4.6 Launcher (RA)

Phases follow DR's Part A. The DR rows in brackets hold the method; the Reforged work is the delta.

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RA1 | Copy `src-tauri` without `offline/`, `controller/offline.rs`, `client_mods.rs`, `net/autoregion.rs` and the 1.4.4 manifest and DLL pins (RD5); the app's identifier, name and `userData` folder are Reforged's | L4–L7 | It builds on Windows and Linux and opens today's page with a stub bridge |
| RA2 | Bridge, generated TS types and isolation hook re-generated for Reforged's command list (`ipc-validate.ts`, `preload.ts`), minus the `revived/` commands | L6, L7, L9 | Every command the page uses is in the bridge spec; a refused command never reaches Rust |
| RA3 | Language files split (en, pt-BR) and the backend's text built from them; the 1.4.4 strings removed, a refusal string for `dauntless-revived://` invites added | L13–L15 | Reforged's `i18n.test.ts` passes on the split files |
| RA4 | Invite, status, username on `protocol` (RS1) | L11, L12 | Shared test tables with the servers |
| RA5 | Pinned TLS, HTTP client, host API | L19–L21 | Reforged's `pinned.test.ts` cases; register and status against the Rust gateway |
| RA6 | Relay on 61100 with `/__origin/` routing, chat WebSocket and local `/QoS`; owner check | L22, L33 | Reforged's `relay.test.ts` cases |
| RA7 | Downloader and verify: the runtime's two files (accept once per set, check before every launch) | L23–L25 | Reforged's `runtime.test.ts` cases |
| RA8 | Settings, log, folders; keystore with the safeStorage reader for Reforged's existing Windows keys (per server, bound to `fp`) | L26–L29, LB1 | A real 1.5.1 profile's keys read back and work |
| RA9 | Game config (Engine.ini, Game.ini, display repair), game folder detection (Legendary, Heroic, Epic, folder), launch with `REFORGED_BACKEND_URL` and the 1.14.7 command line | L30, L31 | Reforged's `game.test.ts`, `launch.test.ts`, `display-repair.test.ts` cases |
| RA10 | **Linux spike, then port**: 1.14.7 under Proton with the runtime loaded through `winmm.dll`, reaching the relay; then DR's Proton/Wine runtime fitted to it, game detection through Heroic and Legendary on Linux | L32 | Spike: Ramsgate loads and a hunt starts on Linux. Port: the same from the packaged launcher |
| RA11 | Links, dialogs, window, `reforged-revival://` scheme registration on both OSes | L5, L34–L36 | Clicking an invite opens the running launcher |
| RA12 | Host page backend (Windows): workspace detection (`hostkit`), running `reforged-host` (RH), requirements, injector (RD9) | — | Reforged's `hostkit.test.ts`, `hostserver.test.ts` cases; the injector loads CatalogExporter into a 1.14.7 client |
| RA12a | **Spike** for the Host page on Linux (RD4): load Dumper-7 and CatalogExporter into 1.14.7 running under Proton (a Windows helper started inside the same Wine prefix, or `CreateRemoteThread` through Wine) and generate an SDK and an export there. Kill criterion: if either injection or the SDK generation does not work on Ubuntu 24.04 and SteamOS within the spike, the Host page stays Windows-only and RD4 is amended. On success: the port (the host kit started from the Linux launcher, requirements for Linux) | — | Spike: `gamedata` from the Linux export equals one made on Windows. Port: Reforged's host page tests on Linux |
| RA13 | Controller ported and every page command forwarded | L38, L39 | Reforged's `controller.test.ts` and `ui.test.ts` cases |
| RA14 | Package and sign updates: NSIS, AppImage, deb, rpm; release workflow; 2.0.0-beta | L45–L48 | Install, update and uninstall on a clean Windows VM; deb and rpm on Ubuntu 22.04/24.04, Debian 12, Fedora; the AppImage on SteamOS |
| RA15 | The Electron 1.6.0 bridge (RD7); it tells players with a 1.4.4 server stored that 2.0.0 drops it (RD5) | L50 | 1.5.1 → 1.6.0 → 2.0.0 on a clean VM keeps keys and settings |
| RA16 | 3.0.0: retire the Squirrel feed, delete the Electron key copies, the Electron main process and Forge | L50a, L51 | — |

### 4.7 Cross-cutting (RX)

| # | What |
| --- | --- |
| RX1 | Per-file delta lists: for each Reforged Node file, `diff` against DR's Node file at `e1c5fec`, written as `docs/port/delta-<family>.md` (gateway, allowlist and content; launcher net and platform; launcher controller, bridge and page) and linked from the RS/RA row that ports it, so nothing Reforged changed is lost in the Rust copy. Read-only, so it runs first, one agent per file |
| RX2 | Catch-up lists from the copy commit to the cut-off (RD16): Reforged's own and Freeza's Node commits to the launcher, gateway, content, host kit, and the forked backend and Director, written as `docs/port/catchup-<family>.md` and ported before each one's cutover by the agent that owns the family. After the cut-off the lists stop; only runtime patches keep coming (RD16) |
| RX3 | Security pass (RD10), done per row rather than at the end: every row that writes a trust-boundary component delivers its fuzz target and a `docs/security/<component>.md` checklist with its code; `SECURITY_REVIEW.md` is the index, kept by the integration pass. Components: gateway policy and TLS, allowlist helper and its IPC, relay, pinned TLS, keystore and legacy reader, invite parsing, runtime download, the injector, the host kit's handling of secrets, the backend's auth and tokens, its JSON and body parsing and XMPP, the documents `db migrate` and `export-mongo` read, and the Director's game-server launch. The 24-hour fuzz run and the signed checklist before each cutover stay with the maintainer |

## 5. Order: parallel waves

The rows are written to be done by independent agents at the same time (RD18). Two things limit how many: the seams,
since a row that fixes an interface has to land before the rows that fill it, and review, since a person reads
every merge. A wave below is the set of rows whose needs are met at the same time; a row starts as soon as the rows
in its "Needs" column are merged, whether or not the rest of its wave has finished. The front door and the launcher
still run as two tracks (RD12) and the backend still waits for the gate (RD17); the waves only say what can overlap.

### 5.1 Rules for a row

- One row is one agent in its own worktree and branch, merged by one PR. The brief is the row's text, the DR rows it
  names, the delta file from RX1 where there is one, section 6, the ownership table below and the branch to base on.
  An agent that finds it needs a change to a seam or to a file it does not own stops and reports; it does not make the
  change.
- Seams first. A skeleton row registers every planned module, command, route group or trait as a stub that compiles
  and is tested as a stub, so a later row only fills its own files: RR1 lists every planned workspace member; RA2
  the whole command list; RH1 the full `clap` tree, one module per command; RB3 the whole schema; RB5 one trait per
  Paradox repository, one router per area with its routes returning 501, the auth extractor's signature and the
  clock and id generator.
- A row delivers its tests, its fuzz target and its `docs/security/<component>.md` with its code (RX3). Strings a
  launcher row needs are appended at the end of both language files; the integration pass resolves the resulting
  conflicts, which are append-only.
- The integration pass closes a wave: one agent, or the maintainer, merges in dependency order, runs CI on both OSes
  and the smoke list (section 6) on the canary when the wave changed something that runs there, strikes the rows
  here and updates the What's-left table. It is the only thing that edits the shared files below.
- Not an agent's work: the canary and every cutover, RA10 and RA12a's hands-on parts (a Linux machine with the game,
  Proton and a host's SDK), signing keys and publishing a release, the cut-off date and telling Freeza (RD16), the
  24-hour fuzz runs and the signed checklists (RD10).

| File or folder | Owner | Other rows |
| --- | --- | --- |
| `rust/Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.cargo/`, the lint configs | RR1 | Never; a crate is added only by its skeleton row |
| `.github/` | RR3; RH6 for `server-release.yml`; RA14 for `launcher-release.yml` | Never |
| `NOTICE.md`, `README.md`, `docs/` index pages, `SECURITY_REVIEW.md`, `MIGRATION_PLAN.md` | Integration pass | Never; a row writes its own page under `docs/` |
| `crates/protocol` | RS1 | Never after RS1; a needed type goes through the integration pass |
| `services/host`: `main.rs`, the `clap` tree, `paths`, `envfile`, `hostconfig` | RH1 | Fill the one command module the row owns |
| `services/backend`: `main.rs`, config, the trait files, router registration, the auth extractor | RB5 | Fill the one area module and its Postgres repository the row owns |
| `crates/store` migrations and `schema.rs` | RB3 | Never after RB3; a change is a new migration through the integration pass |
| `launcher/src-tauri`: `main.rs`, the bridge spec, generated types, the isolation hook | RA1–RA2 | Fill the one module the row owns |
| `paradox/` | RP1–RP4; then fixes only (RD2) and RX2's catch-up | Never |
| `legacy/` | Deleted by RS5, RH7, RA16 | Never |

### 5.2 Before the gate

| Wave | Row (one agent each unless said) | Needs | Hands-on |
| --- | --- | --- | --- |
| 0 | RX1, three agents: gateway, allowlist and content; launcher net and platform; launcher controller, bridge and page | — | — |
| 0 | RP1 in the fork | — | — |
| 0 | RR1 copy of `rust/` | — | — |
| 0 | RR2 moves to `legacy/` | — | — |
| 0 | RA10 spike: 1.14.7 under Proton with the runtime loaded | — | Linux machine with the game |
| 1 | RP2–RP4 | RP1, RR2 | — |
| 1 | RR3 CI, RR4 Dependabot (the `paradox/` job waits for RP3 to exist, it does not block) | RR1, RR2 | — |
| 1 | RS1 protocol | RR1, RX1 | — |
| 1 | RS4 allowlist helper | RR1, RX1 | nft rules on Debian 13 |
| 1 | RA1–RA2 launcher skeleton and bridge | RR1, RX1 | — |
| 1 | RH1 host kit skeleton | RR1 | — |
| 2 | RS2 content | RS1, RX1 | — |
| 2 | RS3 gateway | RS1, RS4 (same crate), RX1 | — |
| 2 | RH2 `start` (starts the Node services and MongoDB until the Rust ones exist) | RH1 | — |
| 2 | RH3 `setup`, `invite`, `doctor`, `multipliers`, MongoDB | RH1 | — |
| 2 | RH4 `gamedata`, `preset`, `progression`, `publish-runtime`, `gamepack`, `captures` | RH1, RP3 | — |
| 2 | RA3 languages | RA1 | — |
| 2 | RA4 + RA5 invite, status, TLS, HTTP, host API | RA2, RS1 | — |
| 2 | RA6 relay | RA2 | — |
| 2 | RA7 downloader and verify | RA2, RS1 | — |
| 2 | RA8 settings, log, keystore and legacy reader | RA2 | A real 1.5.1 profile |
| 2 | RA9 + RA11 game config, folders, launch, links, window | RA2 | — |
| 3 | RH6 release workflow, then RH5 `update` and install from it | RS2–RS4, RH2–RH4 | — |
| 3 | RA12 Host page backend and injector, Windows | RA2, RH2 | A 1.14.7 client |
| 3 | RA10 port (if the spike passed) | RA9 | Linux machine |
| 3 | RX2 lists for gateway, content and host kit | RR1 (the copy commit) | — |
| 3 | Shared check: the Tauri debug build against the Rust gateway and content, with the Node backend | RS2, RS3, RA4–RA9 | Canary |
| 3 | Front-door cutover: canary, then every host through `update`; RX2's items for gateway, content and host kit ported first | RH5, RH6, RX2, the shared check | Canary, hosts |
| 3 | RA13 controller | RA3–RA12 | — |
| 3 | RA12a spike, then port if it passes | RA10 port, RA12 | Linux machine, SDK |
| 3 | RS5 and RH7, one agent: port the remaining catch-up items, delete the three legacy folders | The cutover | — |
| **Gate** | **The Rust front door and `reforged-host` are live on every host (RD17)** | RS5, RH7 | — |

Wave 2 is the widest, eleven rows; the servers and host kit rows and the launcher rows can be merged in two separate
integration passes, since they share no files.

### 5.3 After the gate

| Wave | Row (one agent each unless said) | Needs | Hands-on |
| --- | --- | --- | --- |
| 4 | RB1 route inventory, two agents: backend, Director | Gate | — |
| 4 | RB3 `pq-sys` spike, then the schema; the critical path of this half of the plan | Gate | — |
| 4 | RB10 Director (no database; `service-kit` only) with `director hunt-tables import` from RB11 | Gate | Game servers to start |
| 4 | RB11 game-data generators (pure file-to-file; `admin bootstrap` waits for RB5) | Gate | — |
| 4 | RB2 traffic capture on the canary; the cut-off date set and Freeza told (RD16) | Gate | Canary, testers |
| 4 | RA14 2.0.0-beta packages and release workflow | RA13 | Signing keys |
| 4 | RA15 Electron 1.6.0 bridge | RA14's installer hash | — |
| 5 | RB4 `db migrate` and `db verify` | RB3 | Copies of real databases |
| 5 | RB5 backend skeleton and seams | RB3 | — |
| 6 | RB6 auth | RB5, RB4 | — |
| 6 | RB7 Reforged API, login queue, maintenance status | RB5, RB4 | — |
| 6 | RB8 in five agents: characters and loadouts; inventory, storage policy and reforge; progression, mastery, escalation and session supplies; bounties, store, Lady Luck and platinum; multipliers, matchmaking and parties, the game servers' posts | RB5, RB4 | — |
| 6 | RB9 chat | RB5; RB6 for the XMPP auth, plugged in last | — |
| 6 | RB11 `admin bootstrap` and the data migrations folded into `db migrate`, by RB4's agent | RB4, RB5 | — |
| 6 | RB13 `db export-mongo` | RB4 | — |
| 6 | RB12 harness and normalizer, proven first on Node against Node (no diffs expected) | RB2, RB4 | — |
| 7 | RB12 replay; the diffs handed back one agent per RB8 group, RB6, RB7, RB9, RB10 | RB6–RB11, the harness | — |
| 7 | RX2 catch-up for the backend and Director, by the same agents, then the final replay | RB12 | — |
| 7 | RB13 rehearsal | RB13, the replay clean | Canary |
| 8 | RB14 cutover: canary, a week, then hosts | RB13 rehearsal, the signed checklists (RD10) | Canary, hosts |
| 8 | RA16 and RR5, one agent, when 3.0.0 is due | RA15 shipped | — |
| 9 | RB15 | The last host's rollback window closed | — |

Wave 6 is ten rows; RB13 and the RB12 harness can wait for a second integration pass. The Electron 1.5.x launcher
keeps working against the Rust front door, so the wave-3 cutover does not wait for 2.0.0, and the front door is live
long before the backend changes.

## 6. Things that must not change

- Launcher: the account keys already stored (read once, then re-stored), their binding to the certificate
  fingerprint, the `reforged-revival://` invite formats v1/v2, relay port 61100, the runtime acceptance per server
  and the check before every launch, the 1.14.7 executable hash, the page's look and both languages word for word;
  the key never crosses the bridge or reaches a log.
- Servers: the gateway's certificate and so every invite's `fp`; every route the game, the runtime and the launcher
  use, matched as Express matches them; JSON replies equal in value and in text (DR SB1); the XMPP stanzas the
  client reads; the host-only routes and headers stay refused; the content server's file layout; every player's
  data, unchanged on MongoDB and then carried over field for field to Postgres (RB4); API keys, password hashes
  and tokens issued by the Node backend keep working.
- Host kit: `host.json` and the `.env` files it writes, and every command's name and options except the two RH5
  names (`update`'s meaning, `npm run upstream` removed), so an existing host moves over with `update`.
- Smoke list at every step: register with an invite, login, Ramsgate, a hunt, chat in the City room, runtime
  download, ban, clean stop.

## 7. Open questions

- The cut-off date (RD16) is not set. It needs Freeza told, and a date no later than RB2.
