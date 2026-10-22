<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: runtime download (RX3)

The checklist for the launcher's handling of a server's game runtime: the manifest, the player's acceptance, the
download, the check before every launch and the 1.14.7 executable hash (MIGRATION_PLAN.md RA7, RD10, RD22, section 6).
`SECURITY_REVIEW.md` indexes it.

How to use it: before the launcher's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

Each server's host builds its own runtime, `MysticParadox.dll` and the `winmm.dll` loader, from their own SDK. The
launcher puts both next to the game's executable, and the game loads them at start: whatever lands there runs as
the player, with the player's files and the account key's server in reach. So the content server is trusted only
as far as the player said so, and only for the exact bytes the player accepted.

- **In from the server:** `GET /content/v1/runtime/manifest` (no key header; 10 s, at most 64 KiB), then
  `GET /content/v1/runtime/files/<name>` for each file, with the account key's headers. Both go only to the
  invite's host, over TLS pinned to the invite's certificate in public mode, or plain HTTP into the tailnet or to
  loopback in private mode, with no redirects (`net/http.rs`, `net/http/to_file.rs`).
- **In from the player:** the acceptance of one set (`trustedRuntimes[serverId]`, a list of set ids), and in
  Settings > Advanced a local runtime folder a host built themselves.
- **In from disk:** the files next to the game (hashed again before every launch), the marker
  `.reforged-runtime.json` (written, never read), and `file-hashes.json` in the launcher's data folder (read back
  at start).
- **Out:** two files written into the game's `Win64` folder, under exactly the two runtime names.

Code: `rust/launcher/src-tauri/src/net/runtime_files.rs` and `runtime_files/manifest.rs`, `net/http/to_file.rs`,
`net/verify.rs` and `verify/cache_file.rs`. The controller that calls them is RA13's (`controller/runtime.rs`,
`controller/game.rs`).

Fuzz target: `runtime_input` in `rust/fuzz` (the runtime manifest and the hash cache file). It also checks that a
manifest that parses names only the two runtime files, each once, with a plain hash and a size within the cap.

## Checklist

### The manifest

- [ ] `parse_runtime_manifest` rejects the whole manifest unless `files` lists exactly `MysticParadox.dll` and
      `winmm.dll`, each once. No other name, no path, no `..`, no case variant is ever accepted, so nothing else is
      written next to the game.
- [ ] Each `sha256` is 64 lower-case hex characters, each `size` an integer from 1 byte to 64 MiB
      (`MAX_RUNTIME_FILE_BYTES`); a float with a fraction, a negative, a string or `1e12` rejects the manifest.
- [ ] `version` is cleaned text (control and bidirectional characters removed) of at most 48 characters; an empty
      one rejects the manifest. `build.name` is always `rel-1.14.7-Archon`, whatever the server says.
- [ ] The manifest request carries no account key, is capped at 64 KiB and 10 s, and a pin failure is reported as
      `cert_mismatch`, not as "no runtime".

### Acceptance

- [ ] The set id is SHA-256 of `name:size:sha256` lines sorted by name, joined by `\n`, byte for byte Reforged's
      `runtimeSetId`, so a set accepted in 1.x is still accepted and no other set is (`runtime_set_id_is_reforgeds_byte_for_byte`).
- [ ] Acceptance is per server (`trustedRuntimes[serverId]`) and per exact set: any change to a name, size or hash
      makes a new id, and the player is asked again (`a_set_is_accepted_once_per_server_and_a_changed_set_is_not`).
- [ ] Nothing is downloaded for a set that is not accepted: the controller refuses `install_runtime` with
      `runtime_untrusted` (RA13; checked there).
- [ ] An installed runtime that matches the offered set exactly is `ok` without asking (the bytes are what the
      player would accept anyway); one that does not match is never playable until accepted and installed.

### The download

- [ ] `request_to_file` keeps `request()`'s rules: `check_endpoint` first (a valid host, plain HTTP only to the
      tailnet or loopback, a well-formed pin), the `host` header, no redirects followed (a 3xx is just a status).
- [ ] A status other than 200 writes nothing. A `content-length` over the manifest's size fails before the file is
      opened; a body that grows past it fails while streaming (`too_large`, reported as `different`).
- [ ] The idle timeout (30 s) covers the connection, the answer and every chunk, so a stalled server cannot hold an
      install open for ever. A cancel stops it at once.
- [ ] Each file is written to `<name>.rr-download` next to its target, never to the target. Any failure removes the
      partial file (`a_partial_download_leaves_the_old_runtime_in_place`).
- [ ] Size and SHA-256 are checked against the accepted manifest before the file replaces the old one; a mismatch
      leaves the old file untouched (`a_file_that_does_not_match_the_list_never_replaces_the_old_one`).
- [ ] After the replace the file is hashed again (an antivirus or another program may change it), and an install
      that fails part way leaves a mix that `check_runtime` reports as `outdated` against the offered manifest.
      Known gap, as in Reforged: if the server later offers no manifest at all, any two non-empty files pass (one
      build serves every server), so such a mix would be playable.
- [ ] The account key's headers are the host API's `keyHeaders` (sent only over pinned TLS or into the tailnet);
      the key is never logged. A 401 or 403 is `key_rejected`.

### Replacing a file in use

- [ ] `replace_file` renames over the target; if a running process holds the old file, the old one is moved to
      `<name>.rr-old-<ms>` first and put back if the new one cannot take its place, so the game always has a
      runtime.
- [ ] `sweep_aside` deletes only `MysticParadox.dll.rr-old-*` and `winmm.dll.rr-old-*`, nothing else in the folder.

### The check before every launch

- [ ] PLAY runs `check_runtime` with the hashes read afresh from disk (no cache): a file changed after the install
      makes the state `outdated` and the launch is refused (`end_to_end_accept_install_check_tamper_and_a_changed_set`).
- [ ] A local runtime folder is used only when both files are there and non-empty; it is copied the same way (to
      `.rr-download`, hashed, replaced, unblocked).
- [ ] With no manifest from the server, a complete runtime already installed is used (one build serves every
      server); nothing is downloaded.

### Windows `unblock`

- [ ] `unblock` deletes only the `<file>:Zone.Identifier` stream of a file the launcher has just checked by hash,
      and ignores its absence. On Linux it does nothing (no alternate streams).

### The 1.14.7 executable

- [ ] PLAY hashes `Dauntless-Win64-Shipping.exe` afresh and compares it with `PINNED_EXE_SHA256`
      (`f3f95fbf…fbb66b`, `core/pins.rs`); `check_game_exe` answers `Pinned`, `OtherBuild` or `Missing`.
- [ ] The hash cache only serves the status display; an entry is used only while size and modification time are
      unchanged, and a hash is stored only when the file did not change while it was read.
- [ ] `file-hashes.json` is parsed defensively: entries without a 64-hex hash or with a non-numeric size or time are
      dropped, at most 32 are kept, and it is written through a `.tmp` file and a rename.

### Fuzzing and tests

- [ ] `runtime_input` ran for 24 hours with no crash before the cutover (RD10); the short CI run passes on every PR.
- [ ] Reforged's `runtime.test.ts` cases pass as Rust tests (`net/runtime_files/tests.rs`,
      `net/runtime_files/manifest.rs`), with the end-to-end cases against a local content server.

Reviewed commit: ________ Date: ________ Signed: ________
