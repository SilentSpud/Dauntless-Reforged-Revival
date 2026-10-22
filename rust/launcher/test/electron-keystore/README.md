# Electron key store harness

Shows that the Tauri launcher reads the account keys the Electron launcher (1.5.x) stored with `safeStorage`
(MIGRATION_PLAN.md RA8, RD22). `harness.cjs` runs inside Electron 44 (the version
`legacy/ReforgedLauncher` ships), loads that launcher's own `keystore.ts` and `settings.ts`, and writes a
test profile into an empty folder: made-up keys for public, private, rebound, bare and 1.4.4 servers, the
Documents copies, `Local State`, and a `settings.json` whose current server is a 1.4.4 one. `expected.json`
lists every slot and the key it must read back as. The Rust test `an_electron_profile_reads_back`
(`src-tauri/src/platform/keystore/mod.rs`) then reads the profile through `legacy.rs`, checks the move to
the new files and the Documents copies, and loads the settings.

Run it as the same user on the same PC (DPAPI keys belong to both), on Windows, from this folder:

```bash
npm i --no-save --no-package-lock electron@44.4.3 typescript@6.0.3 @types/node@24
npx tsc -p .
npx electron . "$TEMP/rrk-profile"
REFORGED_ELECTRON_PROFILE="$TEMP/rrk-profile" cargo test -p launcher --lib an_electron_profile_reads_back -- --ignored
```

The last line runs from `rust/`. Never point the harness or the test at a real launcher profile: the harness
refuses a folder that is not empty, and the test moves keys into new files.

## A real 1.5.1 profile (the maintainer's check)

On a PC where Dauntless Reforged Revival Launcher 1.5.1 has joined a server, with the player's consent:

1. Quit the 1.5.1 launcher. Copy `%APPDATA%\Dauntless Reforged Revival Launcher` to a scratch folder as
   `<copy>\User Data`, and `Documents\Dauntless Reforged Revival` to `<copy>\Documents\Dauntless Reforged Revival`.
   Work on the copy only; the test writes `.key2` files next to the `.key` ones.
2. Write `<copy>\expected.json` by hand: `{ "platform": "win32", "slots": [...] }` with one entry per server in
   `settings.json` (`host`, `port`, `mode`, `fp`, `certificate` = the same `fp` for a public server or null,
   `documents`: true when its `<server id>.txt` is in the Documents copy, and `key`: the key from the
   player's backup file). Keep this file off any shared disk and delete it afterwards.
3. Run the `cargo test` line above with `REFORGED_ELECTRON_PROFILE=<copy>`.
4. Once the controller is ported (RA13): start the 2.0.0 build with `REFORGED_DEV_PROFILE=<copy>` (debug builds only) and check the account
   shows as registered and PLAY reaches the server; a 1.4.4 server, if any, is listed and disabled.
5. Delete `<copy>`.
