<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Catch-up list: host kit (RX2)

Every Node commit that changed Reforged's host kit (`ReforgedHost/`, now `legacy/ReforgedHost/`) after the delta
lists (RX1) were made, and whether the Rust host kit in `rust/services/host` on `plan` has it. RH5 and RH7 port what
is left before the host kit's cutover.

## Range

- **Start:** `77fb622` (2026-10-20), the commit RX1's delta lists were diffed from. The reasoning is in
  `catchup-gateway.md` under "Range"; it is the same for all three lists.
- **End:** today, 2026-10-22, because the cut-off date (RD16) is not set yet. A later run extends the list from here
  to the cut-off.
- **This repository:** `git log 77fb622..plan` on `ReforgedHost` and `legacy/ReforgedHost`, plus every local and
  `origin/` branch not merged into `plan` (none of those touch the folder).
- **Freeza's upstream:** every `upstream/*` branch fetched on 2026-10-22, minus commits already in this repository's
  history. Two commits touch `ReforgedHost/`; neither is on `upstream/main`.

## Summary

| Status | Commits |
| --- | --- |
| Ported | 1 |
| Needs porting | 1 |
| Not applicable | 3 |

A commit with parts in more than one status is counted by its part that needs the most work; each part is listed
below it.

## This repository

### `b16d5f5` Move the Node projects to legacy/

- **SHA:** `b16d5f5f353139ebdb8ea4f8a9a99678549232d2`
- **Date:** 2026-10-22 10:34:57 -0600
- **Author:** Silent Spud
- **Files:** every file under `ReforgedHost/`, renamed to `legacy/ReforgedHost/` with no content change (RR2).
- **Behavior:** none; a `git mv`. The paths it breaks are fixed in `9042873`.
- **Rust:** not applicable. A rename changes no behavior; the paths that follow from it are under `9042873`.

### `9042873` Fix the paths broken by the move to legacy/

- **SHA:** `9042873f9705decf81721adfdd438cee5fa2ffdf`
- **Date:** 2026-10-22 11:21:16 -0600
- **Author:** Silent Spud
- **Files:** `legacy/ReforgedHost/README.md`, `linux/Deploy-FromWindows.ps1`, `linux/setup-vps.sh`,
  `src/captures.ts`, `src/cli.ts`, `src/invite.ts`, `src/linux.ts`, `src/paths.ts`, `src/preset.ts`,
  `src/selfupdate.ts`, `src/services.ts`, `test/captures.test.ts`, `test/linux.test.ts`, `test/performance.test.ts`,
  `test/selfupdate.test.ts`, `test/upstream.test.ts` (all under `legacy/ReforgedHost/`)
- **Behavior:** the kit finds the checkout by `legacy/ReforgedHost` and runs the gateway and content server from
  `legacy/`; `update` reinstalls the kit's npm dependencies when `node_modules` is missing after the move; the Linux
  scripts fetch and run `setup-vps.sh` from `legacy/ReforgedHost/linux/`.
- **Rust:** ported, by part:
  - `paths.ts` (`hostKit`, `gatewayDir`, `contentDir`, the root search): ported in
    `rust/services/host/src/paths.rs:129-131` and `rust/services/host/src/paths.rs:178-186`.
  - `cli.ts` (reinstall when `node_modules` is missing), `selfupdate.ts` (`hostKitDeps` paths): not applicable. The
    Rust kit has no npm dependencies of its own, and `update` from a `git` fast-forward is replaced by the server
    release (RH5, RD8).
  - `linux/Deploy-FromWindows.ps1`, `linux/setup-vps.sh`: not applicable as a path change. RH5 puts these scripts on
    the Linux release, where their paths are the release's, not `legacy/`.
  - `captures.ts`, `invite.ts`, `linux.ts`, `preset.ts`, `services.ts`, README, tests: comments, strings and test
    paths only. The Rust comments already name `legacy/ReforgedHost/...` (for example
    `rust/services/host/src/preset.rs:153`).

### `0ba6016` RP4: the host kit builds Mystic Paradox from paradox/ and native/

- **SHA:** `0ba601609a9602940a0f9c6924de80ae67606e9d`
- **Date:** 2026-10-22 12:49:26 -0600
- **Author:** Silent Spud
- **Files:** `legacy/ReforgedHost/README.md`, `package.json`, `src/captures.ts`, `src/cli.ts`, `src/doctor.ts`,
  `src/gamedata.ts`, `src/hostconfig.ts`, `src/paradox.ts` (new), `src/paths.ts`, `src/preset.ts`,
  `src/selfupdate.ts`, `src/upstream.ts` (deleted), `test/captures.test.ts`, `test/paradox.test.ts` (new),
  `test/performance.test.ts` (all under `legacy/ReforgedHost/`). Its gateway part is in `catchup-gateway.md`.
