# Plan: configuration in TOML

Draft, 2026-10-24. Scope: Dauntless Reforged Revival first; Dauntless Revived at the end.

**Target: the Rust host kit on `plan`** (`rust/services/host`, the launcher's `platform/hostkit`), not the Node
projects in `legacy/`. The file and function names below come from the Node kit; the Rust port keeps the same
ones (`hostconfig.rs`, `buildEnvFiles` → `build_env_files`, and so on).

**Status:** Phase 1 is done on `feat/host-toml`. `hosttoml.rs` reads and patches the file with `toml_edit`, so the
host's own comments survive setup. It still needs a decision row amending MIGRATION_PLAN.md section 6, which
lists `host.json` as a file that must not change.

## Where configuration lives today

| File | Written by | Read by | Hand-edited? |
|---|---|---|---|
| `.workspace/config/host.json` | `npm run setup` and its flags (`ReforgedHost/src/cli.ts`) | ReforgedHost (`hostconfig.ts`), launcher Host page (`ReforgedLauncher/src/main/hostkit.ts`) | Supposed to be through setup flags only; people edit it anyway |
| `.workspace/config/{backend,director,content,gateway,allowlist}.env` | `buildEnvFiles()` in `hostconfig.ts`, from host.json + secrets.env | the services, through `services.ts` | No ("Run setup again instead of editing") |
| `.workspace/config/secrets.env` | `loadOrCreateSecrets()`, once | ReforgedHost | No |
| `.workspace/config/news.json` | the host | ReforgedContent (`CONTENT_NEWS_FILE`) | Yes |
| `.workspace/config/session-supplies.json` | the host | Mystic Paradox backend (patch 0023) | Yes |
| `.workspace/config/allowlist-state.json` | the allowlist helper | the allowlist helper | No (state) |
| launcher `userData/settings.json` | the launcher | the launcher | No (app state) |
| `preset/**/*.json`, `upstream/*.json`, `brand/palette.json` | generators and pin tools | build and host kit | No (generated data, checksummed) |

Two things limit what a host can tune today:

1. **host.json can't carry comments**, so every field's meaning lives in `hostconfig.ts` and the docs, not in the file.
2. **Service tunables aren't reachable.** The gateway reads about 25 limits, rate buckets and timeouts from env (`ReforgedGateway/src/config.ts`), and Content and the allowlist helper read several more. None of them are in host.json, and setup overwrites the `.env` files. Values like `HUNT_GO_NOW_*`, `MONGODB_TRANSACTION_MAX_COMMIT_TIME_MS`, `GAMESERVER_READY_TIMEOUT_MS` and `LOG_LEVEL` are hard-coded in `buildEnvFiles()`.

## Principles

- **TOML is for files people write.** Generated data, checksummed preset files, machine state and tool manifests (`package.json`, `tsconfig.json`) stay JSON.
- **Env stays the wire format to the services.** The Mystic Paradox backend and the Director are upstream code that reads env, and our own services (Gateway, Content, allowlist helper) are also run and tested standalone with env. ReforgedHost renders the TOML into the same `.env` files it writes today. The services themselves don't change.
- **host.toml is the single source of truth.** Anything a host may tune goes in host.toml and is rendered into env. Nobody edits a `.env` file.
- **Migrate silently, never break an existing server.** Old files are read, converted once and kept as a `.bak` file.

## Library

In the Rust port: `toml_edit` (already in the lockfile through Tauri). A new file comes from a commented template. After that, every write patches the file as read and rewrites only the values that changed, so the host's own comments, order and spacing survive setup.

## Phase 1: host.json → host.toml

- `hostconfig.ts`: `readHostConfig()` reads `host.toml`. If only `host.json` exists, it converts it, writes `host.toml` and renames `host.json` to `host.json.bak`. `writeHostConfig()` writes TOML from the template. `validateHostConfig()` stays as it is, since the shape doesn't change.
- **Keys are snake_case** (`max_hunt_servers`, `game_address`, `performance.server_fps`), the usual TOML style. The `HostConfig` interface stays camelCase in TypeScript: one key map in `hostconfig.ts` converts both ways, and the migration renames every key from the old host.json. An unknown key in host.toml is an error that names the closest known key, so a leftover camelCase key (`maxHuntServers`) gets "did you mean max_hunt_servers?" instead of being silently ignored.
- TOML specifics: `null` doesn't exist in TOML, so the nullable fields (`game_dir`, `share_url`, `mongo.mongod`, `linux.wine`, `linux.proton`, `linux.firewall_interface`) become **omitted keys**, mapped back to `null` on read. Integers and floats both parse to `number` (multipliers like `1.5` and `1` both work).
- `paths.ts`: `hostJson` → `hostToml`, plus `hostJsonLegacy` for the migration.
- Callers: `cli.ts` (setup, multipliers, the performance and linux flags), `doctor.ts`, `mongo.ts`, `preset.ts`, and the launcher's `hostkit.ts`. The launcher parses the file itself, so it needs the TOML reader too, and it has to keep reading host.json for workspaces that haven't run the new setup yet.
- Tests: round trip (template → parse → validate), migration from a host.json of each older shape (no `performance`, no `linux`, no `publicPort`), omitted-key ↔ null.
- Docs: `docs/hosting.md`, `docs/hosting-linux.md` (both the English and Portuguese sections), `ReforgedHost/README.md`, plus an annotated `host.example.toml`.

Sketch:

```toml
# Dauntless Reforged Revival server. Written by `npm run setup`; safe to edit by hand,
# then run `npm run setup` again to apply. Your own comments are not kept.
version = 1
mode = "public"                 # "private" (Tailscale) or "public"
host = "play.example.com"
game_address = "203.0.113.7"
name = "Freeza"
game_dir = 'D:\Games\Dauntless'
registration = "INVITECODE"     # INVITECODE, OPEN or NONE
max_hunt_servers = 6
public_port = 443

[mongo]
uri = "mongodb://127.0.0.1:27017/?replicaSet=rs0"
manage = true

[performance]
server_fps = 30
idle_fps = 10
training_on_demand = true
training_idle_minutes = 5
verbose_game_logs = false
memory_guard = true
min_free_memory_mb = 1024
hunt_memory_mb = 1536
```

## Phase 2: service tunables in host.toml

Each table is optional. A missing key keeps today's default, and the defaults stay defined in one place (the service's own `config.ts`, or the constant in `buildEnvFiles()`).

