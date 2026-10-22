# Architecture and security model

How the parts fit, what each one trusts, and the contracts between them. For setting up a server
see [hosting.md](hosting.md); for the per-component details, each folder's README.

## The parts

| Part | Runs on | From |
|---|---|---|
| Launcher (`legacy/ReforgedLauncher/`) | each player's PC | Dauntless Revived's launcher, modified |
| Game runtime (`MysticParadox.dll`, `winmm.dll`) | next to each game executable | Mystic Paradox's ParadoxRuntime and RuntimeLoader, patched; built by each host |
| Gateway (`legacy/ReforgedGateway/`) | the server | Dauntless Revived's gateway, modified |
| Allowlist helper (`legacy/ReforgedGateway/src/allowlist/`) | the server, as administrator (public mode) | Dauntless Revived |
| Backend (`ParadoxBackend`) | the server, 127.0.0.1:61000 | Mystic Paradox, patched |
| Director (`ParadoxDirector`) | the server, 127.0.0.1:61001 | Mystic Paradox, patched |
| Game servers (the game exe with `-server`) | the server, UDP 8770-8777 | started by the Director |
| Content server (`legacy/ReforgedContent/`) | the server, 127.0.0.1:61002 | based on Dauntless Revived's |
| MongoDB | the server, 127.0.0.1:27017, replica set `rs0` | |
| Host kit (`legacy/ReforgedHost/`) | the server | this project |

## A session, step by step

1. **Invite.** `reforged-revival://join?v=1&host=<tailnet IP>&port=61000&code=..&name=..` (private)
   or `...v=2&mode=public&host=..&port=443&fp=<SHA-256 of the certificate>&code=..&name=..`
   (public). The parser (`legacy/ReforgedLauncher/src/shared/invite.ts`) accepts nothing else: a v1 invite
   must point into the tailnet (or this PC), a v2 one must carry a fingerprint.
2. **Status.** `GET /reforged/api/ServerStatus` through the gateway: name, registration mode,
   version and source link, the game build and the game-data state for everyone; players and
   instances only with a registered key.
3. **Register.** `POST /reforged/api/Register {Username, InviteCode}`; in one MongoDB transaction
   the code is used up, the account created and the key's HMAC stored. The answer is the key
   `RRK_<48 hex>`, which the launcher stores with Electron `safeStorage` (DPAPI), tied to the
   server's certificate fingerprint in public mode.
4. **Game.** The launcher finds the install (Legendary, Heroic, Epic, or a folder) and compares the
   executable's SHA-256 with the 1.14.7 build's; again before every launch.
5. **Runtime.** `GET /content/v1/runtime/manifest` lists exactly `MysticParadox.dll` and
   `winmm.dll` with sizes and SHA-256. The player accepts that set once per server (the set's id is
   stored); then each file is downloaded with the key, checked, and only then moved into place.
   A different set means asking again.
6. **Play.** The launcher starts its relay on 127.0.0.1:61100, writes Engine.ini's XMPP section
   (`ws://127.0.0.1:61100`), removes Dauntless Revived's `[OnlineSubsystemPhoenix]` endpoints from
   Game.ini (they would send the game's own requests to `127.0.0.1:61000`, outside the runtime's
   redirect), and starts the game with the 1.14.7 command line
   (`-AUTH_TYPE=exchangecode -AUTH_LOGIN=<userId> -AUTH_PASSWORD=<key> -epicapp=Archon ...`) and
   `REFORGED_BACKEND_URL=http://127.0.0.1:61100`. The runtime rewrites every official Dauntless URL
   to `<that address>/__origin/<host><path>`; the relay carries them, and the chat WebSocket, to the
   gateway, and answers `/QoS` itself.
7. **Login.** `POST /account/api/oauth/token` with the key as the exchange code: the backend
   (`AUTH_MODE=APIKEY`) looks up the key's HMAC and issues the game's session token, unless the
   account is banned or disabled.
8. **Worlds.** The backend asks the Director for a server; the Director starts the game exe with
   `-server` on a UDP port and the backend tells the client where. In public mode the gateway has
   already told the allowlist helper this player's address (after the 2xx login or heartbeat), and
   the firewall rule lets it reach the game ports.

## Trust boundaries

- **The gateway is the only door.** Backend, Director, content server and MongoDB listen on
  loopback. The gateway refuses the host-only routes (`/admin`, `/launcher`, `/internal`,
  `/health`, the `/reforged/api` admin routes), the host-only headers (the game-server key, the
  admin key, the metrics token), dot segments, encoded separators and malformed `/__origin/`
  prefixes, before anything reaches the backend; it rate-limits per address and caps bodies.
- **Admin routes** (invites, account status) need the admin key *and* a request from 127.0.0.1
  without the gateway's header or `X-Forwarded-For`, compared in constant time. The kit's
  `invite` command is the only intended caller.
- **Public mode TLS.** The certificate is self-signed; the launcher pins its SHA-256 from the
  invite and checks it on `secureConnect`, before any byte is sent. A mismatch stops everything
  (the key never leaves). A key is only ever sent to the server and certificate it was made for.
- **Private mode** relies on Tailscale for reachability and encryption; the gateway refuses to
  listen in plain HTTP anywhere but a Tailscale or loopback address, and the launcher refuses plain
  HTTP to any other address.
- **The runtime is native code from the host.** It runs in the player's game with the player's
  rights. The launcher never installs it without the player's explicit consent to that exact set
  of hashes, and the host builds it from public source. Players should only accept a runtime from a
  host they trust; the launcher says so.
- **Secrets** live in `.workspace\config\secrets.env` (owner-only), are made once, and are never
  logged. The launcher redacts keys from its logs, the gateway redacts tokens and keys from its
  access log, and the content server logs account ids, never keys.
- **Game ports** (public mode) are closed by default and opened per address for 10 minutes after a
  successful login or heartbeat; the rule is disabled when the helper stops.

## Contracts

| Route | Who | Notes |
|---|---|---|
| `POST /reforged/api/Register` | launcher | `{Username, InviteCode}` → `{UUK: "RRK_…"}`; 400 `username_invalid`/`registration_closed`, 401 invite refused, 409 taken |
| `GET /reforged/api/GetUserInfo` | launcher, content server | key header → `{UserId, Username, IsAdmin}`; 401; 403 `{error: account_banned, reason}` |
| `GET /reforged/api/ServerStatus` | launcher | see `legacy/ReforgedLauncher/src/shared/status.ts` |
| `GET /reforged/api/RegistrationStatus` | launcher | `{mode}` |
| `POST /reforged/api/CreateInvite` etc. | host kit, this PC only | admin key |
| `GET /content/v1/runtime/manifest`, `/files/<name>` | launcher | see `legacy/ReforgedContent/README.md` |
| `GET /content/v1/news`, `/branding` | launcher | as in Dauntless Revived |
| `/__origin/<host>/...` | the game, through the relay | the backend unwraps the prefix |
| `/QoS` | the game | answered by the relay |

Headers: `x-reforged-user-api-key` (account key), `x-reforged-admin-key` (admin),
`x-reforged-gateway` (set by the gateway), `x-mysticparadox-gameserver-apikey` (game servers,
loopback only).
