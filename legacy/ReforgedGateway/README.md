# Dauntless Reforged Revival gateway

The front door of a Dauntless Reforged Revival server, in the `legacy/ReforgedGateway/` folder. It is a
modified version of Dauntless Revived's `UndauntedGateway` (same design, same limits, same
allowlist helper), with the routing rules of the Mystic Paradox 1.14.7 backend and a plain-HTTP
mode for private servers. Two small processes:

- **The gateway** (`dist/server.js`) is the server's only listener that players reach.
  - **Public mode** (default): TLS with a self-signed certificate, and the launcher pins that
    certificate's fingerprint (the `fp` of a v2 invite). Usually on `0.0.0.0:443`.
  - **Private mode** (`GATEWAY_TLS=0`): plain HTTP on the host PC's Tailscale address only (port
    61000, the port in v1 invites). Tailscale encrypts the traffic and decides who can reach it.
    The gateway refuses to start in this mode on any other address.
- **The allowlist helper** (`dist/allowlist/server.js`, public mode only) runs with administrator
  rights. It keeps one Windows Firewall rule that opens the game servers' UDP ports only to
  addresses whose owners logged in through the gateway.

```
 player's PC                                      the server
 game ─ http ─► launcher relay ══ TLS 443 (pinned) ══► gateway ──► 127.0.0.1:61000  backend (API + chat)
       127.0.0.1:61100    or HTTP 61000 in the tailnet    │    └──► 127.0.0.1:61002  content server (/content/*)
                                                          └─ "this address logged in" ──► 127.0.0.1:61005 allowlist helper
 game ════════════════════ UDP 8770-8777 ═════════════════════════════════► game servers (Ramsgate, Training Grounds, hunts)
```

The Director (61001) is never reachable through the gateway: no route leads to it.

## Routing

| Request | Goes to |
|---|---|
| `/content` and `/content/*` | the content server (`GATEWAY_CONTENT_URL`) |
| any WebSocket upgrade (`GET` with `Upgrade: websocket`) | the backend's chat listener (`GATEWAY_WS_URL`, the backend itself). The game asks for `//` with the protocol `xmpp` |
| everything else | the backend (`GATEWAY_BACKEND_URL`) |

The request target goes upstream unchanged: method, path, query and headers (minus the ones below).
Bodies are streamed. The account key header `x-reforged-user-api-key` passes through (the launcher
sends it with `ServerStatus`, `GetUserInfo` and the runtime downloads) and is never logged.

### The 1.14.7 runtime's `/__origin/` paths

The Mystic Paradox runtime rewrites every official Dauntless URL the game calls to
`<backend>/__origin/<official host>/<path>`, and the backend unwraps that prefix before routing.
The gateway unwraps it the same way and applies every rule below to the inner path, so
`/__origin/gamedev-prod.steelyard.ca/reforged/api/CreateInvite` is refused exactly like
`/reforged/api/CreateInvite`. A malformed prefix (no host, a host with `:` `@` `%` or dot
tricks, a second `/__origin` inside) is a `400`.

### What it refuses

