<p align="center"><img src="brand/launcher/icon-512.png" width="160" alt="Dauntless Reforged Revival logo"></p>

# Dauntless Reforged Revival

Play **Dauntless 1.14.7** on a friend's private server. Dauntless Reforged Revival brings together
the [Dauntless Revived](https://github.com/mixutin/dauntless-revived) way of joining a server (one
invite, one account key, a launcher that does the rest) and the
[Mystic Paradox](https://github.com/pranav158/Mystic-Paradox) port that runs the 1.14.7 client and
its game servers.

- **Players** install the launcher, paste the invite from their host, pick a name, point the launcher
  at their own Dauntless 1.14.7 and press PLAY. → [docs/players.md](docs/players.md)
- **Hosts** run the server on a Windows PC: privately for friends over Tailscale, or publicly on the
  internet. → [docs/hosting.md](docs/hosting.md)
- **Or on a Linux VPS** (Debian 13, for example at Hostinger), publicly, with the game servers under
  Wine (experimental), set up from your Windows PC with one command. → [docs/hosting-linux.md](docs/hosting-linux.md)

The launcher speaks **English** and **Brazilian Portuguese**.

> [!IMPORTANT]
> Nothing here contains or downloads the game. Every player needs their own Dauntless 1.14.7
> (installed from their own Epic library with Legendary and the 1.14.7 manifest). Every host
> generates the game's SDK and the game data from their own copy; none of it is shared.

## How it works

```
 player's PC                                             host's server (Windows)
 ┌─────────────────────────────────────────┐            ┌────────────────────────────────────────────────┐
 │ Reforged launcher                       │  invite    │ gateway   public: TLS 443, pinned certificate  │
 │  · invite, account key (safeStorage)    │ ─────────► │           private: HTTP 61000 on Tailscale     │
 │  · checks Dauntless 1.14.7 (SHA-256)    │            │  ├─► backend    Mystic Paradox + Reforged API  │
 │  · installs the host's game runtime     │            │  ├─► content    the runtime, news, art         │
 │    (asks first, checks every byte)      │            │  └─ allowlist  opens UDP only for logged-in    │
 │  · relay 127.0.0.1:61100 ═══════════════╪═══════════►│                players (public mode)           │
 │ Dauntless 1.14.7                        │            │ Director → Ramsgate, Training Grounds, hunts   │
 │  + MysticParadox.dll + winmm.dll        │ ══ UDP ═══►│            on UDP 8770-8777                    │
 └─────────────────────────────────────────┘            │ MongoDB (replica set)                          │
                                                        └────────────────────────────────────────────────┘
```

1. The host's invite (`reforged-revival://join?...`) names the server and holds a one-time code;
   a public invite also holds the SHA-256 fingerprint of the server's certificate, and the launcher
   talks to that server only over TLS pinned to it.
2. Registering trades the code for an **account key**. The key is the game's login: the backend runs
   Mystic Paradox with `AUTH_MODE=APIKEY`, and the launcher starts the game with `-AUTH_PASSWORD=<key>`.
3. The **game runtime** (Mystic Paradox's `MysticParadox.dll` and its `winmm.dll` loader) is built by
   the host from their own SDK and served by their content server. The launcher shows its files and
   hashes and asks the player once; it checks them before every launch.
4. The runtime sends the game's calls to `REFORGED_BACKEND_URL`, which the launcher points at its
   local relay; the relay carries them to the host (pinned TLS in public mode, inside the tailnet in
   private mode) and answers the game's QoS ping. One runtime build serves every server.

## What is in this repository

| Folder | What |
|---|---|
| [`legacy/ReforgedLauncher/`](legacy/ReforgedLauncher) | The Windows launcher (Electron), a modified version of Dauntless Revived's, with the Host tools page |
| [`legacy/ReforgedGateway/`](legacy/ReforgedGateway) | The server's front door and the firewall allowlist helper (from Dauntless Revived's gateway) |
| [`legacy/ReforgedContent/`](legacy/ReforgedContent) | Serves the runtime, the news and the art pack (from Dauntless Revived's content server) |
| [`legacy/ReforgedHost/`](legacy/ReforgedHost) | The host kit: `npm run setup`, `start`, `invite`, `doctor`, `gamedata`, `publish-runtime` |
| [`upstream/`](upstream) | The pinned Mystic Paradox commit and the Reforged patches to it |
| [`brand/`](brand) | The logo and the icons |
| [`docs/`](docs) | Guides for players and hosts (Windows and Linux), the SDK and game data, and the architecture |

## Status

This is a first version. Every part has automated tests, and the whole server side was checked end
to end on Linux (MongoDB, the patched backend and Director, the content server and the gateway,
driven with the launcher's own client code: registration with an invite, the game's login, the
blocked admin routes, the runtime download, bans). **It has not yet been played with the real
1.14.7 client**: that needs Windows, the game and an SDK generated from it. Please report what you
find. Known limits are listed in [docs/hosting.md](docs/hosting.md#known-limits).

## Building from source

Node.js 20.19 or newer. Each folder is its own npm package:

```
cd legacy/ReforgedLauncher && npm ci && npm test && npm run make      # the launcher (make needs Windows)
cd legacy/ReforgedGateway  && npm ci && npm test
cd legacy/ReforgedContent  && npm ci && npm test
cd legacy/ReforgedHost     && npm ci && npm test
```

Publishing a GitHub release builds the launcher installer and attaches it to the release; see
[legacy/ReforgedLauncher/README.md](legacy/ReforgedLauncher/README.md#releases).

## Credits and license

Dauntless Reforged Revival stands on the work of
[Dauntless Revived](https://github.com/mixutin/dauntless-revived) (mixutin and contributors),
[Mystic Paradox](https://github.com/pranav158/Mystic-Paradox) (Pranav Karande) and
[Undaunted](https://github.com/SyST3MDeV/Undaunted) (gwog and contributors). The launcher's Credits
page names everyone.

Free software under the GNU AGPL-3.0 (AGPL-3.0-only): [LICENSE.txt](LICENSE.txt). Additional terms
apply to the Mystic Paradox material: [ADDITIONAL_TERMS.md](ADDITIONAL_TERMS.md). Mystic Paradox's
Dauntless 1.12.0 port and related modifications were developed by Pranav Karande. See NOTICE.md for
contribution and provenance information: [NOTICE.md](NOTICE.md).

An unofficial fan project, not affiliated with or endorsed by Phoenix Labs, Epic Games, Dauntless
Revived or Mystic Paradox. Dauntless is a trademark of its owners.
