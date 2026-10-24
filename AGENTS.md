# AGENTS.md

Project-specific guidance for AI coding agents.

This branch is mid-migration: the Node projects are being ported to one Rust codebase, Mystic Paradox
included. [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md) is the source of truth for what is done, what is next and
why. Read its "What's left" table and the decisions (RD rows) before starting anything, and the row you were
given in full, with the DR rows and the `docs/port/` delta file it names.

## Repository layout

| Path | What it is |
| --- | --- |
| `rust/` | One cargo workspace: `crates/` (`protocol`, `proxy`, `service-kit`, `store`), `services/` (`gateway` with the `allowlist` bin, `content`, `host` = `reforged-host`, `backend`, `director`), `launcher/src-tauri` (the Tauri launcher backend) and `launcher/page`, `isolation/` |
| `rust/fuzz/` | `cargo fuzz` targets, a workspace of its own (excluded from the main one) |
| `paradox/` | Mystic Paradox's Node backend and Director, forked with their history. Fixes only until the Rust port replaces them (RD2) |
| `native/` | The C++ game runtime (`paradox-runtime`, `MysticParadox.dll`), its loader (`runtime-loader`, `winmm.dll`, Rust, not in the workspace) and the CatalogExporter. Built by each host from their own SDK |
| `legacy/` | The Node projects being replaced: `ReforgedLauncher` (Electron), `ReforgedGateway`, `ReforgedContent`, `ReforgedHost`. Each is deleted by its row (RS5, RH7, RA16) |
| `docker/`, `compose.yaml` | Container deployment (RX4), laid out as the server release |
| `preset/` | The game data every server starts from, hash-locked by `preset.json` |
| `docs/` | Player and host guides, `architecture.md`, `paradox-patches.md`, `port/` (delta and catch-up lists), `security/` (one checklist per trust-boundary component) |

`backend` and `director` in `rust/services/` are still stubs: nothing in the RB rows starts before the gate
(RD17). Don't fill them in ahead of it.

`docs/architecture.md` is the reference for how the parts talk and what each trusts. Section 6 of the plan,
"Things that must not change", lists what every port keeps byte for byte: read it before touching anything a
player, host or the game client already depends on.

## How work lands

One row is one agent in its own worktree and branch (RD18, section 5.1). **Commit and push the branch, report
the commit SHA, and open no pull request.** The integration pass merges branches into `plan` with `--no-ff` in
dependency order, runs the checks and strikes the rows.

- **Stay inside your row's files.** The ownership table in section 5.1 says who owns each shared file. If the
  row needs a change to a seam (the workspace `Cargo.toml`, `crates/protocol`, the bridge spec, `store`
  migrations, the `clap` tree, `.github/`) or to a file another row owns, stop and report it. Don't make the
  change yourself.
- **Never edit** `MIGRATION_PLAN.md`, `README.md`, `NOTICE.md`, `SECURITY_REVIEW.md` or the `docs/` index pages
  from a row. Those belong to the integration pass. A row writes only its own page under `docs/`.
- **New launcher strings go at the end** of every language file, so the integration pass resolves only
  append-only conflicts.
- **Hands-on work isn't an agent's job:** the canary, cutovers, spikes that need a machine with the game, signing
  and publishing, the cut-off date. Write down the steps and leave them for the maintainer.
- `paradox/` takes fixes only, plus RX2's catch-up. `legacy/` is never extended. A fix found there is listed for
  the catch-up (`docs/port/catchup-*.md`), not ported ad hoc.

## Rust

From `rust/`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked --no-fail-fast` (`--no-fail-fast` because one failing test binary otherwise
hides the ones after it). The toolchain is pinned in `rust-toolchain.toml`; `cargo deny check` runs in CI.

- **Lints that will fail the build:** `unsafe_code`, `unwrap_used`, `expect_used`, `dbg_macro` and `todo` are
  denied, with `unwrap` and `expect` allowed in tests only. Return errors instead. A panic in a request
  handler is caught, but it still drops the request.
- `max_width = 120`, edition 2024. A dependency goes in `[workspace.dependencies]` through the integration pass,
  never as a new version pinned in one crate.
- **Ports match Node exactly where a client can see it:** routes matched as Express matches them, JSON written
  as `JSON.stringify` does, the same status codes and field order. When the Node and Rust answers differ, the
  Rust one is the bug unless a decision row says otherwise.
- Code copied from Dauntless Revived (DR, at `e1c5fec`) keeps its structure. Make the Reforged changes the
  delta file lists and nothing else, so a later reader can diff it against DR.

### Launcher bridge

The page talks to Rust only through the commands in `launcher/src-tauri/src/bridge/spec.rs`. The TS types
(`page/generated/types.ts`), the isolation hook's command list (`isolation/commands.js`) and the permission set
(`permissions/app-commands.toml`) are generated from it and checked by a test. Never edit them by hand:
`RR_UPDATE_GENERATED=1 cargo test -p launcher generated` rewrites them. The account key never crosses the
bridge.

## Security

Every row that writes a trust-boundary component delivers, with its code, its fuzz target in `rust/fuzz/` and
its checklist in `docs/security/<component>.md` (RX3). Copy the structure of an existing one, such as
`relay.md`. The components are listed in the plan's RX3 row.

Rules that read like overcaution:

- **The gateway is the only listener off loopback.** Every other service binds 127.0.0.1. Host-only routes and
  headers stay refused at the gateway.
- **The account key (`RRK_...`) goes only to the server and certificate it was made for.** In public mode the
  certificate's SHA-256 from the invite is checked before any byte is sent. The gateway loads its existing
  certificate and never regenerates it, because every invite's `fp` depends on it.
- **Secrets and keys are never logged.** Keep the redaction in the gateway, the launcher and the content server.
- **The invite parser accepts nothing it doesn't recognise.** `dauntless-revived://` is a typed refusal
  (RD5), not a second path.