| Table | Keys → env | Source of the defaults |
|---|---|---|
| `[gateway.limits]` | `max_body_bytes`, `max_connections`, `max_connections_per_ip` → `GATEWAY_MAX_*` | `DEFAULT_LIMITS` |
| `[gateway.rate]` | `general`, `content`, `register`, `token`, `ws`, `connect` as `{ burst, per_minute }` inline tables → `GATEWAY_RATE_*` (formatted the way `ParseBucketSpec` expects) | `DEFAULT_LIMITS.rate` |
| `[gateway.timeouts]` | `handshake_ms` … `shutdown_grace_ms` → `GATEWAY_*_TIMEOUT_MS` | `DEFAULT_TIMEOUTS` |
| `[gateway]` | `allowlist_refresh_seconds` | 60 |
| `[content]` | `max_streams_per_account`, `max_streams_total`, `auth_cache_seconds`, `news_url` | `ReforgedContent/src/config.ts` |
| `[allowlist]` | `ttl_seconds`, `min_interval_ms`, `max_entries`, `nft_table` | `allowlist/config.ts` |
| `[backend]` | `log_level`, `transaction_max_commit_ms`, `hunt_go_now_min_ms`, `hunt_go_now_max_ms`, `xmpp_capture` | constants now in `buildEnvFiles()` |
| `[director]` | `log_level`, `gameserver_startup_gap_seconds`, `gameserver_ready_timeout_ms` (default still depends on the platform) | constants now in `buildEnvFiles()` |

Two additions make this safe:

- **Escape hatch**: `[env.backend]`, `[env.director]`, `[env.gateway]` and so on, for raw `KEY = "value"` pairs appended to that service's env. This covers upstream variables we haven't named. A **denylist** rejects keys host.toml already owns: secrets, ports, bind addresses, `MONGODB_URI`, `APPROVED_EXECUTABLE_SHA256`, `AUTH_*`, `REFORGED_SERVER_MODE`, the multipliers. Without the denylist, the passthrough could switch a public server into private-only features. `npm run doctor` lists every passthrough in use.
- **Early validation**: after rendering, setup and `doctor` run `LoadGatewayConfig`, `LoadConfig` (Content) and `LoadAllowlistConfig` against the rendered env maps. A bad value fails setup with the service's own message ("GATEWAY_RATE_WS must be …"), not a crash loop at start. That needs those loaders exported in a form ReforgedHost can import, or a `--check-config` mode in each service.

## Environment variables override the config

Decision: a variable set in ReforgedHost's own environment wins over the value rendered from host.toml, and every override is logged as a warning.

Today it's the opposite. `serviceEnvironment()` in `ReforgedHost/src/services.ts` **removes** every service variable from the inherited environment (`GATEWAY_*`, `MONGODB_*`, `PORT`, `LOG_LEVEL`, …), so a stray shell variable can never reach a service. Changes:

