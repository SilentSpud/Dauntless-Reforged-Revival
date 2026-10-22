# Hosting a Dauntless Reforged Revival server

The server runs on **one Windows 10 or 11 PC** (x64) that also has Dauntless 1.14.7: the
Director starts the game servers (Ramsgate, the Training Grounds and the hunts) from the same game
build the players use. You can play on that PC too.

Two kinds of server, as in Dauntless Revived:

| | Private | Public |
|---|---|---|
| Who can join | people you share your PC with on [Tailscale](https://tailscale.com) | anyone with an invite |
| Invite | `v=1`, your Tailscale IP, port 61000 | `v=2`, your public IP or name, port 443, the certificate fingerprint |
| Network | nothing to open on your router | TCP 443 and UDP 8770-8777 forwarded to the PC |
| Encryption | Tailscale | TLS pinned to your self-signed certificate |
| Game ports | open to your tailnet | opened per player who logged in (firewall allowlist) |

Start private: it needs no router changes and nothing is exposed to the internet.

A public server can also run on a Linux VPS (Debian 13), with the game servers under Wine: see
[hosting-linux.md](hosting-linux.md). It still needs this guide's Windows steps once, on a PC with
your game, for the runtime and the game data.

## What you need

- Windows 10/11 x64, about 20 GB free for the game plus a few GB for the tools.
- Dauntless 1.14.7, installed as in [players.md](players.md#1-install-dauntless-1147).
- [Node.js](https://nodejs.org) 20.19 or newer, [Git](https://git-scm.com).
- [MongoDB Community Server](https://www.mongodb.com/try/download/community) 7 or newer. The host kit
  starts it itself, as a one-member replica set: in the installer, untick **Install MongoD as a
  Service** (an installed service holds port 27017; stop it with `Stop-Service MongoDB` and
  `Set-Service MongoDB -StartupType Disabled` as administrator).
- To build the runtime: [Visual Studio](https://visualstudio.microsoft.com/downloads/) 2022 or newer
  with the **Desktop development with C++** workload, and [Rust](https://www.rust-lang.org/tools/install).
- [Dumper-7](https://github.com/Encryqed/Dumper-7), built from source (to generate the SDK).
- Private mode: [Tailscale](https://tailscale.com/download) on this PC.
- The Dauntless Reforged Revival Launcher, for its Host tools page (optional but easier).

## 1. Get the project

```powershell
git clone https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival D:\Reforged
cd D:\Reforged\legacy\ReforgedHost
npm install
```

With the launcher, this happens by itself: when Host tools are turned on (and at every start while
no project folder is chosen), it looks for a `Reforged` folder beside the game's folder and, when it is
not there, clones the project into it with Git (a game in `D:\Games\D1147\Dauntless` gets
`D:\Games\D1147\Reforged`). A `Reforged` folder that holds other files is never written into.

## 2. Set up the server

Private (finds your Tailscale IP itself):

```powershell
npm run setup -- --mode private --name "Friday Hunts" --game "D:\Games\Dauntless"
```

Public:

```powershell
npm run setup -- --mode public --host 203.0.113.7 --name "Friday Hunts" --game "D:\Games\Dauntless"
# with a DNS name, also say which public IPv4 the game servers announce:
npm run setup -- --mode public --host hunts.example.net --game-address 203.0.113.7 --name "Friday Hunts" --game "D:\Games\Dauntless"
```

Setup builds Mystic Paradox's backend and Director from `paradox\` (they are part of the project; nothing
is downloaded from Mystic Paradox's repository), installs and builds the other services, makes the secrets and writes the
configuration in `.workspace\config\` (see [legacy/ReforgedHost/README.md](../legacy/ReforgedHost/README.md)).
In public mode it also makes the gateway certificate and prints its fingerprint.

Then, once, in PowerShell **as administrator**, add the firewall rules it wrote:

```powershell
powershell -ExecutionPolicy Bypass -File D:\Reforged\.workspace\config\firewall.ps1
```

Public mode: forward **TCP 443** and **UDP 8770-8777** on your router to this PC. Ports 61000-61005
and MongoDB stay on loopback and must never be forwarded.

## 3. The SDK, the runtime and the game data

These come from your own game and are never shared. Follow
[sdk-and-game-data.md](sdk-and-game-data.md). The short version, with the launcher's Host tools
page (Settings > Launcher > Advanced > Host tools), choosing `D:\Reforged` as the workspace:

1. **Build** tab: dump the SDK with Dumper-7, then **Import SDK**.
2. **Build** tab: build MysticParadox.dll, winmm.dll and CatalogExporter.dll.
3. **Game data** tab: **Use example data** for a first start-up test only; for a playable server, play
   on your own server, inject CatalogExporter, then **Generate game data** (or `npm run gamedata`). Save the result
   with `npm run gamedata -- --export`; on your next setup, `npm run gamedata -- --import <pack>`
   (or **Import game data pack**) replaces this step. When the project carries a server preset
   (`preset/`, `npm run preset -- --create`), `npm run setup` already fills the game data from it.
4. `npm run publish-runtime`: the runtime for players, and next to the server's game executable.

## The launcher's Host page, without a terminal

Everything below can also be done from the launcher's Host page (Settings > Launcher > Advanced >
Host tools), without opening a terminal. Its tabs are in the order a new host goes through them:
Requirements, Build, Game data, Server. Importing the SDK, building winmm.dll and the game data
steps work on the project's own `native\` and `paradox\` folders, so they can be done before the server
is set up; **Choose the SDK folder** imports a Dumper-7 output that is
not under `C:\Dumper-7`.

- **Requirements** tab: what this PC needs (Git, Node.js, MongoDB, Tailscale, Visual Studio, Rust,
  Dumper-7, the Visual C++ runtime), each one detected, with its download and the install steps that
  avoid the usual mistakes (MongoDB's Windows service, Visual Studio without its C++ workload, an old
  Node.js). Close and reopen the launcher after installing something, so it finds it.
- **Game data** tab: **Generate game data** copies CatalogExporter's export from beside the game and
  runs `npm run gamedata`; **Save pack** runs `npm run gamedata -- --export`.
- **Server** tab: **Start server** and **Stop server** run `npm run start` and stop it the way Ctrl+C
  does; **Create invite** runs `npm run invite`. **Test multipliers** (private servers only) takes the
  XP and reward multipliers, 1 to 100, and runs `npm run multipliers`; a running server restarts by
  itself to use them. **Update setup** runs `npm run update`: it compares the project folder with
  GitHub and, when there is an update, downloads it, prepares the server again and restarts it if it
  was running (it says so when the game runtime changed and has to be built and published again).
  **Prepare server** runs `npm run setup` with the server's name and kind and the launcher's game
  folder (**Apply changes** runs it again). The server's output is under **Show details**, and its
  logs are in `.workspace\logs\` (`launcher-*.log`). The server stops when the launcher closes.

The launcher runs the host kit with the Node.js installed on this PC, and builds it first when its
sources changed. A server started in a terminal shows as running there: stop it with Ctrl+C before
starting it from the launcher. A public server needs the launcher run as administrator, as the
terminal would be.

## 4. Start it

```powershell
npm run start          # public mode: in a terminal opened as administrator
```

It starts MongoDB (and initiates its replica set the first time), the backend, the Director (which
starts Ramsgate and the Training Grounds), the content server, the allowlist helper (public) and the
gateway, and restarts any that stop. Logs are in `.workspace\logs\`. **Ctrl+C** stops everything,
the game servers included. `npm run doctor` checks every part and tells you what is missing.

## 5. Invite your friends

In a second terminal:

```powershell
npm run invite -- --for "Alex"
```

Send the whole line it prints, one invite per friend (each code works once; `--uses 5` or
`--unlimited` for more). Private server: also share this PC with them in Tailscale
(<https://login.tailscale.com/admin/machines> > your PC > Share), and they accept the share. With
`npm run setup -- --share <share link>` the link goes into every invite and the launcher shows a
button for it.

## Running it

- **Test multipliers** (private servers only): `npm run multipliers -- --xp 5 --rewards 5`, then restart the
  server, multiplies the XP and the hunt loot (Crowns, materials, part breaks, gathering, patrol chests, quest
  Crowns) so you can reach late-game content quickly while testing. `npm run multipliers -- --off` turns them off,
  and `npm run multipliers` alone says what is set. A public server refuses them. Details: the 0007 patch in
  [paradox-patches.md](paradox-patches.md#0007-xp-and-reward-multipliers-for-private-test-servers).
- **Bans**: from this PC only, `POST http://127.0.0.1:61000/reforged/api/SetAccountStatus` with the
  header `x-reforged-admin-key` (from `.workspace\config\secrets.env`) and
  `{"Username": "...", "Status": "banned", "Reason": "..."}` (`active` lifts it). The player sees
  the reason in the launcher.
- **Registration**: `--registration OPEN` lets anyone who can reach the server register without a
  code; `NONE` closes registration.
- **News and art**: `.workspace\config\news.json` (`{"items": [{"date", "title", "body"}]}`) and
  images in `.workspace\branding\` show in the launcher; see [legacy/ReforgedContent/README.md](../legacy/ReforgedContent/README.md).
- **Backups**: back up MongoDB (`mongodump`), and keep `.workspace\config\` (secrets) and
  `.workspace\tls\` (public: the certificate) safe and private.
- **Updating**: `npm run update` (or **Update setup** on the launcher's Host page) compares
  `D:\Reforged` with GitHub, fast-forwards it when GitHub has more and runs `npm run setup` again (it keeps
  your SDK, game data and secrets; `-- --check` only compares). It refuses to overwrite files you changed
  in the project, and says so. When it says the game runtime changed, build the runtime again and
  `npm run publish-runtime`; players are asked to accept a new runtime. `git pull` then `npm run setup`
  does the same by hand.
- **A new certificate** (public) changes the fingerprint: send new invites. Players keep their
  accounts; the launcher asks them to confirm the new certificate first.
- **CPU and RAM**: each game server is capped at 30 frames per second, and at 10 once nobody has been
  on it for 15 s; the Training Grounds start the first time someone goes there and stop again after
  5 min without players; the game servers log warnings and errors only. Change it with
  `npm run setup -- --server-fps 30 --idle-fps 10`, `--training-idle-minutes 5` (0 keeps them running),
  `--training-at-boot` and `--verbose-game-logs` (then restart). The frame cap and the runtime's CPU
  fixes are in the runtime (patches 0019 and 0020 in [paradox-patches.md](paradox-patches.md)): after
  `npm run update`, build it again and `npm run publish-runtime`. `npm run doctor` says whether the
  published runtime has them. More in [hosting-linux.md](hosting-linux.md#cpu-and-ram).

## Known limits

- First version: the server side was tested end to end, but not yet with the real 1.14.7 client
  and real game servers. Expect to look at logs on the first evenings.
- Lady Luck's Store and the platinum store's rank skips, passes and captured prices come from captures
  of the live store service, which `npm run gamedata` downloads (see
  [sdk-and-game-data.md](sdk-and-game-data.md#the-store-captures)). Without them, Lady Luck has no
  offers and the platinum store only reconstructed ones.
- The progression config is built from the game's own progression tables (see
  [sdk-and-game-data.md](sdk-and-game-data.md#the-progression-config)); the Hunt Pass has ranks but
  no rewards.
- A 1.14.7 hub (Ramsgate, the Training Grounds) never asks for the progression config on its own,
  and every player then drops after a minute ("Loading PlayerData Failed" in its log). `npm run start`
  therefore creates `CALL_HUNTPASS_QUERY.flag` beside the game executable, one of Mystic Paradox's
  runtime switches: it makes the hub query the Hunt Pass season, which fetches the config. It acts on
  game servers only, not on your own game client. Delete the file to turn it off.
- One PC for everything: the services, the database and the game servers. Each hunt is a whole game
  process (about 1 GB of RAM); `--max-hunts` caps them (default 3, up to 10), and the memory guard refuses
  a new hunt while the RAM for it is not free. More than 6 hunts need one more UDP port below 8770 per hunt
  (8768-8777 for 8, 8766-8777 for 10) forwarded or allowed too.
- Windows for the launcher, the runtime build and the SDK, like the game. The server itself can also
  run on Linux, with the game servers under Wine ([hosting-linux.md](hosting-linux.md), experimental).

## Ports

| Port | What | Reachable from |
|---|---|---|
| 443/TCP (public) or 61000/TCP on the Tailscale IP (private) | gateway | players |
| 8770-8775/UDP | hunts (with `--max-hunts` above 6, from 8768 for 8 or 8766 for 10) | players (public mode: only after they logged in) |
| 8776/UDP | Training Grounds | players (public mode: only after they logged in) |
| 8777/UDP | Ramsgate | players (public mode: only after they logged in) |
| 61000/TCP on 127.0.0.1 | backend | this PC |
| 61001/TCP | Director | this PC |
| 61002/TCP | content server | this PC |
| 61005/TCP | allowlist helper (public) | this PC |
| 27017/TCP | MongoDB | this PC |
| 61100/TCP | the launcher's relay | the PC running the launcher |
