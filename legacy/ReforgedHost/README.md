# ReforgedHost: the host kit

Everything a host runs from a terminal, in the `legacy/ReforgedHost/` folder. It plays the part of
Dauntless Revived's server kit for Dauntless 1.14.7: it builds the backend and Director of
[Mystic Paradox](https://github.com/pranav158/Mystic-Paradox), forked into this repository at
[`../../paradox/`](../../paradox) (the runtime is in [`../../native/`](../../native)), writes the whole server
configuration (secrets included), runs every service, and makes invites. Nothing is downloaded from Mystic
Paradox's repository. No dependencies of its own: Node.js 20.19+ and Git.

Read [docs/hosting.md](../../docs/hosting.md) first; it walks through a whole setup in order. The
SDK, the runtime build and the game data are in [docs/sdk-and-game-data.md](../../docs/sdk-and-game-data.md).

## Commands

Run them in this folder (`cd legacy/ReforgedHost` from the repository root, then `npm install` once).

| Command | What it does |
|---|---|
| `npm run setup -- --mode private --name "Friday Hunts" --game "D:\Games\Dauntless"` | Private server, for friends over Tailscale. The host address is this PC's Tailscale IP (found automatically, or `--host 100.x.y.z`) |
| `npm run setup -- --mode public --host 203.0.113.7 --name "Friday Hunts" --game "D:\Games\Dauntless"` | Public server on the internet. A DNS name works too, with `--game-address <public IPv4>` |
| `npm run start` | Starts everything (MongoDB if managed, backend, Director, content server, allowlist helper in public mode, gateway). Ctrl+C stops it all, the game servers included |
| `npm run invite -- --for "Alex"` | A one-use invite link (the server must be running). `--uses 5`, `--unlimited`, `--list` |
| `npm run publish-runtime` | Publishes the built `MysticParadox.dll` and `winmm.dll` for players, and installs them next to the server's game executable. `-- --from <folder>` takes both from that folder instead (a Linux server's runtime, built on Windows) |
| `npm run gamedata` | Generates the game data from your CatalogExporter export (the progression config from the progression export; `-- --example-sources` uses the examples for the files no export produced; `-- --progression` rebuilds only the progression config; `-- --export [file]` saves it all into one game data pack and `-- --import <file>` puts a pack back; `-- --examples` copies every example, for a start-up test only) |
| `npm run preset` | The project's server preset in `../../preset/`: `-- --create [--generate] [--name "<name>"]` extracts it from your game data (with its provenance), `-- --apply [--force]` applies it now; `setup` applies it on its own where game data is missing or still the example |
| `npm run multipliers` | Private test servers only: `-- --xp 1-100 --rewards 1-100` multiplies the XP and the hunt loot the game servers grant, `-- --off` turns it off, no arguments shows what is set; restart the server after a change (see `../../docs/paradox-patches.md`, patch 0007) |
| `npm run update` | Compares this project folder with GitHub, fast-forwards it when there is an update (never over your own changes to its files; `.workspace` is not touched) and runs `npm run setup` again; `-- --check` only compares; it says when the game runtime has to be built and published again |
| `npm run doctor` | Checks everything and says what to do next (`-- --no-hash` skips hashing the game executable) |

