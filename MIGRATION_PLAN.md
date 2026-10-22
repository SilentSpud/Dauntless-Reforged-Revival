<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Dauntless Reforged Revival migration plan: the launcher and the front door in Rust

Status: draft, written 2026-10-22. Nothing in it has started.

This plan replaces Dauntless Revived's Rust migration as the active port. That migration was frozen on 2026-10-22
at `e1c5fec` on its `plan` branch (tag `rust-port-frozen`, [its plan](https://github.com/mixutin/dauntless-revived/blob/plan/MIGRATION_PLAN.md)).
Most of what it built was written for code this repository already shares: Reforged's launcher, gateway, allowlist
helper and content server all started as Dauntless Revived's. This plan takes that Rust code over and fits it to
1.14.7 and Mystic Paradox, rather than porting Reforged's Node code a second time.

Row ids: **RA** = launcher, **RS** = server components, **RH** = host kit, **RR** = repository and CI,
**RX** = cross-cutting. **RD** = decisions. "DR" means Dauntless Revived at `rust-port-frozen`; DR row ids
(L27, S2, X4, …) point into its plan, where the method and the checks for each piece are written down.

## 1. End state

| Component | Today | After |
| --- | --- | --- |
| `ReforgedLauncher` | Electron 44, Node main process (~8.2k lines with `main.ts` and `preload.ts`), Squirrel, Forge, Windows only | Tauri 2 on DR's launcher backend (`src-tauri`, ~20k lines), Rust backend, the same TS page, isolation pattern, `tauri-plugin-updater`, NSIS, Windows only (RD3) |
| `ReforgedGateway` (+ allowlist helper) | Node, `node-forge` (~2.8k lines) | DR's Rust `gateway` (bins `gateway`, `allowlist`) with Reforged's routes, ports and nft changes |
| `ReforgedContent` | Node (~1.4k lines) | DR's Rust `content` with Reforged's runtime manifest (`runtime.ts`) added |
| `ReforgedHost` | Node TS CLI (~4.8k lines with `linux/`) | Stays TypeScript on Node (RD2); starts the Rust binaries instead of `node dist/...` |
| ParadoxBackend, ParadoxDirector | Mystic Paradox (Node, MongoDB) at a pinned commit + 33 patches | Unchanged (RD2) |
| Game runtime (`MysticParadox.dll`, `winmm.dll`) | C++, patched upstream, built by each host | Unchanged |
| Repository | One npm package per folder | DR's shape for the parts that move: a `rust/` cargo workspace; Node projects being replaced under `legacy/` until deleted (RR rows) |

## 2. What is reused from DR

Measured 2026-10-22 by diffing Reforged's Node sources against DR's at the freeze. "Delta" is the Reforged change
that has to be carried onto the Rust code; the line counts are changed lines in the Node files, a rough guide only.

| DR piece (at `rust-port-frozen`) | Reforged counterpart | Reuse | Delta to carry over |
| --- | --- | --- | --- |
| `rust/crates/protocol` (invite, status, manifest, account) | `src/shared/invite.ts`, `status.ts`, `types.ts` | Most | Second scheme `reforged-revival://` beside `dauntless-revived://`; `RRK_` keys; a "flavor" on status and register (Reforged's `hostapi.ts` already has `REVIVED_FLAVOR`); the runtime manifest type |
| `rust/crates/proxy` (hop-by-hop, WebSocket tunnel) | relay and gateway proxying | Whole | None expected; `/__origin/<host><path>` rewriting stays in the relay |
| `rust/crates/service-kit` (axum setup, pino logs, env config, X7 service contract) | gateway and content start-up | Whole | None; Express-compatible routing is unused by these two but harmless |
| `rust/crates/store` (Diesel, SQLite) | none: Reforged's data is in MongoDB | Not brought in | — |
| `rust/services/gateway` (TLS, policy, IP checks, limits, redaction, feed, `cert.rs`, `allowlist` bin) | `ReforgedGateway/src` (allowlist `state`, `sync`, `ip`, `ratelimit`, `redact`, `log` identical) | Most | `policy.ts` (~108 lines: `/reforged/api` admin routes, Paradox's host-only routes and headers), `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`; new `allowlist/nft.ts` and `ports.ts` (the game's UDP range from `ALLOWLIST_PORTS`, which `--max-hunts` widens); `tools/make-cert.js` becomes `gateway cert` |
| `rust/services/content` (ranges, news, branding, auth) | `ReforgedContent/src` (`branding`, `news`, `limits`, `log` identical) | Most | New `runtime.ts` (the two-file runtime manifest and download); `app.ts` (~218 lines: no 11 GB game zip), `auth.ts` against Paradox's key check, `config.ts` |
| `rust/services/metagame`, `deploy`, `discord-bot` | ParadoxBackend, ParadoxDirector, none | Not brought in (RD2) | — |
| Launcher `src-tauri`: `net/pinned.rs`, `net/http.rs`, `net/relay.rs`, `net/hostapi.rs`, `net/verify.rs`, `net/downloader.rs` | `pinned.ts` (identical), `http.ts`, `relay.ts`, `hostapi.ts`, `verify.ts`, `revived/downloader.ts` | Most | Relay: port 61100, `/__origin/` routing, `/QoS` answered locally; downloader: also the runtime's two files; verify: the 1.14.7 executable hash |
| Launcher `platform/keystore` (+ `legacy.rs` safeStorage reader, DR L27, D7) | `keystore.ts` (~14 lines changed) | Windows path whole | One key per server, bound to the certificate fingerprint; read Reforged's `userData` (different app name and folder) |
| Launcher `platform/settings.rs`, `log.rs`, `system.rs`, `window.rs`, `invite_link.rs`, `game_folder.rs`, `game_config.rs`, `launch.rs` | `settings.ts`, `log.ts`, `system.ts`, `links.ts`, `game-folder.ts`, `engineini.ts`, `launch.ts`, `display-repair.ts` | Most | Engine.ini XMPP section and Game.ini `[OnlineSubsystemPhoenix]` removal; the 1.14.7 command line and `REFORGED_BACKEND_URL`; Legendary/Heroic/Epic detection |
| Launcher `controller/`, `bridge/` (typed commands, generated TS types, isolation hook) | `controller.ts` (~2.2k lines changed), `ipc-validate.ts`, `preload.ts` | Structure | The controller is the largest delta: Host page state, runtime acceptance, two game flavors |
| Launcher `platform/updater.rs`, Squirrel bridge (DR L45–L50) | `update-electron-app` + Squirrel | Whole | Feed URL and version line (RD4) |
| Launcher `net/autoregion.rs` | `revived/api.ts` (hunt region) | Whole | Only for the Dauntless Revived flavor |
| Launcher `offline/`, `controller/offline.rs`, `platform/linux_runtime.rs`, `platform/client_mods.rs` | none | Not brought in (RD3, RD5) | — |
| Launcher page split per language (DR L13–L15) | `src/shared/i18n.ts` (1.6k lines, en + pt-BR) | Method | Two languages instead of nine |
| CI by family, `cargo deny`, fuzz targets, `SECURITY_REVIEW.md` checklists (DR R8, X6, X9) | `ci.yml`, `launcher-release.yml` | Most | Windows-only launcher job; no metagame or bot jobs |

Not in DR at all, so written new in Rust: the Host page backend (`hostkit.ts`, `hostserver.ts`, `requirements.ts`,
~980 lines), the DLL injector (`injector.ts`, RD7) and the runtime files (`runtime-files.ts`, ~290 lines).

## 3. Decisions

| # | Topic | Decision | Consequence |
| --- | --- | --- | --- |
| RD1 | How the DR code comes in | **Copied once, with its provenance**, not a submodule or subtree: `upstream/dauntless-revived.json` pins the DR commit (`e1c5fec`) like `mystic-paradox.json` pins Mystic Paradox, and lists each copied path. History stays in DR. Later DR fixes come over by hand through a catch-up list (RX2), and Reforged fixes that apply to DR go on the back-port list (RX3) | One repository to build; no cross-repository build step. The copy diverges, so every RX2/RX3 entry is a manual review |
| RD2 | Backend, Director, host kit | **Not ported.** ParadoxBackend and ParadoxDirector stay Mystic Paradox's Node/MongoDB code with Reforged's patches; DR's Rust metagame (SQLite, Dauntless 1.4.4 routes) does not fit them. `ReforgedHost` stays TypeScript on Node, since the host needs Node for Paradox anyway | Hosts still need Node.js 20.19+ and MongoDB. Players need nothing but the launcher |
| RD3 | Launcher platforms | **Windows only**, as today: 1.14.7 and its runtime are Windows programs. DR's Linux code (Proton/Wine runtime, Secret Service and KWallet keystore, AppImage/deb/rpm) is left out, not compiled behind a flag | Smaller backend and CI; a Linux launcher later starts again from DR's code |
| RD4 | Updates | DR's design (D2): `tauri-plugin-updater` with ed25519 signatures, NSIS, and a one-time Squirrel bridge. The bridge is Electron **1.6.0**, which installs Tauri **2.0.0**; the Squirrel feed is retired with 3.0.0 | One extra Electron release; the Electron key copies are kept until 3.0.0 so a rollback works |
| RD5 | Offline play | None: Reforged has no offline mode, so DR's `offline/` and `--service` modes are not brought in | — |
| RD6 | The two game flavors | Kept: 1.14.7 servers (`reforged-revival://`) and Dauntless Revived servers on 1.4.4 with 0.1.35's pins (`dauntless-revived://`, `src/main/revived/`). The flavor is a field of the stored server, not a second code path through the controller | `protocol` gains the flavor; DR's 0.1.39 pins are not used, 0.1.35's stay in `revived/constants` |
| RD7 | DLL injector (Host page) | Rewritten on the `windows` crate (`OpenProcess`, `VirtualAllocEx`, `CreateRemoteThread(LoadLibraryW)`), only into the game process the launcher started, only a DLL the host picked | No PowerShell or C# compile step; it is the most sensitive new code, so it gets its own security checklist (RX4) |
| RD8 | Host page running the host kit | Unchanged behaviour: the launcher runs `ReforgedHost` with the system Node as a child process and stops it on exit (Windows job object instead of the current workaround) | The Tauri backend needs `std::process` plus a job object, no shell |
| RD9 | Languages | en and pt-BR, as today, split one file per language as DR did (L13), so a third language is data only | — |
| RD10 | Repository layout | DR's shape for what moves: `rust/` (one workspace: `crates/protocol`, `crates/proxy`, `crates/service-kit`, `services/gateway`, `services/content`, `launcher/src-tauri` after RR5), Node projects being replaced under `legacy/`; `ReforgedHost/`, `upstream/`, `preset/`, `brand/`, `docs/` stay where they are | The host kit's paths to the gateway and content change once (RH1) |
| RD11 | Things that must not change | Section 6 | — |

## 4. Work

### 4.1 Repository and CI (RR)

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RR1 | Add `upstream/dauntless-revived.json` (commit, path list) and its README section | — | Paths in the pin exist at the pinned commit |
| RR2 | Copy `rust/` (workspace files, `rust-toolchain.toml`, `deny.toml`, `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`, `crates/protocol`, `crates/proxy`, `crates/service-kit`, `fuzz/` targets for those crates) and set `repository` and the members | R4, X6 | `cargo test --workspace`, `cargo deny check`, `cargo clippy` pass on Windows and Linux |
| RR3 | `git mv` `ReforgedGateway`, `ReforgedContent`, `ReforgedLauncher` to `legacy/`; the host kit and CI follow the new paths | R5 | Today's npm tests pass from `legacy/` |
| RR4 | CI by family behind one required check: Rust (fmt, clippy, test, deny, short fuzz), launcher page, legacy Node, host kit | R8 | A Rust-only PR skips the Node jobs and the other way round |
| RR5 | Move the launcher to `rust/launcher/` once Electron is deleted (RA14) | R7 | Paths sweep finds no `legacy/ReforgedLauncher` |

### 4.2 Server components (RS)

Same order as DR: the content server first (smallest, read-only), then the gateway and the allowlist helper.

| # | What | From DR | Delta | Verify |
| --- | --- | --- | --- | --- |
| RS1 | `protocol`: Reforged invite scheme, `RRK_` keys, flavor on status and register, runtime manifest type | X4 | §2 | DR's protocol tests plus Reforged's `invite.test.ts` cases as Rust tests |
| RS2 | Content server: copy `services/content`, drop the game zip, add the runtime manifest and the two-file download, auth against Paradox's key check | S1 | `runtime.ts`, `app.ts`, `auth.ts`, `config.ts` | Reforged's content tests ported; the Node launcher downloads a runtime from the Rust server |
| RS3 | Gateway: copy `services/gateway`, apply `policy.ts` (Paradox and `/reforged/api` admin routes, host-only headers), config, feed and server deltas; `gateway cert` replaces `tools/make-cert.js`; **load the existing certificate, never regenerate** | S2 | `policy.ts`, `config.ts`, `gateway.ts`, `feed.ts`, `server.ts`, `make-cert.js` | Reforged's gateway tests ported; DR's parity test re-pointed at Reforged's Node gateway; an existing invite's `fp` still matches |
| RS4 | Allowlist helper: DR's `allowlist` bin with Reforged's game-port range (`ports.ts`) and its nftables variant (`nft.ts`) for the Linux VPS | S3 | `ports.ts`, `nft.ts`, `config.ts`, `firewall.ts` | Rule contents after login and logout on Windows and on Debian 13 (nft) |
| RS5 | Catch-up of Reforged's Node gateway and content commits since the copy (RX2), then delete `legacy/ReforgedGateway` and `legacy/ReforgedContent` | X3 | — | Every commit on the list ported or marked not applicable |

### 4.3 Host kit (RH)

| # | What | Verify |
| --- | --- | --- |
| RH1 | `services.ts`, `run.ts`, `paths.ts`: start `gateway`, `allowlist` and `content` release binaries with `--env-file` (DR X7) instead of `node dist/...`; `doctor` checks the binaries | `npm run start` brings every service up; Ctrl+C stops them cleanly |
| RH2 | `setup` and `update` fetch the binaries from the project's release (with `SHA256SUMS.txt`) or build them when `cargo` is present | A fresh `setup` on Windows and on the Linux VPS path |
| RH3 | `linux.ts`, `ReforgedHost/linux/`: the Linux VPS gets the `x86_64-unknown-linux-gnu` binaries | `docs/hosting-linux.md` followed on a clean Debian 13 |

### 4.4 Launcher (RA)

Phases follow DR's Part A. The DR rows in brackets hold the method; the Reforged work is the delta.

| # | What | From DR | Verify |
| --- | --- | --- | --- |
| RA1 | Copy `src-tauri` without `offline/`, `linux_runtime.rs`, `client_mods.rs`, the Linux keystore and the Linux bundle config; Windows-only `tauri.conf.json`; the app's identifier, name and `userData` folder are Reforged's | L4–L7 | It builds and opens today's page with a stub bridge |
| RA2 | Bridge, generated TS types and isolation hook re-generated for Reforged's command list (`ipc-validate.ts`, `preload.ts`) | L6, L7, L9 | Every command the page uses is in the bridge spec; a refused command never reaches Rust |
| RA3 | Language files split (en, pt-BR) and the backend's text built from them | L13–L15 | Reforged's `i18n.test.ts` passes on the split files |
| RA4 | Invite, status, username on `protocol` (RS1) | L11, L12 | Shared test tables with the servers |
| RA5 | Pinned TLS, HTTP client, host API with both flavors | L19–L21 | Reforged's `pinned.test.ts` cases; register and status against the Rust gateway |
| RA6 | Relay on 61100 with `/__origin/` routing, chat WebSocket and local `/QoS`; owner check | L22, L33 | Reforged's `relay.test.ts` cases |
| RA7 | Downloader and verify: the runtime's two files (accept once per set, check before every launch) and 1.4.4's manifest for the DR flavor | L23–L25 | Reforged's `runtime.test.ts`, `revived.test.ts` cases |
| RA8 | Settings, log, folders; keystore with the safeStorage reader for Reforged's existing keys (per server, bound to `fp`) | L26–L29, LB1 | A real 1.5.1 profile's keys read back and work |
| RA9 | Game config (Engine.ini, Game.ini, display repair), game folder detection (Legendary, Heroic, Epic, folder), launch with `REFORGED_BACKEND_URL` and the 1.14.7 command line; DR flavor with 0.1.35's DLL pins | L30, L31 | Reforged's `game.test.ts`, `launch.test.ts`, `display-repair.test.ts` cases |
| RA10 | Links, dialogs, window, invite-link scheme registration for both schemes | L5, L34–L36 | Clicking either scheme opens the running launcher |
| RA11 | Host page backend: workspace detection (`hostkit`), running the host kit (`hostserver`, RD8), requirements, injector (RD7) | — | Reforged's `hostkit.test.ts`, `hostserver.test.ts` cases; the injector loads CatalogExporter into a 1.14.7 client |
| RA12 | Controller ported and every page command forwarded | L38, L39 | Reforged's `controller.test.ts` and `ui.test.ts` cases |
| RA13 | Package, sign updates, release workflow; 2.0.0-beta; the 1.6.0 Squirrel bridge (RD4) | L45–L50 | Install, update and uninstall on a clean Windows VM; 1.5.1 → 1.6.0 → 2.0.0 keeps keys and settings |
| RA14 | 3.0.0: retire the Squirrel feed, delete the Electron key copies, delete the Electron main process and Forge | L50a, L51 | — |

### 4.5 Cross-cutting (RX)

| # | What |
| --- | --- |
| RX1 | Per-file delta lists: for each Reforged Node file, `diff` against DR's Node file at `rust-port-frozen`, written into the RS/RA row that ports it, so nothing Reforged changed is lost in the Rust copy |
| RX2 | Catch-up lists from the copy commit: Reforged's own Node commits to the launcher, gateway and content (ported before each one's cutover), and DR's Rust commits after `rust-port-frozen` if DR resumes |
| RX3 | Back-port list: fixes made here to copied DR code, recorded so they can go back to DR |
| RX4 | Security pass per component (DR D26, X9): gateway policy and TLS, allowlist helper, relay, pinned TLS, keystore and legacy reader, invite parsing, runtime download, the injector; fuzz targets for every parser in CI, a 24-hour run before each cutover |

## 5. Order

| Step | Servers | Launcher | Host kit |
| --- | --- | --- | --- |
| 1 | RR1–RR4, RS1 | RA1–RA3 | — |
| 2 | RS2 content | RA4–RA7 | — |
| 3 | RS3 gateway, RS4 allowlist | RA8–RA10 | RH1 |
| 4 | Cutover on one test host; RS5 | RA11–RA12 | RH2, RH3 |
| 5 | Every host on the Rust services | RA13: 2.0.0-beta, then the bridge | — |
| 6 | — | RA14, RR5 | — |

The servers can cut over before the launcher: the Electron 1.5.x launcher talks to the Rust gateway and content
server unchanged, which is the first real check of RS2–RS4.

## 6. Things that must not change

- Launcher: the account keys already stored (read once, then re-stored), their binding to the certificate
  fingerprint, invite formats v1/v2 of both schemes, relay port 61100, the runtime acceptance per server and the
  check before every launch, the 1.14.7 executable hash, the page's look and both languages word for word; the key
  never crosses the bridge or reaches a log.
- Servers: the gateway's certificate and so every invite's `fp`; every route the game, the runtime and the
  launcher use; the host-only routes and headers stay refused; the content server's file layout; log field names
  the host kit and `doctor` read.
- Host kit: its commands, `host.json` and the `.env` files it writes, so `npm run update` moves an existing host over.

## 7. Open questions

- Keep the Dauntless Revived (1.4.4, 0.1.35 pins) flavor in the Tauri launcher, or drop it at 2.0.0?
- Should `ReforgedHost` move to Rust (or Bun, DR's D20) later? Not while Paradox needs Node on the host (RD2).
- Release binaries for the host kit: built by CI and downloaded (RH2), or built on each host with `cargo`?
