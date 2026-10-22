<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Catch-up list: content server (RX2)

Every Node commit that changed Reforged's content server (`ReforgedContent/`, now `legacy/ReforgedContent/`) after
the delta list (`docs/port/delta-servers.md`, RX1) was made, and whether the Rust content server in
`rust/services/content` on `plan` has it. RS5 ports what is left before the front-door cutover.

## Range

- **Start:** `77fb622` (2026-10-20), the commit RX1's delta list was diffed from. The reasoning is in
  `catchup-gateway.md` under "Range"; it is the same for all three lists.
- **End:** today, 2026-10-22, because the cut-off date (RD16) is not set yet. A later run extends the list from here
  to the cut-off.
- **This repository:** `git log 77fb622..plan` on `ReforgedContent` and `legacy/ReforgedContent`, plus every local and
  `origin/` branch not merged into `plan` (none of those touch the folder).
- **Freeza's upstream:** every `upstream/*` branch fetched on 2026-10-22, minus commits already in this repository's
  history. None of them touches `ReforgedContent/`.

## Summary

| Status | Commits |
| --- | --- |
| Ported | 0 |
| Needs porting | 0 |
| Not applicable | 2 |

No behavior of the Node content server changed in the range.

## This repository

### `b16d5f5` Move the Node projects to legacy/

- **SHA:** `b16d5f5f353139ebdb8ea4f8a9a99678549232d2`
- **Date:** 2026-10-22 10:34:57 -0600
- **Author:** Silent Spud
- **Files:** every file under `ReforgedContent/`, renamed to `legacy/ReforgedContent/` with no content change (RR2).
- **Behavior:** none; a `git mv`.
- **Rust:** not applicable. A rename changes no behavior, and the Rust content server is its own crate.

### `9042873` Fix the paths broken by the move to legacy/

- **SHA:** `9042873f9705decf81721adfdd438cee5fa2ffdf`
- **Date:** 2026-10-22 11:21:16 -0600
- **Author:** Silent Spud
- **Files:** `legacy/ReforgedContent/README.md`
- **Behavior:** the README names the new folder. Docs only.
- **Rust:** not applicable. The README is the Node project's.

## Freeza's upstream

No commit on any `upstream/*` branch after `77fb622`, and not in this repository's history, touches
`ReforgedContent/`.