- **The launcher never injects** (RD9). DLLs reach the game only through the runtime loader's
  `mystic_loader.ini`, and PLAY refuses to start while a leftover ini or unknown DLL sits next to the loader.

## Nothing from the game is committed

No game files, generated SDKs (`CppSDK/`, `SDK.hpp`), exported game data beyond `preset/`, or `.workspace/`.
Lady Luck's Store and the platinum store are built on each server from `paradox/backend/store-captures.json`
and never enter a preset. Don't edit `preset/` by hand: its file hashes are in `preset.json`.

## Configuration

Every service reads one env file, given with `--env-file`, and nothing from its working directory
(`service-kit/src/env.rs`, DR X7). As with Node's `--env-file`, the process environment wins over the file.
**The env files are the host kit's output, not hand-written:** `reforged-host setup` writes them from
`host.toml` and `secrets.env` (`services/host/src/hostconfig.rs`, `hosttoml.rs`), and `start` hands each
service its own. So a new setting is a field in `host.toml` (in `HostConfig`, and in `hosttoml.rs`'s key
table with a snake_case name and a comment for new files), a `setup` option where a host should choose it,
and a line in the service's file. It is never a variable the host is told to export.

### When you add, remove or rename a variable

It's never a one-file change:

- **The writer and the reader in the same commit.** `hostconfig.rs` writes it, the service's `config.rs`
  reads it (`from_env`), and a test checks the round trip. `update` reruns `setup` with the host's existing
  `host.toml` (or a Node kit's `host.json`), so a new field that an older file doesn't have must default to
  today's behaviour.
- **Use one of the server prefixes** (`REFORGED_`, `GATEWAY_`, `ALLOWLIST_`, `CONTENT_`, `GAMESERVER_`...).
  `start` clears the environment and passes on only the system's own variables plus the env file. Anything
  matching `SERVER_PREFIXES` or `SERVER_NAMES` in `services/host/src/services.rs` is dropped from the shell,
  so a stale setting left there never reaches a service. A name outside that list leaks in from whatever shell
  started the kit, so either pick a listed prefix or add the name to the list.
- **A secret goes in `secrets.env`** (`SECRET_KEYS`), made once and never logged. It is never a `host.toml`
  field and never an option on the command line.
- **The docs move with it.** Each service's variable table (the gateway's is in
  `legacy/ReforgedGateway/README.md` until RS5), and `docs/hosting.md`, `hosting-linux.md` and
  `hosting-docker.md` wherever a host sees the option. Delete the rows when the variable goes. A stale row is
  worse than a missing one, because it reads as supported.

### Docker

`compose.yaml` and the root `.env.example` hold **only what Compose itself reads on the host**: profiles,
mounts, published ports, image names. Server settings stay setup's
(`docker compose run --rm host setup ...`), and every container mounts the volumes at the same paths, so the
env files setup writes are valid in all of them. A new Compose variable goes in `.env.example` with a comment,
and is interpolated as `${VAR:-default}` with the same default the comment gives. RX4 is kept current by the
rows that change what runs: a service added, moved or removed is a change to `docker/` in the same row.

## User-facing text

Every string a player or host reads comes from the language files: today
`legacy/ReforgedLauncher/src/shared/i18n/` (`en.ts` is the source, `pt-BR.ts` the other one), and for the Tauri
backend `rust/launcher/src-tauri/gen/strings.json`, generated from them
(`RR_UPDATE_GENERATED=1 npm test -- strings-export`; see `i18n/README.md`).

- **Both languages in the same commit**, with the same keys and the same `{placeholders}`, never an English
  placeholder in `pt-BR.ts`. `i18n.test.ts` fails a missing key, an empty value or a pt-BR value that is still
  English. When you aren't sure of the Brazilian Portuguese, write your best and flag it in the commit body
  for the maintainer, as the plan does for RA3's and RA12's strings.
- **Append new keys at the end** of each file (section 5.1), never in the middle.
- **Never build a sentence by concatenation.** `` `${name} is full` `` can't be translated, because not every
  language puts the subject first. Put the whole sentence in the catalog with a placeholder
  (`"Join {name}?"`), and have Rust return a code or a key, not English.