- **Order**: rendered env file, then `extraEnv`, then the inherited variables that match a key the service reads. The inherited variable is applied last and wins.
- **Which variables count**: a key in that service's rendered env file, or a key named in that service's Phase 2 table or `[env.<service>]` passthrough. Other inherited variables pass through as today (PATH, SystemRoot …).
- **Generic names need a service prefix.** `PORT`, `BIND_HOST`, `NODE_ENV`, `LOG_LEVEL`, `MY_IP` and `BACKEND_URL` are often set globally on a dev box or by a hosting panel, and the backend, Director and Content all read `PORT`. Override them as `REFORGED_<SERVICE>_<NAME>` instead (`REFORGED_BACKEND_PORT`, `REFORGED_DIRECTOR_LOG_LEVEL`), which ReforgedHost renames before it starts the service. A bare `PORT` in the environment is still dropped, with its own warning saying which prefixed name to use.
- **Secrets and safety keys can be overridden too**, as asked. The warning for those is louder, and the value is redacted (only the key and a "value redacted" note are logged). Covered keys: anything in `SECRET_KEYS`, `AUTH_*`, `REFORGED_SERVER_MODE`, `APPROVED_EXECUTABLE_SHA256`, the multipliers. The backend still checks for itself that multipliers run only on a private Tailscale server.
- **The warning**: one line per overridden key, each time a service starts or restarts, in both the console and the host log:
  `WARN gateway: GATEWAY_RATE_WS=40/24 from the environment overrides host.toml (gateway.rate.ws = 20/12)`
  Secret keys show `(value redacted)` in place of both values. Restarts log it again, so a long-running server doesn't hide an override in a log from weeks ago.
- **Validation runs on the final environment**: the Phase 2 check (each service's own loader) runs after the overrides are applied, so a bad override fails with the service's message.
- **`npm run doctor`** lists every active override under its own heading, plus any bare generic variable that was dropped.
- The Linux systemd units ReforgedHost writes (`.workspace/config/systemd`) carry over no environment beyond what their unit sets, so overrides there go in a drop-in (`Environment=`). The docs need to say so.
- Tests: override wins; generic name dropped and prefixed name applied; secret redacted in the warning; warning printed again on restart; a key the service doesn't read stays untouched.

## Phase 3: the other files hosts write

- **news.json → news.toml**: `[[items]]` with `date`, `title`, and a `body` that can be a multi-line `"""` string, which is the real win for news. ReforgedContent reads `.toml` when the file ends in `.toml` and JSON otherwise. Setup converts an existing news.json once.
- **session-supplies.json → session-supplies.toml**: the backend patch reads JSON, so ReforgedHost converts TOML → JSON at setup into `.workspace/generated/session-supplies.json` and points `REFORGED_SESSION_SUPPLIES_FILE` there. The upstream patch doesn't change. Check the file's real shape before committing to a layout.

## Staying as they are

- `secrets.env`: generated once, never hand-edited, and already in the format the services read. Converting it buys nothing and risks rotating keys if the migration goes wrong.
- The rendered `*.env` files: build output.
- Launcher `settings.json`: app-owned state (maps keyed by server ID, trusted runtime sets). Revisit when the Rust/Tauri launcher lands, since serde makes either format trivial, but keep JSON there for compatibility with existing installs.
- `allowlist-state.json`, `preset/**`, `upstream/*.json`, `brand/palette.json`, `package.json`/`tsconfig.json`, the Tauri permission files (already TOML).

## Order and size

1. Phase 1: one change; the migration and the launcher reader are most of the work. Medium.
2. Phase 2: gateway tables first (the biggest set and the most likely to need tuning on a VPS), then backend and director constants, then the passthrough with its denylist and the doctor check. Medium.
3. Phase 3: two small, independent changes.

## Dauntless Revived (separate repository)

This repository has the same shape without the single source: each service has its own hand-edited `.env` (`UndauntedMetagame` reads ~69 variables, `UndauntedDeployServer` ~35, `UndauntedGateway` ~34, `UndauntedContent` ~13, plus `tools/dashboard.env`). systemd units load them with `node --env-file` (`deploy/linux-server`). That makes the win larger than here: one commented `server.toml` instead of five env files.

Proposed: port the Reforged pattern. A `server.toml` with one table per service, a renderer in `deploy/unix-common` (and its Windows counterpart) that writes the per-service env files the units already load, and the same passthrough plus denylist. Do it after Reforged Phase 2 so the renderer and validation design are proven. It needs its own survey of which Metagame variables are tunables and which are wiring.

## Decisions

- 2026-10-24: snake_case keys in every TOML file (host.toml, news.toml, session-supplies.toml, and Dauntless Revived's server.toml).
- 2026-10-24: environment variables override the config, with a warning in the log for each override (see "Environment variables override the config").