- **Behavior:** nothing is downloaded from Mystic Paradox any more. Setup checks the checkout has `paradox/backend`,
  `paradox/director` and the three `native/` folders, and copies a host's SDK, builds, exports and game data from the
  old `.workspace/Mystic-Paradox` once. `npm run upstream` and `--force-upstream` are gone. `doctor` checks `paradox/`
  and `native/` instead of the patch marker. `update` says the runtime must be rebuilt when `native/paradox-runtime` or
  `native/runtime-loader` changed (it used to look at runtime patches and the pin). The store captures pin moves to
  `paradox/backend/store-captures.json` and no longer writes a `.git/info/exclude` entry.
- **Rust:** needs porting, by part:
  - `paths.ts` (the `paradox/` and `native/` layout, `oldUpstream`, `capturesPin`): ported in
    `rust/services/host/src/paths.rs:115-146` and `rust/services/host/src/commands/setup/paradox.rs:21-54`.
  - `paradox.ts` (`missingParadox`, `oldWorkspaceItems`, `migrateOldWorkspace`): ported in
    `rust/services/host/src/commands/setup/paradox.rs:79`, `:107` and `:184`, called from
    `rust/services/host/src/commands/setup.rs:210-231`.
  - `doctor.ts` (the `paradox` check replacing `upstream`; the `native\...` fix texts): ported in
    `rust/services/host/src/doctor.rs:549-558`, `:361`, `:727`, `:741` and `:755`.
  - `captures.ts` (pin in `paradox/backend`, no exclude entry): ported in `rust/services/host/src/captures.rs:8` and
    `rust/services/host/src/paths.rs:146`; the Rust module never had the exclude step.
  - `gamedata.ts`, `hostconfig.ts`, `preset.ts`: comments and strings; the Rust strings already match
    (`rust/services/host/src/preset.rs:164`, `:214`).
  - `cli.ts` and `upstream.ts` (the `upstream` command and `--force-upstream` removed): **needs porting.** The Rust
    kit still has `Command::Upstream` (`rust/services/host/src/cli.rs:46`, dispatched in
    `rust/services/host/src/commands/mod.rs:48` to the stub `commands/upstream.rs`), the `force_upstream` option
    (`rust/services/host/src/cli.rs:121-123`) and the stubbed `rust/services/host/src/upstream.rs`. RH5 removes
    them, as `rust/services/host/src/upstream.rs:3-4` already says.
  - `selfupdate.ts` (`runtimeAffected` over `native/paradox-runtime/` and `native/runtime-loader/`): **needs
    porting** in RH5's `update` (`rust/services/host/src/commands/update.rs:12`, a stub, and
    `rust/services/host/src/selfupdate.rs`). With the server release, the test becomes "did the release's runtime
    sources change since the installed one", and `update` tells the host to rebuild and publish the runtime.

## Freeza's upstream

Neither commit below is on `upstream/main`, which was still `77fb622` on 2026-10-22.

### `5e22258` Backend: Ramsgate chat, quick chat and emojis reach the City room (patch 0025)

- **SHA:** `5e22258fb4a65fb9d30b7397be7172ea68aa33c8`
- **Date:** 2026-10-19 01:00:55 +0000
- **Author:** Claude (on `upstream/claude/project-thread-pcean2`)
- **Files (host kit part):** `ReforgedHost/test/upstream.test.ts`
- **Behavior:** the patch count test expects 25 patches. The patch itself is a backend change.
- **Rust:** not applicable. It has the same patch id as `b1643fc` (upstream PR #43), which is already in this
  repository's history before `77fb622`; the branch is a leftover of that PR.

### `e2d2912` Preserve deployed individual cooldown persistence as upstream patch 0034

- **SHA:** `e2d2912c93e938de72f284a0bce7c5be088627d2`
- **Date:** 2026-10-21 10:22:12 -0300
- **Author:** Freeza (on `upstream/codex/server-fixes-2026-10-21`, not merged upstream)
- **Files:** `ReforgedHost/test/upstream.test.ts`, `upstream/README.md`, `upstream/mystic-paradox.json`,
  `upstream/patches/0034-individual-cooldown.patch`
- **Behavior (host kit part):** the patch count test expects one more patch. The rest adds backend patch 0034 and
  moves the Mystic Paradox pin.
- **Rust:** not applicable to the host kit. RP4 deleted `upstream/`, the patch count test and the Node
  `upstream.ts`; the Rust kit builds from `paradox/` and has no patch list. Patch 0034 and the pin change are backend
  changes for the backend and Director catch-up list (RX2, wave 7), if Freeza merges them before the cut-off.