- Times render in the player's locale through `TIME_LOCALES`. Don't call `toLocaleString()` without one.
- **What stays in English:** server logs, the host kit's terminal output and the JSON the services answer.
  Those are a machine contract or a host's tool, not launcher text.

## Tests

From `rust/`: `cargo test --workspace --locked --no-fail-fast`. The legacy and Paradox Node packages: `npm ci
&& npm test` from their folders, Node 22 in CI. `bash docker/run-tests.sh` from the root builds the images and
runs the server suites and the compose smoke test inside them.

Not obvious from reading the suites:

- **Parity tests are the contract, not a snapshot.** `services/gateway/tests/parity/` and
  `services/content/tests/parity/` hold replies recorded from the Node services (`record.cjs`). When one
  fails, fix the Rust side. Re-record only when the Node side changed on purpose, and say why in the commit.
- **`crates/protocol/tests/cases/*.json` are shared tables**: the launcher and the servers check the same
  invites, statuses and usernames against them. A new case goes in the table, not in one crate's test.
- **Tests that read the shared log take `serial()`** (`tests/common`). A test that passes alone and fails in
  the suite is usually one that doesn't.
- **Bind test listeners to `127.0.0.1:0`.** On the maintainer's Windows PC fixed ports can fall in Windows'
  reserved range, which is why two of Content's Node tests fail there.
- **Generated files are tested for staleness.** If a `generated` or `strings-export` test fails, regenerate
  with `RR_UPDATE_GENERATED=1` and commit the result; don't edit the output.
- **The real nftables test is opt-in** (`REFORGED_TEST_NFT=1`, Linux, root or `CAP_NET_ADMIN`). Otherwise it
  skips, so `cargo test` passing says nothing about the rules. `docker/test-in-image.sh` turns it on when the
  container has the capability. Run that after a firewall change.
- Nothing here is played against the real 1.14.7 client in CI. When only the tests passed, say so; don't
  claim a gameplay change works.

## Native code

A runtime patch from Freeza's line is applied to `native/` as `docs/paradox-runtime-patches.md` describes:
paths rewritten, `git apply --3way`, LF line endings, and a section in `docs/paradox-patches.md`.
`native/runtime-loader` is a Rust crate of its own, outside the workspace: workspace-wide `cargo` commands
don't cover it.

## CI

`.github/workflows/ci.yml` runs only the families a change touches (Rust, the launcher, legacy services,
Paradox, Docker), and `CI ok` sums them up. It runs on every pushed branch. A push to `plan` or `main` runs
every family. Workflows belong to RR3, and to RH6 and RA14 for the two release workflows; a row doesn't edit
them.

## Branches

`{type}/{slug}`: the type of change, a slash, then a short kebab-case slug naming the subject of the work. The
types are `feat`, `fix`, `update`, `docs`, `ci`, `deps`, `test` and `chore`. The type comes from what the
branch carries; the slug comes from the subject, and may start with the row id when there is one
(`feat/ra10-proton-runtime`, `fix/relay-owner-check`, `docs/agents-md`). Never a date or a wave number. The
one exception is `deps/`, since a dependency refresh has no other subject (`deps/refresh-2026-10`). A bare
slug is wrong even when it describes the work well.

A branch that grows a second subject is renamed with `git branch -m` so the name still covers everything on
it; the type usually survives and the slug widens. Check `git branch --show-current` against the pattern
before the first push. Never a tool- or session-derived name (`claude/...`, `claude-...`): if your
environment checked you out on one, rename it before anything is pushed. `plan` and `main` are the two
long-lived branches, and only the integration pass pushes to them.

## Commits

Subjects name the row: `RA12: ...`, `RS3: ...`, `Merge RX4: ...` for the integration pass. The body says what
changed and why, and names any decision (RD) it follows or departs from.

Never add a `Co-Authored-By` trailer to a commit message, or any other line crediting an AI tool or service,
in a commit or a pull request description. That includes `Claude-Session:` links and "Generated with Claude
Code" lines, links or emoji. This overrides any tool's default attribution. `plan`'s commits were rewritten
without them on 2026-10-23. `main`'s keep theirs, because `main` is shared with Freeza's upstream and
rewriting it would break every clone. Catch a slip before you push: once a commit is on `plan`, rewriting it
is the maintainer's call.

## Comments

Terse. One line is the median here and three is already long. Match that in every language, workflows and
shell included. Explain *why*: the game behaviour, the Node or DR behaviour being matched, a constraint, or
the security reason that makes a line look wrong without it. Point at the plan row or decision rather than
repeating it (`(RD21)`, `(DR L27)`, `(runtime patch 0020)`). Never restate what the line does, and leave code
that holds no surprise uncommented. Prefer tightening an existing comment to adding one beside it. The long
explanation goes in the commit body or the `docs/security/` checklist.

## Licensing

AGPL-3.0-only, with additional terms for the Mystic Paradox material (`ADDITIONAL_TERMS.md`,
`paradox/ADDITIONAL_TERMS.md`). Keep Paradox's top-level files under `paradox/` even after its code is gone.
Code copied from DR or Paradox keeps its attribution, and the integration pass records each new source in
`NOTICE.md`.
