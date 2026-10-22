<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Security checklist: gateway policy and TLS (RX3)

The checklist for the gateway, the `gateway` binary of `rust/services/gateway` (MIGRATION_PLAN.md RS3, RD10, RD19,
RD20). `SECURITY_REVIEW.md` indexes it. The allowlist helper, the crate's other binary, has its own list in
[allowlist.md](allowlist.md).

How to use it: before the gateway's cutover, copy this list into the cutover PR, check each item in the code (not
from memory), link the code or test that shows it, and sign at the end with the date and the commit reviewed. An
item that does not hold is a bug to fix before the cutover, not a box to leave open.

## What it is and where the boundary is

The gateway is the server's only public port. Everything a player's game, runtime and launcher send comes through
it; everything behind it (the Mystic Paradox backend, its chat listener, the content server, the allowlist helper)
listens on loopback and trusts that the gateway already refused what must not reach it.

- **In, public mode:** TLS on `GATEWAY_BIND:GATEWAY_PORT` (default `0.0.0.0:443`) with the server's self-signed
  certificate. The launcher pins its SHA-256 fingerprint, the `fp` of every v2 invite, so the certificate is loaded
  and never made at start.
- **In, private mode (`GATEWAY_TLS=0`):** plain HTTP, default port 61000, only on a Tailscale address
  (100.64.0.0/10) or loopback. Tailscale encrypts the traffic; the gateway refuses to start on any other address.
- **Out:** plain HTTP to the backend (`GATEWAY_BACKEND_URL`), the content server (`GATEWAY_CONTENT_URL`) and the chat
  listener (`GATEWAY_WS_URL`), all on loopback only, each request carrying the shared secret `x-reforged-gateway`;
  `POST /allow` to the allowlist helper (`ALLOWLIST_URL`, also loopback) with `x-allowlist-secret`.
- **Files:** the certificate and key (read only), the `.env` the host kit writes (trusted), and when the capture is
  on, the capture folder (`CAPTURE_DIR`, written).

Code: `rust/services/gateway/src/` (`policy.rs`, `gateway.rs`, `guard.rs`, `ip.rs`, `ratelimit.rs`, `redact.rs`,
`feed.rs`, `capture.rs`, `cert.rs`, `config.rs`, `server.rs`, `main.rs`).

Fuzz targets in `rust/fuzz`:

- `gateway_policy`: a method, target and headers from the network give a decision, never a panic, and nothing
  proxied carries a host-only header or reaches a host-only prefix or a non-public `/reforged` route, whatever the
  spelling or the `/__origin` wrapping.
- `gateway_capture`: the capture's redaction of bodies, headers, targets and chat frames never panics and never
  writes an account key or a whole token from its input; the chat frame reader gives the same messages however the
  bytes are split.

## Checklist

### TLS and the certificate

- [ ] Public mode loads `GATEWAY_CERT` and `GATEWAY_KEY` and never makes a certificate (`server.rs`, `load_tls`).
      The start stops if the key does not belong to the certificate (`cert::load`) or the certificate is not valid
      now; it warns within 30 days of expiry.
- [ ] A certificate made by the Node `tools/make-cert.js` loads, and the fingerprint the Node host kit put in an
      invite is the one the gateway serves (`tests/certificate.rs`, fixture in `tests/fixtures/node-kit`).
- [ ] `gateway make-cert` refuses to replace an existing certificate or key without `--force`, and says that a new
      one breaks every invite (`cert.rs`, `make_cert_main`). The key is written 0600 where the file system has
      modes. On Windows the folder's ACL is the host kit's job (`legacy/ReforgedHost`, later RH rows).
- [ ] TLS 1.2 and 1.3 only, aws-lc-rs, ALPN `http/1.1` only (`cert::server_config`). The handshake has its own
      timeout (`GATEWAY_HANDSHAKE_TIMEOUT_MS`, 10 s) and counts against the per-address connection rate.
- [ ] A public config without a certificate, or a private one with one, does not start (`Gateway::new`).
- [ ] Private mode binds only a Tailscale IPv4 address or loopback (`config.rs`, `is_tailscale_ipv4`); `0.0.0.0`,
      `::`, LAN and public addresses stop the start. `x-forwarded-proto` is then `http`.

### What reaches the backend

