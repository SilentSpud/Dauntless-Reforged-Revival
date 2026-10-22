<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Catch-up list: gateway and allowlist helper (RX2)

Every Node commit that changed Reforged's gateway or allowlist helper (`ReforgedGateway/`, now
`legacy/ReforgedGateway/`) after the delta list (`docs/port/delta-servers.md`, RX1) was made, and whether the Rust
gateway in `rust/services/gateway` on `plan` has it. RS5 ports what is left before the front-door cutover.

## Range

- **Start:** `77fb622` (2026-10-20, "Merge pull request #51 from FreezaMotoqueiro/claude/busy-hopper-34m62z"). The
  RX1 commit `04aff48` sits on `plan`, whose history leaves `main` at `77fb622`; the commits between (`66dc82a` to
  `eabc94c`) touch only `MIGRATION_PLAN.md`, so the Node files RX1 diffed are those of `77fb622`. `main` and
  `upstream/main` were both still at `77fb622` when this list was made. This is also the point RX2's row calls "the
  copy commit": nothing in the Node sources changed between it and RR1.
- **End:** today, 2026-10-22, because the cut-off date (RD16) is not set yet. A later run extends the list from here
  to the cut-off.
- **This repository:** `git log 77fb622..plan` on `ReforgedGateway`, `legacy/ReforgedGateway` and the other front-door
  and host folders, plus every local and `origin/` branch not merged into `plan` (none of those touch these folders).
- **Freeza's upstream:** every `upstream/*` branch fetched on 2026-10-22, minus commits already in this repository's
  history. None of them touches `ReforgedGateway/`.

## Summary

| Status | Commits |
| --- | --- |
| Ported | 1 |
| Needs porting | 0 |
| Not applicable | 3 |

## This repository

### `b16d5f5` Move the Node projects to legacy/

- **SHA:** `b16d5f5f353139ebdb8ea4f8a9a99678549232d2`
- **Date:** 2026-10-22 10:34:57 -0600
- **Author:** Silent Spud
- **Files:** every file under `ReforgedGateway/`, renamed to `legacy/ReforgedGateway/` with no content change (RR2).
- **Behavior:** none; a `git mv`.
- **Rust:** not applicable. A rename changes no behavior, and the Rust gateway is its own crate.

### `9042873` Fix the paths broken by the move to legacy/

- **SHA:** `9042873f9705decf81721adfdd438cee5fa2ffdf`
- **Date:** 2026-10-22 11:21:16 -0600
- **Author:** Silent Spud
- **Files:** `legacy/ReforgedGateway/README.md`, `legacy/ReforgedGateway/test/helpers.ts`
- **Behavior:** the README names the new folder; the test helper reads patch 0002 one folder higher (`../../upstream`)
  and a comment names the launcher's new path. Tests and docs only.
- **Rust:** not applicable. The README is the Node project's; the test helper's path is replaced again by `0248ade`
  below, whose Rust form is ported.

### `0ba6016` RP4: the host kit builds Mystic Paradox from paradox/ and native/

- **SHA:** `0ba601609a9602940a0f9c6924de80ae67606e9d`
- **Date:** 2026-10-22 12:49:26 -0600
- **Author:** Silent Spud
- **Files (gateway part):** `legacy/ReforgedGateway/README.md`, `legacy/ReforgedGateway/test/helpers.ts`
- **Behavior:** comments only: patch 0002 is now described in `docs/paradox-patches.md` and lives in `paradox/backend`.
  The host kit part is in `catchup-host.md`.
- **Rust:** not applicable. No code changed in the gateway.

### `0248ade` RP4: gateway tests read the /reforged/api routes from paradox/backend

- **SHA:** `0248ade1b23d4af557db25f43468a0cf64cd88db`
- **Date:** 2026-10-22 13:01:50 -0600
- **Author:** Silent Spud
- **Files:** `legacy/ReforgedGateway/test/helpers.ts`, `legacy/ReforgedGateway/test/policy.test.ts`
- **Behavior:** the policy tests take the list of `/reforged/api` routes from
  `paradox/backend/src/routes/reforgedApi.ts` (the source) instead of the deleted `upstream/patches/0002-*.patch`
  (no leading `+` in the pattern any more).
- **Rust:** ported. `rust/services/gateway/src/policy.rs:403-409` (unit test) and
  `rust/services/gateway/tests/common/mod.rs:727-731` (integration helper) read the same file with the same pattern.

## Freeza's upstream

No commit on any `upstream/*` branch after `77fb622`, and not in this repository's history, touches
`ReforgedGateway/`.
