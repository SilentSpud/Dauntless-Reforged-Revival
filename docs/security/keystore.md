<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: launcher key store and legacy reader (RX3)

The checklist for the Tauri launcher's account key store, the reader for keys the Electron launcher (1.5.x) stored
with `safeStorage`, and the files around them: settings and the log (MIGRATION_PLAN.md RA8, RD7, RD10, RD22,
section 6). `SECURITY_REVIEW.md` indexes it.

How to use it: before the launcher's 2.0.0 cutover, copy this list into the cutover PR, check each item in the code
(not from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed.
An item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

An account key (`RRK_<48 hex>`, or any 8 to 128 plain characters a player pasted) is the player's password on one
server. Whoever has it plays as that player there. The launcher sends it in the `x-reforged-user-api-key` header and
on the game's command line (`-AUTH_PASSWORD=`), and to nothing else.

- **At rest, this launcher:** `userData/keys/<server id>.key2`. Windows: DPAPI for the current user
  (`CryptProtectData`). Linux: AES-256-GCM (`rr1` + nonce + ciphertext) with a random key the desktop's Secret Service
  keeps (gnome-keyring, or KWallet on Plasma 6), found by the attributes `application` =
  `io.github.freezamotoqueiro.dauntlessreforgedrevival` and `purpose` = `account-key-files`. Decrypted, a file is
  `RRK2\n<fingerprint or ->\n<key>`.
- **At rest, the Electron launcher:** `userData/keys/<server id>.key`, `safeStorage` (Windows: `v10` + nonce +
  AES-256-GCM with the key in `userData/Local State`, itself DPAPI-wrapped; Linux, never shipped: `v10`/`v11`
  AES-128-CBC). Read once, written again as `.key2`, read back, and left in place for a rollback to 1.5.x until
  3.0.0 deletes it (RD7, RA16).
- **Plain copies:** `Documents/Dauntless Reforged Revival/Account Keys/<server id>.txt` (the same `RRK2` text, mode
  0600 on Linux), kept so a player finds the key after reinstalling the system, as the Electron launcher does; and
  the backup file the player chooses to save.
- **Trust:** anything running as the player can read the plain copies and rewrite every file here. The store
  protects keys from other users and other PCs, from a copied profile, and from the launcher's own mistakes
  (sending a key to the wrong server, logging it, handing it to the page). It does not protect against malware
  running as the player.

Code: `rust/launcher/src-tauri/src/platform/keystore/` (`mod.rs`, `format.rs`, `legacy.rs`, `os.rs`),
`platform/settings.rs`, `platform/log.rs`, `platform/paths.rs`.

Fuzz target: `launcher_files` in `rust/fuzz`: `settings.json` (what is written reads back as itself, a 1.4.4 server
is never current, a public server always has a valid fingerprint, stored usernames pass the registration rule),
decrypted key text (`parse_stored`, which round-trips) and the Electron framing (`Local State`, the `DPAPI` tag, the
`v10`/`v11` blobs). The AES, PBKDF2 and DPAPI calls behind the framing are RustCrypto's and Windows', not fuzzed
here; what reaches them is length-checked first.

Tests: `platform::keystore`, `platform::settings`, `platform::log` (Reforged's `keystore.test.ts` cases, the shared
tables in `rust/launcher/test/cases` that `legacy/ReforgedLauncher/test/platform-tables.test.ts` checks against the
Node code), and `an_electron_profile_reads_back` with the harness in `rust/launcher/test/electron-keystore`.

## Checklist

### One key per server, bound to its certificate

- [ ] The file name is `server_id`: the first 24 hex of SHA-256 of `host:port` (private) or `public:host:port`
      (public), as `serverId` in `keystore.ts`, so a public and a private server on one address never share a key
      (`server_ids_are_the_typescript_ones` holds the Node values).
- [ ] A public server's key loads only when the slot's fingerprint equals the stored one; a slot without one, or a
      key stored without one (a bare key, `-`), loads nothing (`load`).
- [ ] A public key is never saved without a valid fingerprint (64 lower-case hex); `rebind` is the only way a key
      moves to another certificate, and only the controller calls it after the player confirmed.
- [ ] `save` refuses anything `is_plausible_account_key` refuses, so no key can carry a space, quote or line break
      onto the command line.

### Reading the Electron launcher's keys (`legacy.rs`)

- [ ] Only `RRK2` text and bare keys are read (RD22). `DRK2` or any other tag reads as no key, and nothing is
      written for it (`only_rrk2_and_bare_electron_keys_are_read`).
- [ ] A file that does not decrypt (another user, another PC, damaged) is no key, never an error that stops the
      launcher; the import flow takes over.
- [ ] The `Local State` key is used only after DPAPI unwraps it for this user, and must be 32 bytes. The blob must be
      `v10` + 12 + at least 16 bytes before AES-GCM sees it; the tag is checked.
- [ ] A key read this way is written as `.key2`, decrypted again and compared before it counts as moved; if that
      fails the new file is deleted and the Electron file kept. The Electron file is never changed or deleted
      here, only by `remove` (the player forgot the server) or by 3.0.0.
- [ ] On Linux the Electron password is looked up by its Secret Service attributes, never by the shared label
      "Chromium Safe Storage".
- [ ] Shown with Electron's own code: the harness profile reads back key for key (`an_electron_profile_reads_back`),
      and the maintainer's check on a real 1.5.1 profile (the harness README) passed before the cutover.

### The key never leaves where it belongs

- [ ] No bridge type holds a key: `bridge/types.rs` has none, and the page learns only `hasKey`. The backup file is
      written by the backend after the save dialog; its text never goes to the page. A key the player pastes goes
      page to backend only (`register`/import), never back.
- [ ] Every key read or saved is registered with `log::add_secret` (with and without `RRK_`) before anything can log
      it, including a key moved from the Electron files.
- [ ] Every log line passes `redact`: registered secrets, `RRK_...`, `-AUTH_PASSWORD=...` and the
      `x-reforged-user-api-key` header are hidden, also in library (`tracing`) events and panics
      (`a_registered_key_never_reaches_the_file`, the shared table).
- [ ] Error values from the store (`KeyStoreError`) carry no key and no path.

### Files and folders

- [ ] `userData` is the Electron launcher's folder (`%APPDATA%\Dauntless Reforged Revival Launcher`,
      `~/.config/Dauntless Reforged Revival Launcher`), so an upgrade reads the old profile; `REFORGED_DEV_PROFILE`
      is honoured in debug builds only.
- [ ] Key files are written to a `.tmp` file and renamed; a crash leaves the old file or the new one.
- [ ] The Documents copy is written 0600 on Linux. Writing it may fail without losing the key (a warning, no key in
      it).

### Settings

- [ ] Every value in `settings.json` is checked when read; a wrong one falls back to its default. The current server
      is always one that could be reached safely: private only into the tailnet or loopback, public only with a valid
      fingerprint.
- [ ] A saved 1.4.4 server (`game: "revived"`) stays listed with its key on disk but is never the current server, so
      no key or launch goes to it (RD5, RD22). `revivedRoot` and `revivedVerified` are read and not written again.
- [ ] Stored usernames pass the registration rule (3 to 16 of `A-Z a-z 0-9 _`); they are shown, never used to
      authenticate.
- [ ] Folder settings are absolute, under their length limit and without NUL; `dumperDll` names a `.dll`.

### Before the cutover

- [ ] `launcher_files` ran for 24 hours with no finding (RD10).
- [ ] The maintainer's real-profile check passed on a 1.5.1 install, and a 1.5.1 profile with a 1.4.4 server loads.

Reviewed commit: ________ Date: ________ Signed: ________
