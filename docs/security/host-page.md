<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: the launcher's Host page (RX3)

The checklist for the Host page of the Tauri launcher: how it runs the host kit and handles the server's secrets,
and how it has the game runtime's own loader load Dumper-7 or CatalogExporter through `mystic_loader.ini`, with the
check before every PLAY that nothing is left behind (MIGRATION_PLAN.md RA12, RD9, RD23, RX3). `SECURITY_REVIEW.md`
indexes it.

How to use it: before the launcher's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The Host page lets a host run a server from their PC. It drives `reforged-host` from the host's project folder
(the server release, RD3, RD8), builds the runtime with the host's own toolchain, and makes game data from the
1.14.7 client with Dumper-7 (an SDK) and CatalogExporter (an export). Both DLLs are loaded by the runtime loader
(`winmm.dll`, `native/runtime-loader`), which the game loads at start and which loads every DLL its
`mystic_loader.ini` lists. So whatever DLL is beside the loader and on that list runs inside the player's game, with
the player's account key in reach: the list and the folder it points into are the trust boundary.

- **In from the host:** the project folder (a folder dialog), the Dumper-7 DLL (a file dialog), a game data pack
  (a file dialog), the setup form, who an invite is for, the multipliers. The page's values are checked in
  `bridge/commands.rs` before the controller sees them.
- **In from the host kit:** `reforged-host`'s output (lines, `Error:` lines, `--json` lines), `host.toml` (or a
  Node kit's `host.json`), and
  `vswhere`'s answers. All of it is parsed by `platform/hostkit/parse.rs`, which touches no file or process.
- **In from disk:** the session record `host-loader-session.json` in the launcher's data folder, and whatever is in
  the game's `Win64` folder.
- **Out:** `reforged-host` and the build tools started as child processes (never through a shell); in a session,
  one DLL copy and `mystic_loader.ini` in the game's `Win64` folder, and for Dumper-7 the runtime DLLs renamed
  aside; `export_flags.txt` there for CatalogExporter.

Code: `rust/launcher/src-tauri/src/controller/host.rs`, `platform/loader_session.rs`, `platform/hostkit.rs` and
`hostkit/parse.rs`, `platform/hostserver.rs`, `platform/requirements.rs`; the pre-launch check and the session's end
in `controller/mod.rs` (`play_now`, `game_changed`).

Fuzz target: `launcher_host` in `rust/fuzz` (`hostkit/parse.rs`: the host kit's output, the invite and update JSON,
`host.toml` and `host.json`, vswhere). It also checks that an invite that parses is a short `reforged-revival://` link on one line,
and that multipliers that parse are finite, from 1 to 100, and private only.

## Checklist

### No injection

- [ ] Nothing in the launcher opens another process to write into it or start a thread in it: no `OpenProcess`,
      `VirtualAllocEx`, `WriteProcessMemory`, `CreateRemoteThread`, `NtCreateThreadEx` or `SetWindowsHookEx`
      (`git grep` over `rust/launcher`). RA2's `platform/injector.rs` is gone and the `Platform` trait has no
      `inject`.
