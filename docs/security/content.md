<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: content server and runtime download (RX3)

The checklist for the content server, `rust/services/content` (MIGRATION_PLAN.md RS2, RD10). It covers the
"runtime download" component of RX3: what a player's launcher fetches before it installs the runtime.
`SECURITY_REVIEW.md` indexes it.

How to use it: before the content server's cutover, copy this list into the cutover PR, check each item in the code
(not from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The content server hands the host-built game runtime (`MysticParadox.dll` and `winmm.dll`) to registered players,
plus the host's news and art pack. The launcher loads that DLL into the game, so a server that could be made to
serve another file, or a list that does not match the bytes, would put someone else's code in every player's game.
The launcher checks every byte against the list again, and the player accepts a runtime per server.

- **In, from players:** HTTP through the gateway only. The server listens on loopback (`BIND_HOST`); the gateway is
  the only listener players reach. The request path picks the route and the file; the headers it reads are
  `x-reforged-user-api-key`, `if-none-match` and `x-forwarded-for` (log only, the gateway overwrites it).
- **In, from the host:** `CONTENT_RUNTIME_DIR` (`runtime.json` and the two DLLs, written by the host kit's
  `publish-runtime` or the launcher's Host page), `CONTENT_BRANDING_DIR`, `CONTENT_NEWS_FILE`, and the `.env` the
  host kit writes. The host is trusted with its own server, but whatever these hold is passed on to players, so it
  is checked before it is offered.
- **In, from the network:** the news file at `CONTENT_NEWS_URL` (raw.githubusercontent.com over HTTPS only), and the
  backend's answer to the key check (`GET <BACKEND_URL>/reforged/api/GetUserInfo`, loopback HTTP only).
- **Out:** the runtime files, their list, news and branding to players; account keys to the backend.

Code: `rust/services/content/src/` (`input.rs` holds every parser of outside input, `app.rs` the routes, `runtime.rs`
the runtime store, `auth.rs` the key check, `config.rs`, `news.rs`, `branding.rs`, `limits.rs`, `client.rs`,
`server.rs`).

Fuzz targets in `rust/fuzz` (each drives `content::input`):

- `content_request`: the request path, the account key and `if-none-match` headers, repeated lines included. Checks
  that a path accepted as a runtime file names exactly one of the two, and that an accepted key is one printable
  token.
- `content_runtime_index`: `runtime.json` bytes. Checks that an accepted index lists exactly the two files, sorted,
  with sizes in range and lower-case SHA-256s under the one build name, and that its body reads back to itself.
- `content_news`: a news document. Checks the item count, order, and that titles and bodies carry no control
  characters past what is allowed.

## Checklist

### Listening and limits

- [ ] Every `BIND_HOST` entry is `localhost`, a `127.x` address or `::1`; anything else stops the start
      (`config.rs`, `is_loopback_bind_host`). There is no override.
- [ ] At most 512 connections per listener, 20 s for the headers, and a connection that moves no byte for 120 s is
      dropped, which frees a download slot (`server.rs`).
- [ ] Downloads take a slot: at most `CONTENT_MAX_STREAMS_PER_ACCOUNT` per account (default 4, 1-64) and
      `CONTENT_MAX_STREAMS_TOTAL` in all (default 48, 1-1024). A refusal is 429 or 503 with `Retry-After`. A slot is
      released when the body is done or dropped (`limits.rs`, `app.rs` `Logged`; `tests/app.rs`).
- [ ] A panicking request does not stop the server (`panic = "unwind"` in the release profile; the key lookup's
      panic is caught and answered as `auth_unavailable`).

### Which file a request can reach

- [ ] Only five route shapes exist, matched on the raw path up to `?` with no decoding: the manifest, branding,
      news, `branding/<file>` and `runtime/files/<name>` (`input.rs`, `Route::of`). Anything else is 404, a known path
      with another method 405. There is no directory listing.
- [ ] A runtime file name must be exactly `MysticParadox.dll` or `winmm.dll`, letter case included, with no
      decoding; `..`, `%2e`, `/`, `\`, `runtime.json` and lookalikes are 404 (`is_runtime_file_name`;
      `tests/app.rs`, the parity scripts).
- [ ] The file served is `CONTENT_RUNTIME_DIR` joined with a name from the checked list, never with the request's
      text (`RuntimeStore::file`).
- [ ] A branding image name must match `IMAGE_NAME` (no folder, no leading dot, jpg/png/webp), be listed by the
      current scan, be a regular file of 1 byte to 25 MB, and start with the signature of its extension
      (`branding.rs`).

### The runtime and its list

- [ ] `runtime.json` is a regular file of at most 64 KiB (`load`). Its shape is checked exactly as Node's
      `ParseRuntimeIndex` does (`input.rs`, `parse_runtime_index`): the version `[A-Za-z0-9._+-]{1,48}` after
      trimming, a positive safe-integer changelist (default 647472), each of the two names once and no other, sizes
      1 byte to 64 MiB, SHA-256 as 64 lower-case hex.
- [ ] The list is offered only after both files were found as regular files (`symlink_metadata`, so a symlink is
      refused) with the listed size and SHA-256 (`load`). Otherwise the manifest is 404 `no_runtime` or 503
      `runtime_invalid`, and no file is served.
- [ ] The build name in the list is always `rel-1.14.7-Archon`, whatever the host wrote; the body is
      `RuntimeManifest::to_json` from `crates/protocol` and its ETag the SHA-256 of that body, byte for byte as Node
      (`runtime.rs` tests against RS1's fixture; the parity scripts).
- [ ] The folder is checked again at most every 10 s, and loaded and hashed again only when a file's size or
      modification time changed. Hashing runs off the async threads (`spawn_blocking`).
- [ ] A file that cannot be opened, or that is no longer a regular file of the listed size when it is served, is
      not sent: 503 `file_unavailable`, and the store forgets its check so the next request reloads the folder. The
      body is exactly the listed size (`take`), never more.

### Account keys

- [ ] A runtime file needs `x-reforged-user-api-key`. Repeated header lines are joined with `, ` as Node joins them,
      which makes the key implausible (`header_text`).
- [ ] A key that is not 1-256 printable ASCII characters without spaces is refused without asking the backend
      (`is_plausible_key`), so it cannot add a header or a line to the backend request.
- [ ] `BACKEND_URL` must be `http://` on a loopback host (`config.rs`); keys never leave the machine. A redirect is
      refused (`client.rs`), and the answer is read up to 1 MB with a 5 s timeout.
- [ ] Only 200 with a non-empty `UserId` is an account; 401/403 is a refused key; anything else is 503
      `auth_unavailable` with `Retry-After: 5`, and is not cached.
- [ ] The cache is keyed by the key's SHA-256 and never holds the key; good keys for `CONTENT_AUTH_CACHE_SECONDS`
      (default 300), refused ones for 30 s, at most 2000 entries; concurrent checks of one key share one lookup
      (`auth.rs` tests).
- [ ] No key reaches the log: the request log names the account id and username only (`tests/app.rs` checks every
      line).
- [ ] HEAD and 304 answers for a runtime file still need a valid key, as Node does.

### News and branding

- [ ] `CONTENT_NEWS_URL` is an HTTPS file URL on `raw.githubusercontent.com` with no port, credentials, query or
      fragment; anything else stops the start (`github_news_url`). It is fetched in the background at most once a
      minute, at most 1 MB, 5 s, with `If-None-Match`; a bad answer keeps the last good news.
- [ ] News items are plain text: control characters are removed or turned into spaces, titles cut at 200 UTF-16
      units and bodies at 10 000, at most 50 items, dates rewritten by the server (`parse_news`).
- [ ] The news file is at most 1 MB and read again at most every 5 s; a broken edit keeps the last good version.
      The branding folder is scanned again at most every 10 s; a broken `branding.json` means no list from it, and an
      entry can only name a file in the folder. `branding.json` has no size limit, in Node either: only the host
      writes it.
- [ ] Every answer carries `X-Content-Type-Options: nosniff`; JSON answers `Content-Type: application/json;
      charset=utf-8`; no CORS header is sent (Reforged sends none).

## Fuzzing

A short run of every target is in CI. The 24-hour run before the cutover, from `rust/fuzz` (on Windows the
AddressSanitizer runtime must be on PATH, see `rust/fuzz/Cargo.toml`):

```bash
cargo +nightly fuzz run content_request -- -max_total_time=86400
cargo +nightly fuzz run content_runtime_index -- -max_total_time=86400
cargo +nightly fuzz run content_news -- -max_total_time=86400
```

## Parity with the Node server

`tests/parity.rs` replays four request scripts against the Rust server and compares status, headers and body with
what `legacy/ReforgedContent` answered on the same fixture (`tests/parity/*.expected.json`, recorded by
`tests/parity/record.cjs`). To record them again:

```bash
cd legacy/ReforgedContent && npm ci && npx tsc -p tsconfig.test.json
node ../../rust/services/content/tests/parity/record.cjs content
```

## Changes from the Node server

- A `runtime.json`, news file or `branding.json` that JSON.parse accepts but `serde_json` does not (a lone UTF-16
  surrogate escape such as `"\ud800"`, or nesting deeper than 128) is refused as invalid. Refusing is the safe side.
- A title or body cut at its limit in the middle of a surrogate pair loses the whole character, where JavaScript
  keeps half of it.
- The backend's answer to the key check is read up to 1 MB; Node read it whole.
- Node also closed a request after 30 s (`requestTimeout`) and an idle keep-alive connection after 5 s; the Rust
  server has the 20 s header timeout and the 120 s idle timeout only. Requests carry no body, so the 30 s limit had
  nothing to cut short.
- DR's client refused GitHub's 304 as a redirect and logged a warning each minute; only 301, 302, 303, 307 and 308
  are redirects now, as for `fetch`.

Signed: ________ (date, commit)