- [ ] The request target must be origin-form; absolute-form and `*` are 400 (`split_target`).
- [ ] `check_path` refuses backslashes, control characters, white space (Unicode spaces included), encoded `/`, `\`,
      NUL and `.`, and dot segments, before anything else looks at the path.
- [ ] `/__origin/<host><rest>` is unwrapped once, as the backend does (`unwrap_origin`): the host is
      percent-decoded, lower-cased and must be `[a-z0-9.-]{1,253}` without a leading or trailing dot or `..`; a bare
      `/__origin`, an undecodable or odd host, or a second `/__origin` inside is 400 `bad_origin`. Every rule below
      applies to `<rest>`.
- [ ] Paths are compared the way Express matches routes, and more leniently: percent-decoded, lower case, repeated
      slashes folded, trailing slashes dropped (`canonical_path`).
- [ ] Under `/reforged`, only `POST register`, `GET`/`HEAD getuserinfo`, `serverstatus` and `registrationstatus` under
      `/reforged/api/` pass; every other route (invites, account status) is 403 `admin_route`, even with a valid admin
      key and even as an upgrade. The test reads every route from `paradox/backend/src/routes/reforgedApi.ts`, so a
      route added there is covered (`policy.rs` and `tests/gateway.rs`, `only_the_public_reforged_routes_*`).
- [ ] `/admin`, `/launcher`, `/internal` and `/health`, and everything under them, are 403 `host_only_route`
      (Mystic Paradox's admin API, its own launcher API, metrics and readiness probes).
- [ ] A request with `x-mysticparadox-gameserver-apikey`, `x-reforged-admin-key` or
      `x-mysticparadox-metrics-token` is 403, whatever its value or route.
- [ ] `/content` goes to the content server only when it was not behind `/__origin`.
- [ ] Only GET, HEAD, POST, PUT, DELETE, PATCH and OPTIONS pass (405 otherwise). An upgrade must be a `GET` with
      `Upgrade: websocket`.
- [ ] Client claims are dropped and the gateway's own set: `x-forwarded-for` is the real peer, `x-forwarded-proto`
      the mode, `x-reforged-gateway` the secret; `forwarded`, `x-real-ip`, `cf-connecting-ip` and the like never
      cross; hop-by-hop headers and the headers `Connection` names never cross either way. Answers lose
      `x-powered-by` and `x-reforged-gateway` (`forward_headers`, `response_headers`).
- [ ] Upstream URLs must be `http://` on loopback with no path, query or credentials (`parse_loopback_url`), so the
      secret never leaves the machine.
- [ ] Parity: every reply in `tests/parity/gateway.requests.json` (status, headers, body, access log, allowlist feed)
      is the one Reforged's Node gateway gives (`tests/parity.rs`, recorded by `record.cjs`).

### Limits

- [ ] Bodies are capped at `GATEWAY_MAX_BODY_BYTES` (128 KiB): refused by `Content-Length` before reading and cut
      while streaming (413). A refused request's body is never read; its connection is closed instead.
- [ ] Headers: 16 KiB and 100 at most (431 / 400), read within `GATEWAY_HEADERS_TIMEOUT_MS`; a request must arrive
      within `GATEWAY_REQUEST_TIMEOUT_MS`; idle and keep-alive timeouts end slow clients (`guard.rs`).
- [ ] Connections: `GATEWAY_MAX_CONNECTIONS` in all, `GATEWAY_MAX_CONNECTIONS_PER_IP` per address (IPv6 by /64), a
      new-connection rate per address. Per-address token buckets for general, content, register, token and WebSocket
      requests; 429 with `retry-after` (`ratelimit.rs`, `tests/limits.rs`).
- [ ] Every warning about a client is throttled to once a minute per kind and address, so a client cannot flood the
      log.

### The allowlist feed

- [ ] Only a 2xx answer to `POST /account/api/oauth/token`, or to `POST /heartbeat` that carried a bearer token,
      reports the peer's address (`policy.rs`, `gateway.rs`). Nothing else does; in particular not `/game/login`
      (Dauntless Revived's route, which Paradox does not have) nor Paradox's `POST /login` (RD20).
- [ ] Known weakness, the same as the Node gateway: Paradox answers `POST /heartbeat` 200 without checking the token
      (`paradox/backend/src/routes/system.ts`), so the gateway's `has_bearer` is the only check, and any
      `Authorization: Bearer <anything>` opens the game ports for its sender. The token route does check credentials.
      Closing this needs the backend to check the bearer on `/heartbeat` (a Paradox fix, RD2) or the gateway to feed
      from a route that does.
- [ ] The feed never delays a player's request and backs off from a helper that is down (3 failures in a row, then
      2 s doubling to 60 s; `feed.rs`).
- [ ] The helper's secret is never logged.

### The access log

- [ ] One line per request with the address, method, target, route, status and sizes; tokens, keys and invite codes
      in the target are replaced and the user agent is cut (`redact.rs`). No header names or values are logged.

### The traffic capture (RD19)

- [ ] Off unless `CAPTURE_DIR` is set; when on, the start logs a warning that says so.
- [ ] Bearer tokens become `<token:subject>` (read without the signature), account keys (`RRK_`, DR's `UUK_`) become
      `<key:sha256>`, the game-server key `<gskey>`, and every field or header whose name looks secret (`key`,
      `token`, `secret`, `pass`, `auth`, `jwt`, `sig`, `cred`, `recovery`, `invite`, `cookie`) `<redacted>`; hex runs
      of 40 or more and other long tokens are cut; cookies and `x-reforged-gateway` are dropped (`capture.rs`).
- [ ] The gateway secret and the helper's secret are cut from every line, wherever an answer echoes them.
- [ ] Chat: a SASL PLAIN login keeps only its account (`<plain:account>`), `<password>` elements are cut, the rest is
      redacted as text. Compressed or oversized messages are kept by size only.
- [ ] Bodies over `CAPTURE_MAX_BODY_BYTES` (256 KiB) and binary bodies are kept by size only.
- [ ] Files are one per UTC day, created 0600 in a 0700 folder where the file system has modes, and removed after
      `CAPTURE_RETENTION_DAYS` (14). On Windows the folder inherits its parent's ACL: put it where only the host's
      account can read.
- [ ] A write error stops the capture with one warning; the gateway keeps serving.
- [ ] What is written is still personal data (account ids, addresses, play history): the capture runs only for RB2 on
      the canary, with the players told.

## Sign-off

Reviewed at commit `________` on `________` by `________`.