- [ ] The `windows` crate features are only the clipboard's and the key store's (`Cargo.toml`: `Foundation`,
      `Security_Cryptography`, `System_DataExchange`, and `System_Memory` for the clipboard's global memory); no
      `Win32_System_Threading` or `Win32_System_Diagnostics_Debug`.
- [ ] The loader is unchanged (`native/runtime-loader`): the launcher only places files it reads.

### The loader's list (`mystic_loader.ini`)

- [ ] A session begins only when the folder is clean: `begin` refuses with the leftovers listed while an ini, a DLL
      a leftover ini names, a DLL an unfinished session placed, a set-aside runtime, or a DLL with the host's tool
      names is beside the loader (`a_session_refuses_leftovers_clashes_and_own_names_and_needs_the_loader`).
- [ ] The picked DLL is copied under its own file name only; a name that is not `*.dll`, or is `winmm.dll`,
      `MysticParadox.dll` or `MystPaxInternalServer.dll` in any case, is refused; a file of the same name already
      there is never overwritten (and so never removed later).
- [ ] The ini names exactly that one file, by plain name (no path), so the loader resolves it beside itself
      (`a_dump_session_places_the_dll_and_ini_without_the_runtime_and_removes_them`).
- [ ] A Dumper-7 session sets `MysticParadox.dll` and `MystPaxInternalServer.dll` aside (`*.rr-dump-off`) before
      the game starts, so the loader loads Dumper-7 alone; the game starts without `REFORGED_*` and
      `MYSTICPARADOX_*` variables and without the relay (`a_dumper_7_session_loads_through_the_loader_list_and_cleans_up`).
- [ ] A CatalogExporter session starts the game exactly as PLAY does (the hash check, the runtime check, the relay
      and its owner check) and only then adds the exporter to the list, after the runtime
      (`a_catalog_exporter_session_plays_with_the_exporter_on_the_loader_list`).
- [ ] The record is written before any file is copied or renamed; a failed step undoes what was done
      (`a_failed_copy_undoes_the_session`).
- [ ] When the game the launcher started exits, the session ends: the ini and the DLL are removed, the runtime is
      renamed back (or the set-aside copy dropped when a newer runtime was installed meanwhile), then the record is
      removed. A file still held is retried, and a failure is logged and leaves the record.
- [ ] At the next start, a record left by a launcher that closed mid-session is undone before the runtime is looked
      at (`a_session_the_launcher_did_not_finish_is_undone_from_its_record`). A record whose DLL is not a plain file
      name is ignored, so a tampered record cannot delete a file elsewhere (`a_record_naming_a_path_is_ignored`).
- [ ] Sessions are Windows only until RA12a decides Linux (RD4); the loader works the same under Proton.

### The check before every PLAY

- [ ] `play_now` lists the leftovers before it changes anything in the game folder (before the local runtime is
      copied) and refuses with `loader_leftovers`, the files in the detail, and starts neither the relay nor the
      game (`play_refuses_a_leftover_ini_or_host_tools_dll_beside_the_loader`).
- [ ] The ini is refused whatever it lists, even empty; the DLLs it names are listed when present; the host's
      Dumper-7 (by the name in the settings) and `CatalogExporter.dll` are refused even without an ini; the
      game's own DLLs (any other name) are not touched or reported.
- [ ] The loader's and the runtime's own names are never reported as leftovers.
- [ ] The CatalogExporter session uses the same check first, so an export never rides along with a leftover.

### Secrets

- [ ] The invite link (`invite --json`) is a secret: its JSON line goes neither to the page's output lines nor to
      `launcher-invite.log`; it reaches only the `host-server` view, which the page blurs in streamer mode
      (`the_host_page_drives_reforged_host_setup_server_invite_and_stop`). The controller never logs it.
- [ ] Only a `reforged-revival://` link of at most 4096 characters with no control characters is taken from the
      host kit's output (`invite_line`).
- [ ] The launcher never reads `secrets.env` or the env files; `host.toml` (or `host.json`) is read for the name,
      mode, public host and multipliers only, and a file over 1 MiB is not read.
- [ ] The host kit gets the launcher's environment plus `NO_COLOR`, `FORCE_COLOR` and, for `start`,
      `REFORGED_CONTROL=stdin`; no key or invite goes into its arguments or environment.
- [ ] Job messages and errors carry the host kit's last `Error:` line (at most 300 characters), never a JSON line.

### Running the host kit and the build tools

- [ ] `reforged-host` is only the one in the chosen project folder (beside it, in `bin\`, or a build in
      `rust\target\release\`), started directly with an argument list, never through a shell or `npm`/`node`
      (RD23). A folder without it and `paradox\` is not a workspace.
- [ ] The host's text (server name, invite note) is trimmed, length-capped and free of control characters, and goes
      as one argument; the public host matches the DNS-name pattern; multipliers are finite, 1 to 100, rounded to
      two places (`bridge/commands.rs`).
- [ ] `start` keeps RH2's contract: a `stop` line on stdin, its stdin closing stops it too, and after 45 s its
      process tree is ended. A server answering on port 61000 that the launcher did not start is `external` and
      never stopped, set up, updated or given new multipliers from here.
- [ ] MSBuild comes from `vswhere` under `ProgramFiles(x86)` and cargo from `%USERPROFILE%\.cargo\bin` or
      `%CARGO_HOME%\bin`: the host's own toolchain, run on the host's own project, with output to the workspace's
      logs. (Whoever controls those variables already runs code as the host.)
- [ ] The Dumper-7 DLL must exist and end in `.dll`; the SDK import copies only files and folders (links are
      skipped) from a folder with `SDK.hpp`, into the runtime source.

### Fuzzing and tests

- [ ] `launcher_host` ran for 24 hours with no crash before the cutover (RD10); the short CI run passes on every PR.
- [ ] Reforged's `hostkit.test.ts` and `hostserver.test.ts` cases pass as Rust tests (`platform/hostkit.rs`,
      `platform/hostserver.rs`, `platform/requirements.rs`), with the session and pre-launch tests in
      `platform/loader_session.rs` and `controller/tests.rs`.
- [ ] The maintainer loaded CatalogExporter into a real 1.14.7 client through the loader and got `Items_Analysis`
      (RA12's last check).

Reviewed commit: ________ Date: ________ Signed: ________
