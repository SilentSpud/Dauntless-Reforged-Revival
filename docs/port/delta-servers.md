<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Delta list: gateway, allowlist helper and content server (RX1)

What Reforged changed in its Node gateway, allowlist helper and content server, compared with Dauntless Revived's
(DR) Node sources at `e1c5fec`, so the Rust copies in RS2, RS3 and RS4 carry every one of those changes.

- Reforged paths are as they are on this branch. RR2 is moving `ReforgedGateway/` and `ReforgedContent/` to
  `legacy/` at the same time, so prefix them with `legacy/` once RR2 lands.
- DR Node paths are under `legacy/UndauntedGateway/` and `legacy/UndauntedContent/` at `e1c5fec`. DR Rust paths are
  under `rust/services/` at `e1c5fec`.
- Line ranges are in the Reforged file. "Changed lines" counts the `<` and `>` lines of a plain `diff` between
  the DR Node file and the Reforged file.
- Some differences come from DR changing its Node code after Reforged copied it, not from Reforged. Those are
  listed under "DR-only, not to carry" for each file. The Rust copy already has them; whether to keep them is
  in "Open points".

## Identical, nothing to carry

These Reforged files match DR's Node file at `e1c5fec` byte for byte. The DR Rust module ports them as they are.

| Reforged | DR Node | DR Rust | Row |
| --- | --- | --- | --- |
| `ReforgedGateway/src/allowlist/state.ts` | `legacy/UndauntedGateway/src/allowlist/state.ts` | `gateway/src/allowlist/state.rs` | RS4 |
| `ReforgedGateway/src/allowlist/sync.ts` | `legacy/UndauntedGateway/src/allowlist/sync.ts` | `gateway/src/allowlist/sync.rs` | RS4 |
| `ReforgedGateway/src/ip.ts` | `legacy/UndauntedGateway/src/ip.ts` | `gateway/src/ip.rs` | RS3 |
| `ReforgedGateway/src/log.ts` | `legacy/UndauntedGateway/src/log.ts` | `service-kit` `linelog` | RS3 |
| `ReforgedGateway/src/ratelimit.ts` | `legacy/UndauntedGateway/src/ratelimit.ts` | `gateway/src/ratelimit.rs` | RS3 |
| `ReforgedGateway/src/redact.ts` | `legacy/UndauntedGateway/src/redact.ts` | `gateway/src/redact.rs` | RS3 |
| `ReforgedContent/src/branding.ts` | `legacy/UndauntedContent/src/branding.ts` | `content/src/branding.rs` | RS2 |
| `ReforgedContent/src/limits.ts` | `legacy/UndauntedContent/src/limits.ts` | `content/src/limits.rs` | RS2 |
| `ReforgedContent/src/log.ts` | `legacy/UndauntedContent/src/log.ts` | `content/src/log.rs` | RS2 |
| `ReforgedContent/src/news.ts` | `legacy/UndauntedContent/src/news.ts` | `content/src/news.rs` | RS2 |

## DR files with no Reforged counterpart

These DR files are not in Reforged. The Rust copy drops the module unless noted.