| Answer | When |
|---|---|
| `403 {"error":"forbidden"}` | a header only the host's own processes send, whatever its value: `x-mysticparadox-gameserver-apikey` (the dedicated game servers talk to the backend directly), `x-reforged-admin-key`, `x-mysticparadox-metrics-token` |
| `403 {"error":"forbidden"}` | any `/reforged/api/*` route except `POST Register`, `GET GetUserInfo`, `GET ServerStatus` and `GET RegistrationStatus` (and `HEAD` of the three GETs): invites and account status are admin routes |
| `403 {"error":"forbidden"}` | the host-only parts of the backend: `/admin/*` (Mystic Paradox's admin dashboard API), `/launcher/*` (Mystic Paradox's own launcher API: email and Discord accounts, game sessions, its update channel; a Reforged server registers players through invites only), `/internal/*` (metrics), `/health/*` |
| `400 {"error":"bad_path"}` | `.` or `..` segments, backslashes, control characters or whitespace, percent-encoded `/` `\` `.` or NUL, broken percent-encoding, a malformed `/__origin/` prefix |
| `400 {"error":"bad_request"}` | a request target that is not a plain path, or an upgrade that is not a WebSocket `GET` |
| `405 {"error":"method_not_allowed"}` | methods other than GET, HEAD, POST, PUT, DELETE, PATCH, OPTIONS |
| `413 {"error":"body_too_large"}` | a body over `GATEWAY_MAX_BODY_BYTES` (128 KiB; the backend's own JSON limit is 100 KB) |
| `429 {"error":"rate_limited"}` + `Retry-After` | the address's bucket for this kind of request is empty |
| `408`, `431`, `400` | headers or body too slow, headers too large (16 KB), malformed request |
| `502`, `504` | the upstream is down, or went quiet for 120 s |

The checks match regardless of letter case, trailing or repeated slashes and percent-encoding (at
least as lenient as Express's routing). The tests read every `/reforged/api` route from the
backend patch (`upstream/patches/0002-backend-reforged-launcher-api.patch`) and try each one
through the gateway, so a route added there later is blocked unless it is added to
`PUBLIC_REFORGED_API` in `src/policy.ts`.

### Headers it sets

- `X-Forwarded-For: <peer address>`, **overwritten**, never appended to. The client's own
  `X-Forwarded-*`, `Forwarded`, `X-Real-IP` and similar are dropped. The backend uses it for its
  per-address rate limits (it trusts it from loopback only).
- `X-Forwarded-Proto: https` (public mode) or `http` (private mode).
- `X-Reforged-Gateway: <GATEWAY_SECRET>`; a client-supplied one is dropped. The backend never
  treats a request that carries it as a local admin request.
- Hop-by-hop headers and `Expect` are not forwarded; `X-Powered-By` is dropped on the way back.

## The allowlist feed (public mode)

When the backend answers **2xx** to `POST /heartbeat` with a bearer token, or to
`POST /account/api/oauth/token` (which succeeds only with a valid account key, `AUTH_MODE=APIKEY`),
the gateway sends the peer address to the helper (`POST /allow`), at most once per
`GATEWAY_ALLOWLIST_REFRESH_SECONDS` (60 s). The helper keeps an address for 10 minutes. The
player's request never waits for the helper. In private mode run the gateway with
`GATEWAY_ALLOWLIST=0`: only tailnet members can reach the game ports anyway.

The gateway listens on IPv4 so that the address it reports is the one the game's UDP traffic comes
from: the Director announces game servers on the server's IPv4 (`MY_IP`).

## Limits

Dauntless Revived's, sized from real sessions (its README has the measurements); the game's
request pattern is the same on 1.14.7.

| Bucket | Requests | burst, per minute |
|---|---|---|
| general | everything not below | 300, 180 |
| content | `/content/*` | 600, 600 |
| register | `POST /reforged/api/Register` | 5, 0.2 |
| token | `POST /account/api/oauth/token` | 10, 1 |
| ws | WebSocket upgrades | 20, 12 |
| connect | new TCP connections | 200, 300 |

Per address: 128 open connections; 2,048 in total. Timeouts: TLS handshake 10 s, headers 10 s,
whole request 30 s, keep-alive 65 s, idle connection 120 s, silent upstream 120 s, idle WebSocket
5 min. Every value can be changed in `.env` (see [`.env.example`](.env.example)).

## Access log

One JSON line per request on stdout (warnings and errors on stderr), with `ip`, `method`,
`target`, `route` (`backend`, `content`, `ws` or `gateway` with a `reason`), `status`, byte counts
and time. **Never logged:** headers (except the user agent), bodies, secrets. In the target, JWTs
become `<token>`, account keys `<key>`, long hex or token-like strings `<redacted>`, and so do
query parameters named like key, token, secret, pass, auth, code or session.

## The allowlist helper

HTTP on `127.0.0.1:61005` only, every request with the header `x-allowlist-secret`. `POST /allow
{"ip": ...}` adds an address for 10 minutes; `GET /status` shows the live set. It keeps one
Windows Firewall rule, `ReforgedRevival-GamePorts-Allowlist` ("Dauntless Reforged Revival game
ports (allowlist)"), allowing inbound UDP on `ALLOWLIST_PORTS` (8770-8777) from the live set, and
disables it when the set is empty or the helper stops. Changes are debounced, retried with
backoff, saved across restarts and written to an audit log. `ALLOWLIST_DRY_RUN=1` never touches
the firewall and writes the exact PowerShell it would run to the audit log instead; every test
uses it. `node dist/allowlist/server.js --close-ports` disables the rule once (for stop scripts).

On Linux, `ALLOWLIST_FIREWALL=nftables` (the default is `windows`) keeps one nftables table instead,
`ALLOWLIST_NFT_TABLE` (`reforged_revival`), adapted from Dauntless Revived's Linux port: an input
chain at priority -5 that accepts UDP on `ALLOWLIST_PORTS` from the live set (two sets, IPv4 and
IPv6), drops the rest of it, and never filters loopback; `ALLOWLIST_INTERFACE` limits it to one
interface. Every change replaces the whole table in one `nft -f` transaction, and an empty set (or
`--close-ports`) leaves the ports closed, since Linux has no default-deny firewall. `nft` is found at
a fixed path (`/usr/sbin/nft`, or `ALLOWLIST_NFT`), never in PATH, and real mode refuses to start
without root or `CAP_NET_ADMIN`. `REFORGED_TEST_NFT=1 npm test` (Linux, as root) also applies the
scripts to a test table of its own.

What else must be true on a public server: Windows Firewall on with default inbound **Block**, no
other rule allowing UDP 8770-8777 or the game exe, inbound TCP 443 open for the gateway, nothing
open for 61000-61005 (they listen on loopback), and the backend with `AUTH_MODE=APIKEY`.

## The certificate

```
node tools/make-cert.js --host 203.0.113.7 [--host play.example.org] --out C:\ReforgedServer\tls
node tools/make-cert.js --fingerprint C:\ReforgedServer\tls\gateway-cert.pem [--json]
```

RSA-2048, SHA-256, self-signed, valid for 10 years, each `--host` as a subject alternative name.
It writes `gateway-cert.pem` and `gateway-key.pem` and prints the SHA-256 fingerprint of the
certificate (the invite's `fp=`). It refuses to overwrite them without `--force`: a new
certificate changes the fingerprint and every invite already sent stops working (players keep
their accounts; the launcher asks them to confirm the new certificate). `ReforgedHost`'s setup
runs this for you in public mode.

## Configuration

All settings, with their defaults, are in [`.env.example`](.env.example). Required:

| Variable | Process | Meaning |
|---|---|---|
| `GATEWAY_CERT`, `GATEWAY_KEY` | gateway (public mode) | the PEM files from `make-cert.js` |
| `GATEWAY_TLS=0`, `GATEWAY_BIND=<tailnet IP>` | gateway (private mode) | plain HTTP inside the tailnet |
| `GATEWAY_SECRET` | gateway | 32-256 printable characters |
| `ALLOWLIST_SECRET` | both | shared by the gateway and the helper (`GATEWAY_ALLOWLIST=0` runs without the feed) |
| `ALLOWLIST_DRY_RUN` | helper | `1` or `0`, no default |

Upstream and helper URLs must be `http://` on 127.0.0.0/8 or `::1`.

## Development

```
npm ci
npm run build          # dist/
npm test               # build/ (git-ignored), about 100 tests, about 20 s
```

The tests listen on 127.0.0.1 ports 62400-62442 only, and use other loopback source addresses
(127.0.0.2-127.0.0.42) to act as different players. The helper always runs in DRY-RUN in the tests:
**they never change the firewall of the machine they run on.**

When a client pins a certificate it must check the fingerprint on `secureConnect`, before any
request byte is sent: with `rejectUnauthorized: false` Node never calls `checkServerIdentity` for a
self-signed certificate. The launcher's `src/main/pinned.ts` and this package's test client
(`test/helpers.ts`) both check on `secureConnect`.

## License

AGPL-3.0-only. A modified version of Dauntless Revived's `UndauntedGateway` by mixutin and the
Dauntless Revived contributors.
