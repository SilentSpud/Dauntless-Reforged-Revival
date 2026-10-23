<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: the host kit's game-data commands (RX3)

The checklist for `reforged-host gamedata`, `preset` and `publish-runtime` in `rust/services/host` (MIGRATION_PLAN.md
RH4, RD3, RD23). `SECURITY_REVIEW.md` indexes it.

How to use it: before the host kit's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An item
that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The commands run as the host, on the host's own machine, and write only into the checkout and `.workspace/`. Nothing
here listens on a network. The input that is not the host's own:

- **Game data packs** (`gamedata --import <file>`, and the launcher's Host page): gzip-compressed JSON a host may
  have been sent by another host.
- **The store captures**: files fetched from a third-party GitHub repository at the commit
  `paradox/backend/store-captures.json` pins (git, shallow, into `.workspace/captures/`).
- **The CatalogExporter export** (`paradox/Items_Analysis/`): JSON lines written by a DLL injected into the host's
  game client.
- **The preset** (`preset/`): committed with the project; a pull request can change it.

Out: the backend's and the Director's game-data files, `preset/`, `.workspace/runtime/` (the files every player
downloads and runs) and the runtime DLLs beside the game servers' executable.

Code: `rust/services/host/src/` (`gamedata.rs`, `gamepack.rs`, `preset.rs`, `progression.rs`, `captures.rs`,
`publish.rs`, `commands/{gamedata,preset,publish_runtime}.rs`). Fuzz target: `rust/fuzz/fuzz_targets/host_gamedata_input.rs`.

## Checklist

### Game data packs

- [ ] A pack can only write the 16 known game-data files (`pack_entries`): any other `path`, `..` included, refuses
  the whole pack before anything is written (`read_game_data_pack`).
- [ ] Every file's `data` matches its `sha256` and parses as JSON, and the pack names this game build, or nothing is
  written.
- [ ] A file that is there and different is moved to `<name>.<time>.bak`, never overwritten in place.
- [ ] A pack is decompressed whole into memory with no size limit, as the Node kit does. It is the host's own file;
  a pack from someone else should be treated as any other download. Accepted risk, or add a limit before the cutover.
- [ ] `--export` leaves out the shipped examples and writes only where the host asked (or
  `.workspace/gamedata-packs/`).

### Store captures

- [ ] The pin's repository is `https://`, `file://` or an absolute local path, the commit a full 40-hex id, and each
  file a plain relative path with a 64-hex SHA-256 (`parse_capture_pin`). On Windows a drive (`C:x`) or a backslash
  is refused too, so a name can never leave `paradox/DauntlessEndpointDocumentation/` (stricter than the Node kit).
- [ ] git runs with an argument list, never a shell; the commit and repository come from the pin only.
- [ ] Each file is read from git's objects (`git show <commit>:<path>`) and checked against its pinned hash before
  any file is written; one mismatch writes nothing.
- [ ] The captures are never copied into the repository (they carry no license); they are git-ignored.

### Generators and the export

- [ ] Until RB11 the generators are the Node scripts of `paradox/backend` and `paradox/director`, started through
  `gamedata::run_generator` with fixed arguments; nothing from the export reaches a command line.
- [ ] The progression export is parsed line by line; a line that is not a JSON object is skipped; numbers outside
  JavaScript's safe integers fall back as the Node kit's do. The output is the Node kit's byte for byte
  (`tests/gamedata.rs`).
- [ ] A progression source the host supplied (no `generated_by` of the kit) is never overwritten.

### Preset

- [ ] `preset --apply` (and setup) checks every file against `preset.json` before writing anything, refuses unknown
  paths and the two stores built from the captures, and never replaces a host's own data without `--force` (which
  keeps a `.bak`).
- [ ] `preset --create` writes only into `preset/` and removes only files there that the new manifest does not hold
  (the README stays).

### Runtime publishing

- [ ] `publish-runtime` copies exactly `MysticParadox.dll` and `winmm.dll`, from the checkout's builds or the
  `--from` folder the host names, and writes `runtime.json` last (after a `.tmp` and a rename), so the content server
  never lists a file that is not in place.
- [ ] `runtime.json`'s hashes are of the copied files, not the sources. Its format is the Node kit's, byte for byte
  (RD23: the Host page calls this command).
- [ ] A DLL a running game server holds is renamed aside (`.rr-old-<ms>`) and replaced; the aside files are swept on
  the next publish. A file that cannot be replaced is reported, not skipped silently.

## Sign-off

Reviewed at commit `________` on `____-__-__` by `________`.