Other `setup` options: `--registration INVITECODE|OPEN|NONE` (default invite codes only),
`--share <Tailscale share link>` (private: put it in every invite), `--mongo-uri <uri>` with
`--no-manage-mongo` (your own MongoDB, which must be a replica set), `--mongod <path>`,
`--max-hunts 1-10` (default 3; each hunt is a whole game process; above 6 the game ports reach below 8770, one
per hunt: 8766-8777 at 10), `--public-port <port>` (public: the
gateway's port when 443 is taken, by a hosting panel for example; it goes into every invite), `--skip-install`.
Running `setup` again with other options changes only those.

CPU and RAM (`host.json` `performance`, written into `director.env`; see `docs/hosting-linux.md#cpu-and-ram`):
`--server-fps 0|10-240` (default 30: each game server's frame cap, `REFORGED_SERVER_MAX_FPS`, runtime patch
0020), `--idle-fps 0|1-240` (default 10: after 15 s without players, `REFORGED_SERVER_IDLE_FPS`),
`--training-on-demand` / `--training-at-boot` (default on demand: `TRAINING_ON_DEMAND`, Director patch 0021),
`--training-idle-minutes 0-1440` (default 5: on demand, they stop again after that long without players,
`TRAINING_IDLE_STOP_MINUTES`, Director patch 0033; 0 keeps them running),
`--quiet-game-logs` / `--verbose-game-logs` (default quiet: `GAMESERVER_LOG_CMDS`), `--memory-guard` /
`--no-memory-guard` with `--hunt-ram <MB>` (default 1536) and `--min-free-ram <MB>` (default 1024): a new hunt
starts only while that much memory is free (`GAMESERVER_MEMORY_GUARD`, Director patch 0022). A managed MongoDB gets a
1 GB WiredTiger cache. `npm run doctor` says whether the published runtime has the CPU patches (0019, 0020).

Linux (`docs/hosting-linux.md`): `--wine <path>`, `--proton <script>` (`--no-proton`), `--no-xvfb`
(`--xvfb`), `--firewall-interface <eth0>` (`--no-firewall-interface`). Setup and `start` refuse to run
as root there.

## On Linux

The game servers are still the Windows executable. Setup writes `.workspace/bin/reforged-gameserver`,
which the Director starts instead of it (`GAMESERVER_BINARY_PATH`): it runs the executable through
Wine (or Proton) on `xvfb-run`, with `WINEDLLOVERRIDES=winmm=n,b` so the game loads Mystic Paradox's
loader, in the prefix `.workspace/wine` (setup makes it once). A stop signal for a game server goes to
its whole process group (Wine, the game, Xvfb). See `src/wine.ts`, adapted from Dauntless Revived's
Linux port.

`linux/setup-vps.sh` (as root on a Debian 13 VPS: packages, MongoDB, the server's user, the project,
setup; `--finish` places the game, publishes the runtime, imports the game data, starts the units and
makes an invite) and `linux/Deploy-FromWindows.ps1` (runs both over SSH from the Windows PC that has
the game, and sends the game) do the whole of `docs/hosting-linux.md`.

Setup also writes, in `.workspace/config/`:

- `firewall.sh` instead of `firewall.ps1`: lets an active ufw pass TCP 443 and UDP 8770-8777.
- `systemd/`: `reforged-allowlist.service` (public: the allowlist helper with `CAP_NET_ADMIN` only,
  `ALLOWLIST_FIREWALL=nftables`), `reforged.service` (`start`, with `CAP_NET_BIND_SERVICE` only in
  public mode, and `REFORGED_ALLOWLIST=external` so it waits for the other unit instead of starting
  the helper) and `install.sh`. Both run as the user who ran setup.

## What setup does

1. **Mystic Paradox** is already in the checkout: `paradox/backend`, `paradox/director` and `native/`.
   Your SDK, game data, exports and builds go next to the code that uses them, in files git ignores
   (`native/paradox-runtime/SDK*`, `paradox/backend/game-data/*.json`, `paradox/Items_Analysis/`, ...).
   A host set up before Mystic Paradox was merged in had them in `.workspace/Mystic-Paradox/`: setup copies
   them over once, where nothing is in the new place yet, and leaves that folder for you to delete.
2. `npm ci` and `npm run build` in paradox/backend, paradox/director, ReforgedGateway and ReforgedContent.
3. **Configuration** in `.workspace/config/`:
   - `host.json`: what you chose.
   - `secrets.env`: made once and never changed (the backend's token signing key, the API-key HMAC
     secret, the game-server key, the admin key, the gateway and allowlist secrets, the metrics token).
     **Never share it.** Changing it logs everyone out or breaks the invites already sent.
   - `backend.env`, `director.env`, `content.env`, `gateway.env`, `allowlist.env` (public mode):
     written from the two files above every time setup runs.
   - `firewall.ps1`: the Windows Firewall rules to add once, as administrator (Linux: `firewall.sh`
     and `systemd/`, see above).
4. **Public mode**: the gateway's self-signed certificate in `.workspace/tls/` (made once; a new one
   changes the fingerprint in every invite). The fingerprint is printed.

## How the server is laid out

```
 player's launcher ─ relay 127.0.0.1:61100 ─► gateway ─► backend   127.0.0.1:61000  (Mystic Paradox, AUTH_MODE=APIKEY)
                                              │        └► content   127.0.0.1:61002  (the runtime, news, art)
                                              └ public: 0.0.0.0:443 TLS   private: <Tailscale IP>:61000 HTTP
 Director 127.0.0.1:61001 ─► game servers on UDP 8770-8777 (Ramsgate 8777, Training Grounds 8776, hunts 8770-8775; down to 8766 with --max-hunts 10)
 MongoDB 127.0.0.1:27017 (replica set rs0)
```

- The backend runs with `NODE_ENV=production`, plain HTTP on loopback behind the gateway
  (`REFORGED_HTTP_ONLY=true`), and `AUTH_MODE=APIKEY`: the account key from an invite is the game's
  login. Production mode needs a MongoDB **replica set**; `start` starts `mongod --replSet rs0` and
  initiates the one-member set the first time. With your own MongoDB, enable
  `replication.replSetName: rs0` in its `mongod.cfg` and run `rs.initiate()` once.
- The Director's game servers inherit `REFORGED_BACKEND_URL=http://127.0.0.1:61000`, so the same
  runtime build talks to the backend directly. They run from the game folder you gave `--game`
  and announce `--game-address` (`MY_IP`).
- The launcher answers the game's QoS ping itself (`QOS_TARGET_URL=http://127.0.0.1:61100/QoS`).
- Logs: `.workspace/logs/` (one per service, the game servers in `gameservers/`).

## Tests

```
npm test    # env files, host.json, secrets, the env files of both modes, Mystic Paradox in the
            # checkout and the move from the old .workspace/Mystic-Paradox, invites (parsed with
            # the launcher's own parser), runtime publishing (read back with the content server's own parser), game data, the supervisor
```

## License

AGPL-3.0-only.