- `legacy/UndauntedGateway/src/capture.ts` (traffic capture, DR M2, added after Reforged's copy). The DR Rust
  gateway at `e1c5fec` has no capture module either. The plan keeps a capture option for RB2: see "Open points".
- `legacy/UndauntedContent/src/gamedir.ts`, `manifest.ts`, `paths.ts`, `verify.ts` and
  `data/dauntless-1.4.4.json`: the 1.4.4 game folder, its manifest and `npm run verify`. Drop
  `content/src/gamedir.rs`, `manifest.rs`, `paths.rs` and `verify.rs` (RS2).
- `legacy/UndauntedContent/src/range.ts`: HTTP ranges. Reforged serves no ranges, but it kept
  `IfNoneMatchHits` (now in `app.ts`, see below). From `content/src/range.rs` keep only `if_none_match_hits` (RS2).
- DR Rust only: `content/src/client.rs`, `gateway/src/guard.rs`, `gateway/src/allowlist/windows.rs`,
  `gateway/src/allowlist/mod.rs`, `gateway/src/bin/allowlist.rs`, `lib.rs` and `main.rs` of both services. These
  are the Rust split of code that sits in the Node files below. They carry the deltas listed for those files.

## Gateway (RS3)

### `ReforgedGateway/src/policy.ts`

- DR Node: `legacy/UndauntedGateway/src/policy.ts`
- DR Rust: `rust/services/gateway/src/policy.rs` (fuzz target `rust/fuzz/fuzz_targets/gateway_policy.rs`)
- Changed lines: 108
- Row: RS3

Behavior to carry:

- Upstream renamed `metagame` to `backend` (`Upstream`, line 8; every `proxy` decision, lines 171, 191, 194,
  197). The access log's `upstream` field and the config key follow (see `config.ts`).
- The public API prefix is `/reforged/api`, not `/undaunted/api` (lines 22-30, 150, 165-172). Register's rate
  class keys on `/reforged/api/register` (line 150).
- `PUBLIC_REFORGED_API` (lines 25-30) is `register` (POST), `getuserinfo`, `serverstatus` and
  `registrationstatus` (GET, HEAD). DR's `huntregion` and `devicesignals` are gone. Everything else under
  `/reforged` answers 403, even with a valid admin key.
- New host-only path prefixes `/admin`, `/launcher`, `/internal` and `/health` (lines 32-38). Any path equal to
  one of them or under it answers 403 with reason `host_only_route` (lines 174-176). The match is on the
  canonical path, after the `/__origin` unwrap, and uses the `Under` helper (lines 124-126).
- Host-only headers (lines 40-43, 159-163): `x-mysticparadox-gameserver-apikey` (DR's was
  `x-undaunted-gameserver-apikey`), `x-reforged-admin-key` and `x-mysticparadox-metrics-token`. Any of them on a
  request answers 403. The reason is `gameserver_key_header` for the first and `host_only_header` for the other
  two.
- New `/__origin/<host><rest>` unwrapping (lines 95-119, 138-142). The 1.14.7 runtime sends the game's calls as
  `/__origin/<official host>/<path>`, and every rule then applies to `<rest>`. The rules:
  - the match is case-insensitive and allows extra leading slashes;
  - the host is percent-decoded and lower-cased, and must be `[a-z0-9.-]{1,253}`, with no leading or trailing dot
    and no `..`;
  - an empty rest becomes `/`, and a rest without a leading slash gets one;
  - a bare `/__origin` or `/__origin/` without a host, an undecodable host, or a second `/__origin` inside the
    rest answers 400 `bad_path` with reason `bad_origin` and rate class `general`.
- Content routing (lines 145-147, 152, 186): `/content` is content only when it was not behind `/__origin`. An
  `/__origin/<host>/content/...` path goes to the backend.
- `CheckPath` (line 63) rejects `\`, `0x00-0x20` and `0x7f`. DR Node's `[\\\u0000-\s]` was a broken class. DR
  Rust's `check_path` already rejects backslash, controls, `0x7f` and JavaScript whitespace, which is stricter
  than Reforged on Unicode spaces such as U+00A0. Keep DR Rust's rule: Paradox paths are ASCII.
- Only `POST /heartbeat` feeds the allowlist with `feed: "heartbeat"` (lines 190-192). DR Node and DR Rust also
  feed on `POST /game/login`. See "Open points".

### `ReforgedGateway/src/config.ts`

- DR Node: `legacy/UndauntedGateway/src/config.ts`
- DR Rust: `rust/services/gateway/src/config.rs`
- Changed lines: 39
- Row: RS3

Behavior to carry:

- New `GATEWAY_TLS` key (lines 39-41, 155). Any value but `0` is public mode (TLS); `0` is private mode, plain
  HTTP.
- In private mode the bind address must be a Tailscale IPv4 address (100.64.0.0/10, `IsTailscaleIPv4`, lines
  102-109) or loopback, or the start fails (lines 160-163).
- `GATEWAY_CERT` and `GATEWAY_KEY` are required only in public mode (lines 167-169). The error text names
  `GATEWAY_TLS=0` as the other choice. In private mode `certFile` and `keyFile` are empty (lines 183-184).
- `GATEWAY_PORT` defaults to 443 in public mode and 61000 in private mode (line 181).
- `GATEWAY_METAGAME_URL` is renamed `GATEWAY_BACKEND_URL`, default `http://127.0.0.1:61000` (line 187). The
  config field is `backend` (line 45).
- `GATEWAY_WS_URL` defaults to `http://127.0.0.1:61000`, the backend's own listener, not DR's 61099 (lines
  188-189).
- Comment only: the body-limit note now refers to Express's 100 KB JSON limit (lines 53-55). The limits
  themselves are unchanged.

### `ReforgedGateway/src/gateway.ts`

- DR Node: `legacy/UndauntedGateway/src/gateway.ts`
- DR Rust: `rust/services/gateway/src/gateway.rs` and `guard.rs`
- Changed lines: 118 (about 80 of them are DR's later capture code, which Reforged does not have)
- Row: RS3

Behavior to carry:

- The gateway's shared-secret header is `x-reforged-gateway`, not `x-dauntless-gateway`. It is stripped from
  incoming requests (line 23), set on every proxied request (line 72) and dropped from responses (line 28).
- `ForwardHeaders` takes the scheme (line 60). `x-forwarded-proto` is `https` in public mode and `http` in
  private mode (lines 71, 386, 527).
- Private mode serves plain HTTP with the same timeouts and limits (lines 145-161). `Certificate` is `null` in
  private mode (lines 114-116, 130). A TLS config without a certificate is a programming error (lines 127-129).
- The upstream lookup uses `backend` (line 329).

DR-only, not to carry: the capture writer, body taps and `Record()` (DR lines 6, 106-107, 127-135, 230-267 and
others), added by DR M2. See "Open points".

### `ReforgedGateway/src/feed.ts`

- DR Node: `legacy/UndauntedGateway/src/feed.ts`
- DR Rust: `rust/services/gateway/src/feed.rs`
- Changed lines: 39
- Row: RS3

Behavior to carry: none. Every changed line is DR's later feed backoff (DR commit `d6abb33`: after 3 failures in
a row the feed skips a helper that is down for 2 s, doubling to 60 s). Reforged's copy predates it. DR Rust
`feed.rs` has the backoff (`BACKOFF_AFTER`, `BACKOFF_BASE_MS`, `BACKOFF_MAX_MS`). See "Open points".

### `ReforgedGateway/src/server.ts`

- DR Node: `legacy/UndauntedGateway/src/server.ts`
- DR Rust: `rust/services/gateway/src/server.rs` and `main.rs`
- Changed lines: 63
- Row: RS3

Behavior to carry:

- The certificate is loaded, checked against the key, checked for validity and warned about within 30 days of
  expiry only in public mode (lines 22-45). Private mode starts with no certificate. The certificate is loaded,
  never made (section 6: every invite's `fp`).
- The start-up log has `mode` (`public (TLS)` or `private (plain HTTP inside the tailnet)`). Fingerprint, names
  and expiry are logged only when there is a certificate. `backend` replaces `metagame` (lines 60-68).

DR-only, not to carry: the capture set-up (DR lines 49-61).

### `ReforgedGateway/tools/make-cert.js`

- DR Node: `legacy/UndauntedGateway/tools/make-cert.js`
- DR Rust: `rust/services/gateway/src/cert.rs` (`make_cert_main`), run as `gateway make-cert`
- Changed lines: 6
- Row: RS3

Behavior to carry:

- The certificate subject's `organizationName` is `Dauntless Reforged Revival server` (line 129). DR Rust sets
  `Dauntless Revived server` (`cert.rs` line 243). This only affects new certificates; existing ones are loaded
  as they are.
- Usage text: the example folder is `C:\ReforgedServer\tls` (lines 7, 9).
- The plan names the subcommand `gateway cert`, but DR Rust has `gateway make-cert`. See "Open points".

## Allowlist helper (RS4)

### `ReforgedGateway/src/allowlist/ports.ts`

- DR Node: new (the code was `CheckPorts` in `legacy/UndauntedGateway/src/allowlist/config.ts`)
- DR Rust: `rust/services/gateway/src/allowlist/config.rs` (`check_ports`)
- Changed lines: 13 (new file, moved verbatim)
- Row: RS4

Behavior to carry: none. This is a move, so `nft.ts` can import it without a cycle. `check_ports` in DR Rust
already matches it: `ALLOWLIST_PORTS` is one port or `low-high` in 1-65535, and the result is normalised.
`--max-hunts` widening the range is the host kit's job: it writes `ALLOWLIST_PORTS` (RH rows).

### `ReforgedGateway/src/allowlist/nft.ts`

- DR Node: new (adapted from DR's `deploy/linux-server/allowlist.mjs`, which is not in the Node gateway)
- DR Rust: `rust/services/gateway/src/allowlist/firewall.rs` (`NftSettings`, `nft_table_script`, `nft_set_script`,
  `Nft`)
- Changed lines: 139 (new file)
- Row: RS4

DR Rust already has an nftables backend, but it differs from Reforged's. Behavior to carry:

- Default table name `reforged_revival` (line 12). DR Rust's is `dauntless_revived`. A Debian host that already
  runs the Node helper has a `reforged_revival` table, which the Rust helper must replace, not leave next to its
  own.
- Settings and their keys (`config.ts` lines 69-78, 93-97):
  - `ALLOWLIST_NFT`: the nft binary. It must be an absolute path, and PATH is never searched. Otherwise the first
    of `/usr/sbin/nft` and `/sbin/nft` that exists, else `/usr/sbin/nft` (lines 15-20, 37-44). DR Rust reads
    `DR_NFT_BINARY` and searches PATH first.
  - `ALLOWLIST_NFT_TABLE`: a letter, then up to 31 letters, digits or `_` (lines 22-27). DR Rust reads
    `DR_NFT_TABLE` and allows a leading digit.
  - `ALLOWLIST_INTERFACE`: an interface name of at most 15 characters, starting with a letter or digit, then
    `[A-Za-z0-9_.@-]` (lines 29-35). DR Rust reads `DR_FIREWALL_INTERFACE`, allows 32 characters and allows `:`.
- The script (lines 53-89) is one `nft -f` transaction that replaces the whole table on every change:
  `table inet T {}`, `delete table inet T`, then the full table with sets `players4` and `players6` and their
  elements. The swap is atomic, so there is never a moment with the ports open to everyone or with the table
  missing. DR Rust builds the table once and then flushes and refills the sets in separate `nft` runs.
- `iifname "lo" accept` is the first rule of the chain (line 81), so loopback is never filtered. DR Rust has no
  such rule.
- A single port stays a single port (`udp dport 8777`). DR Rust always writes `low-high`. The rules match the
  same packets either way.
- Chain `input`, `type filter hook input priority -5; policy accept;`. When an interface is set, it is the
  `iifname` prefix on the three game-port rules (lines 79-85). These match DR Rust.
- Every address is checked again with `CheckAllowlistIp`, and it must already be canonical (lines 59-65). DR
  Rust's `plan` does the same.
- Running nft (lines 91-130): the script goes into a new `0700` temporary folder as a `0600` file opened with
  `wx`, then runs `nft -f <file>` with no shell. Timeout 30 s; stderr is kept to its last 4000 characters, and
  the error shows the first 1000. The folder is removed afterwards. The comment says nft refuses a pipe on stdin,
  but DR Rust feeds `nft -f -` on stdin. See "Open points".
- Start-up check (lines 132-139): `nft list tables` must succeed with a 15 s timeout, or the helper refuses to
  start in real mode (`server.ts` lines 34-36).

### `ReforgedGateway/src/allowlist/config.ts`

- DR Node: `legacy/UndauntedGateway/src/allowlist/config.ts`
- DR Rust: `rust/services/gateway/src/allowlist/config.rs`
- Changed lines: 42
- Row: RS4

Behavior to carry:

- New `ALLOWLIST_FIREWALL` key, `windows` (default) or `nftables`; anything else stops the start (lines 7, 15,
  69-73, 84). DR Rust chooses the backend by the platform it was built for. Reforged chooses it from the key and
  refuses a key that does not match the platform (`server.ts` lines 31-43).
- `ALLOWLIST_NFT`, `ALLOWLIST_NFT_TABLE` and `ALLOWLIST_INTERFACE` (lines 74-78, 93-97), described under
  `nft.ts`.
- `CheckPorts` moved to `ports.ts` and is re-exported (lines 45-46).

### `ReforgedGateway/src/allowlist/firewall.ts`

- DR Node: `legacy/UndauntedGateway/src/allowlist/firewall.ts`
- DR Rust: `rust/services/gateway/src/allowlist/firewall.rs` and `windows.rs`
- Changed lines: 15
- Row: RS4

Behavior to carry:

- The Windows Firewall rule's `Name` is `ReforgedRevival-GamePorts-Allowlist` and its `DisplayName` is
  `Dauntless Reforged Revival game ports (allowlist)` (lines 9-10). DR Rust's `RULE_DISPLAY_NAME` is
  `Dauntless Revived game ports (allowlist)`, and DR Rust finds the rule through COM by display name. It must use
  Reforged's display name, or a host that moves over keeps the old PowerShell-made rule and gets a second one.
- `BuildScriptFor` (lines 59-63) picks PowerShell or nftables from `ALLOWLIST_FIREWALL`. In Rust this is the
  `Backend` choice.

### `ReforgedGateway/src/allowlist/helper.ts`

- DR Node: `legacy/UndauntedGateway/src/allowlist/helper.ts`
- DR Rust: `rust/services/gateway/src/allowlist/helper.rs`
- Changed lines: 4
- Row: RS4

Behavior to carry: none beyond the backend choice. The sync applies `BuildScriptFor(Config, Ips)` instead of the
Windows script (lines 8, 94).

### `ReforgedGateway/src/allowlist/server.ts`

- DR Node: `legacy/UndauntedGateway/src/allowlist/server.ts`
- DR Rust: `rust/services/gateway/src/bin/allowlist.rs`
- Changed lines: 17
- Row: RS4

Behavior to carry:

- Real mode with `ALLOWLIST_FIREWALL=nftables` requires Linux and a working `nft list tables`. The fatal message
  names `apt install nftables` and the `reforged-allowlist` systemd unit (lines 31-39).
- Real mode with `windows` on a platform that is not Windows fails with a message that points to
  `ALLOWLIST_FIREWALL=nftables` (lines 41-43). The administrator check is Windows-only, so DR Rust's
  `is_elevated()` check (bin line 68) must not run for nftables.
- `--close-ports` closes through the chosen backend (line 52).
- The ready log has `firewall` (line 72).

## Content server (RS2)

### `ReforgedContent/src/runtime.ts`

- DR Node: new
- DR Rust: new module (for example `content/src/runtime.rs`), replacing `manifest.rs`, `gamedir.rs`, `paths.rs` and
  `verify.rs`
- Changed lines: 194 (new file)
- Row: RS2 (the manifest type itself is RS1's, in `crates/protocol`)

Behavior to carry, all new:

- The runtime is exactly two files, `MysticParadox.dll` and `winmm.dll` (line 19), with a `runtime.json` next to
  them in `CONTENT_RUNTIME_DIR` (lines 6-17).
- Limits: each file is 1 byte to 64 MiB (line 20), and `runtime.json` is at most 64 KiB and a regular file (lines
  21, 158-160). A UTF-8 BOM is stripped (line 161).
- `ParseRuntimeIndex` (lines 38-79):
  - `version` is trimmed and must be `[A-Za-z0-9._+-]{1,48}`;
  - `build.changelist` is a positive safe integer and defaults to 647472;
  - `files` lists each of the two names exactly once, with no other names;
  - each `size` is an integer within the limits and each `sha256` is 64 lowercase hex characters.
- The parsed index is normalised: files sorted by name, and `build` is always
  `{ "name": "rel-1.14.7-Archon", "changelist": N }`, whatever `build.name` the file had (line 78).
- A runtime is offered only when every listed file is a regular file (`lstat`, so a symlink is refused) with the
  listed size and SHA-256 (lines 173-190).
- Three states with reasons (lines 32-35): `ok`, `none` (no `CONTENT_RUNTIME_DIR`, or no `runtime.json`) and
  `invalid` (anything that does not check out). `app.ts` maps them to 200, 404 and 503.
- The manifest body is `JSON.stringify` of the normalised index, in key order `version`, `build` (`name`,
  `changelist`), `files` (`name`, `size`, `sha256`). The ETag is `"<sha256 hex of that body>"` (lines 191-192).
  The Rust copy must write the same bytes so ETags stay equal (section 6).
- Re-check without a restart (lines 22, 115-150): at most every 10 s on a request, the folder's signature (size
  and mtime of `runtime.json` and both files) is compared with the last one, and the folder is loaded and hashed
  again only when it changed. A change of state or version is logged once (`runtime ready` at info, `no runtime
  is offered to players` at warn for invalid and info for none).
- `invalidate()` (lines 109-113) forgets the last check. `app.ts` calls it when a file fails while being served.

### `ReforgedContent/src/app.ts`

- DR Node: `legacy/UndauntedContent/src/app.ts`
- DR Rust: `rust/services/content/src/app.rs` (fuzz target `rust/fuzz/fuzz_targets/content_range.rs` goes with the
  ranges)
- Changed lines: 218
- Row: RS2

Behavior to carry:

- Routes (lines 11-23, 113-117), all GET or HEAD; anything else is 405 with `Allow: GET, HEAD`, and unknown paths
  are 404:
  - `GET /content/v1/runtime/manifest`, no key needed (lines 129-147). 404 `no_runtime` when nothing is
    published and 503 `runtime_invalid` when it does not check out, each with the state's reason in the log.
    `If-None-Match` hits give 304 with `ETag`. Otherwise 200 JSON with `ETag` and `Cache-Control: no-cache`.
  - `GET /content/v1/runtime/files/<name>` (lines 167-176, 242-358). These replace DR's `/content/v1/manifest`
    and `/content/v1/files/<path>`.
  - `/content/v1/branding`, `/content/v1/branding/<file>` and `/content/v1/news` are unchanged.
- Runtime file names (lines 243-248): the name must be exactly one of the two, letter case included, with no
  decoding. Anything else is 404 `not_found`, reason `not_a_runtime_file`.
- Key header `x-reforged-user-api-key` (line 251), not `x-undaunted-user-api-key`. A missing key or a refused key
  is 401 `unauthorized`. A key check that cannot be made is 503 `auth_unavailable` with `Retry-After: 5`. These
  match DR.
- When the runtime vanished after the route check: 404 `not_found`, reason `no_runtime` (lines 272-277).
- The file ETag is `"<sha256>"` from the list (line 279). `If-None-Match` hits give 304 (lines 280-285).
- HEAD gives 200 with `ETag`, `Content-Type: application/octet-stream` and `Content-Length` and no stream slot.
  Unlike GET, it has no `Cache-Control` (lines 287-294).
- No HTTP ranges at all: no `Accept-Ranges`, no `Range` or `If-Range` handling, no 206 or 416. GET always sends
  the whole file (lines 341-349), with `Cache-Control: private, no-transform` (line 345).
- Stream slots and their 429 and 503 answers are unchanged (lines 296-310).
- When a file cannot be opened (lines 314-322), the runtime store is invalidated and the answer is 503
  `file_unavailable`, reason `open_failed`. When it is no longer a regular file or its size no longer matches the
  list (lines 332-338), the store is invalidated and the answer is 503 `file_unavailable` with `Retry-After: 10`,
  reason `changed_on_disk`. DR marked the file broken until a restart. The size check has no mtime part: the
  store's signature already covers mtime.
- The read stream uses 256 KiB chunks (line 349). DR used 1 MiB.
- The CORS headers are gone. DR's `Public()` set `Access-Control-Allow-Origin: *` and
  `Cross-Origin-Resource-Policy: cross-origin` on the manifest, branding, branding images and news. Reforged sends
  neither. DR Rust's `Reply::public` (`app.rs` lines 217-220) must not be called.
- `IfNoneMatchHits` moved here from `range.ts` (lines 70-79). It is unchanged: `*`, or a comma list with weak tags
  compared by their opaque tag.
- Logging (lines 91-111): `ip` is `x-forwarded-for` when present, else the socket address (line 99). The gateway
  overwrites that header with the real peer. Routes are named `runtime-manifest` and `runtime-file`, and a GET of
  a runtime file with bytes logs as `download`. There is no `range` field.
- `SendJson` takes a pre-serialised string (line 50), so the runtime body goes out byte for byte as the store
  made it.

### `ReforgedContent/src/auth.ts`

- DR Node: `legacy/UndauntedContent/src/auth.ts`
- DR Rust: `rust/services/content/src/auth.rs`
- Changed lines: 31
- Row: RS2

Behavior to carry:

- The key lookup is `GET <BACKEND_URL>/reforged/api/GetUserInfo` (lines 125-129), not
  `/undaunted/api/GetUserInfo`. The function is renamed `BackendLookup`.
- The key header sent to the backend is `x-reforged-user-api-key` (line 136).
- The "unavailable" reasons say `backend`, not `metagame` (lines 142, 152, 160, 165). These texts reach the log
  only.
- The cache, its TTLs, the single in-flight lookup per key and the key-shape check are unchanged.

### `ReforgedContent/src/config.ts`

- DR Node: `legacy/UndauntedContent/src/config.ts`
- DR Rust: `rust/services/content/src/config.rs`
- Changed lines: 82
- Row: RS2

Behavior to carry:

- `BIND_HOST` entries must all be loopback: `localhost`, any `127.x` IPv4 address, or `::1` (lines 34-44,
  49-57). Players reach the content server through the gateway only. DR also allowed Tailscale addresses, and
  any address with `CONTENT_ALLOW_ANY_BIND=1`. That key is gone.
- `METAGAME_URL` is renamed `BACKEND_URL`, default `http://127.0.0.1:61000` (lines 59-69). It must be `http:` on a
  loopback host, because account keys travel in it. DR allowed any `http` or `https` host.
- Removed keys: `CONTENT_GAME_DIR` (DR's only required key), `CONTENT_MANIFEST` and `CONTENT_DOWNLOAD_BASE_URL`.
- New optional `CONTENT_RUNTIME_DIR`, resolved to an absolute path (line 80). Unset means no runtime is offered,
  which is not a start error.
- `CONTENT_NEWS_URL` moved into the config (line 83). Its validation still happens in the `News` constructor, and
  `server.ts` makes a bad value fatal. DR Rust validates it in config with `github_news_url`, which is fine.
- `CONTENT_MAX_STREAMS_PER_ACCOUNT` defaults to 4, not 6 (line 84). The range 1-64 is unchanged.

### `ReforgedContent/src/server.ts`

- DR Node: `legacy/UndauntedContent/src/server.ts`
- DR Rust: `rust/services/content/src/server.rs` and `main.rs`
- Changed lines: 63
- Row: RS2

Behavior to carry:

- There is no manifest load and no game-folder check at start (DR's lines 37-70 are gone). The start creates a
  `RuntimeStore` from `CONTENT_RUNTIME_DIR`. When no runtime is ready it logs a warning with the folder and
  reason and keeps running (lines 36-40).
- A bad `CONTENT_NEWS_URL` is a fatal `Configuration:` error at start (lines 42-48).
- Auth uses `BackendLookup(BACKEND_URL)` (line 52).
- The start-up log has `listen` (every `host:port`), `backend`, `runtime` (folder or null), and `news` as the
  file, else the URL, else null (lines 72-78). DR's per-host "Dauntless Revived content server on ..." line is
  gone.
- `SIGBREAK` stops the server like `SIGINT` and `SIGTERM` (line 93), for a console window closed on Windows. In
  Rust this is `tokio::signal::windows::ctrl_break`, next to `ctrl_c`.

## Open points

1. **Traffic capture.** Plan row RS3 keeps "the traffic-capture option" for RB2. DR Node has it
   (`legacy/UndauntedGateway/src/capture.ts`), but the DR Rust gateway at `e1c5fec` has no capture module, and
   Reforged's Node gateway never had one. RS3 either ports `capture.ts` into Rust or RB2 records on the Node
   gateway with `capture.ts` copied in. That needs a decision before RS3 is briefed.
2. **Allowlist feed backoff.** DR Rust `feed.rs` backs off from a helper that is down (DR `d6abb33`), and Reforged's
   Node feed does not. Keeping it changes nothing a player sees and stops the log flood the DR commit describes.
   Recommendation: keep it, and list it in RS3's PR as a behavior change from the Node gateway.
3. **`POST /game/login` as an allowlist feed.** DR Rust feeds the allowlist on `/game/login` as well as
   `/heartbeat`; Reforged feeds only on `/heartbeat`. Whether to keep it depends on whether Mystic Paradox's
   `/game/login` (under `/__origin`) checks the bearer before it answers 2xx, as DR's metagame did. RS3 should
   check that against the Paradox fork and drop the `/game/login` branch if not.
4. **nft on stdin.** Reforged's `nft.ts` writes a file because, its comment says, nft refuses a pipe on stdin.
   DR Rust uses `nft -f -`. RS4's Debian 13 check should try `-f -` first. If it fails, the Rust copy writes a
   `0600` file in a `0700` folder as Reforged does.
5. **One atomic transaction or prepare-then-flush.** Reforged swaps the whole table in one transaction. DR Rust
   prepares the table once, then flushes and refills the sets in a second `nft` run, which leaves a short window
   with empty sets (ports closed, not open). Recommendation: Reforged's single transaction, plus its `lo` rule.
6. **Env names on Linux.** Reforged's host kit writes `ALLOWLIST_NFT`, `ALLOWLIST_NFT_TABLE` and
   `ALLOWLIST_INTERFACE`, and DR Rust reads `DR_NFT_BINARY`, `DR_NFT_TABLE` and `DR_FIREWALL_INTERFACE`. Section 6
   keeps the host kit's `.env` files, so the Rust helper reads the `ALLOWLIST_*` names. The open question is
   whether it also accepts the `DR_*` names; recommendation: no.
7. **PATH lookup for nft.** DR Rust searches PATH for `nft`. Reforged never does, because the helper runs with
   `CAP_NET_ADMIN`. Recommendation: Reforged's rule.
8. **Subcommand name.** The plan says `gateway cert` replaces `tools/make-cert.js`, and DR Rust's subcommand is
   `gateway make-cert`. The host kit (RH rows) calls whichever is chosen. Keeping `make-cert` matches DR and
   Reforged's `npm run make-cert`.
9. **`ALLOWLIST_POWERSHELL`.** DR Rust drives Windows Firewall through COM, so Reforged's PowerShell path setting
   has nothing to do. The Rust helper should accept and ignore it, so an existing `.env` still starts.
10. **Runtime manifest type.** RS1 defines the manifest type in `crates/protocol`. RS2's JSON must still be byte
    for byte what `runtime.ts` writes (key order, `build.name` forced), so serialise the struct in field order
    `version`, `build { name, changelist }`, `files [{ name, size, sha256 }]`, and test against a body recorded
    from the Node server.
