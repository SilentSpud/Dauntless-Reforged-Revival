# Hosting on a Linux server (Debian 13)

A public Dauntless Reforged Revival server on a Linux VPS, for example a Hostinger KVM VPS with
Debian 13. Nothing to install on players' PCs beyond the usual launcher: they join with an invite as
for any public server ([players.md](players.md)).

> [!WARNING]
> **Experimental.** The Linux port is adapted from
> [Dauntless Revived's Linux port](https://github.com/mixutin/dauntless-revived/tree/main/deploy/linux-server).
> What was tested on Linux: the services, the real Director starting Ramsgate and the Training
> Grounds through the Wine launcher and stopping them cleanly, a Windows GUI program run the way a
> game server is run (Mystic Paradox's `winmm.dll` loaded, the arguments, the environment and the
> readiness line all coming through Wine), and the nftables allowlist changing a real firewall.
> **The real Dauntless 1.14.7 has not yet been run under Wine.** Expect to read logs on the first
> evenings, and report what you find.

## The quick way: one command from your Windows PC

Steps 2 to 7 below, done for you from the Windows PC that has your game, with the SSH client that comes
with Windows. You need the VPS's public IPv4 address and its `root` password (Hostinger: hPanel > VPS >
Overview), and step 1 (the provider's firewall) done.

1. Download [`Deploy-FromWindows.ps1`](../legacy/ReforgedHost/linux/Deploy-FromWindows.ps1) (on GitHub:
   **Download raw file**), for example into `Downloads`.
2. In PowerShell:

   ```powershell
   cd $HOME\Downloads
   powershell -ExecutionPolicy Bypass -File .\Deploy-FromWindows.ps1 -Server <VPS IPv4> -Name "Friday Hunts" -Game C:\Dauntless
   ```

It checks that `C:\Dauntless` is Dauntless 1.14.7 (the folder with `Archon` inside, or the folder
holding it), asks the VPS password once (it adds an SSH key for this PC), installs everything on the
VPS with [`setup-vps.sh`](../legacy/ReforgedHost/linux/setup-vps.sh), sends the game (about 15 GB: keep the
window open; a second run skips it when the VPS already has the same files), sends the runtime and the
newest game data pack when this PC has a Reforged project (`C:\Reforged`, or `-Runtime <folder>` and
`-GameData <pack>`), starts the server and prints your first invite. Run it again whenever you like:
every step keeps what is already done.

**A Hostinger Game Panel plan** (or any VPS with a web panel) already serves its panel on port 443.
Give the server another port with `-PublicPort 4443` (and allow TCP 4443 instead of 443 in the
firewall): it goes into the invites, and the launcher connects to it. Without it, players' launchers
reach the panel and refuse its certificate ("the security certificate does not match").

Without a runtime yet, players can join but not play: send it later with
`-Runtime <folder> -SkipGame` (step 6 says where it comes from).

On the VPS itself, without Windows: as root, `sh setup-vps.sh --name "Friday Hunts"` (the script is
in `legacy/ReforgedHost/linux/`), copy the game to `/home/reforged/games/Dauntless`, then
`sh /home/reforged/Reforged/legacy/ReforgedHost/linux/setup-vps.sh --finish [--runtime <folder>] [--gamedata <pack>]`.

The rest of this page is the same thing by hand, and what to do when something goes wrong.

## How it differs from a Windows server

The services (backend, Director, content server, gateway, MongoDB) run natively. The game servers
are still `Dauntless-Win64-Shipping.exe`, a Windows program: the Director starts
`.workspace/bin/reforged-gameserver`, which runs it through **Wine** on a virtual display
(**Xvfb**), because the game opens a window even as a dedicated server.

| | Windows | Linux |
|---|---|---|
| Game servers | the game executable | the same executable through Wine (or Proton) on `xvfb-run` |
| Game-port allowlist | one Windows Firewall rule | one nftables table, `reforged_revival` |
| Privileges | the terminal runs as administrator | two systemd units, each with only the privilege it needs (no root) |
| Starts at boot | no (`reforged-host start`) | yes (systemd) |
| SDK, runtime, game data | made on this PC | made **once on a Windows PC** with your game, then copied over |

The two systemd units run as an ordinary user:

- `reforged-allowlist.service`: the allowlist helper, with `CAP_NET_ADMIN` only (it changes nftables).
- `reforged.service`: `reforged-host start` (MongoDB, backend, Director and its game servers, content
  server, gateway), with `CAP_NET_BIND_SERVICE` only (the gateway listens on port 443).

## What you need

- A VPS with **Debian 13** (x86-64). Each game server is a whole game process under Wine (about
  1-1.5 GB of RAM); Ramsgate always runs, the Training Grounds when someone uses them, plus one per
  hunt (`--max-hunts`, 3 by default, up to 10). **16 GB of RAM** is comfortable (Hostinger KVM 4) for
  about 6 hunts at once; 8 GB (KVM 2) only with `--max-hunts 1` or 2. The memory guard refuses a new
  hunt when the RAM for it is not free (see [CPU and RAM](#cpu-and-ram)). About 30 GB of free disk.
- The VPS's **public IPv4 address** (Hostinger: hPanel > VPS > Overview).
- Your own **Dauntless 1.14.7** (from your Epic library, installed with Legendary: see
  [players.md](players.md#1-install-dauntless-1147)).
- **Once, a Windows PC** with your 1.14.7, to build the game runtime (`MysticParadox.dll`,
  `winmm.dll`) and the game data. They are made from your own game and are never shared; see
  [sdk-and-game-data.md](sdk-and-game-data.md).

## 1. The provider's firewall

Allow inbound **TCP 443** (the gateway) and **UDP 8770-8777** (the game servers), and keep SSH
(TCP 22). More than 6 hunts at once use one more port below 8770 per hunt: UDP 8768-8777 for 8,
8766-8777 for 10. On Hostinger: the VPS's **Firewall** page in hPanel; make a rule set with those three
and activate it for the VPS. Never open 61000-61005 or 27017: they listen on 127.0.0.1 only.

The game ports stay closed to everyone who has not logged in: the allowlist's own nftables table
opens them per player, whatever the provider's firewall allows.

## 2. Packages (as root)

Sign in to the VPS over SSH (Hostinger gives you `root`), then:

```sh
apt update && apt upgrade -y
apt install -y nodejs wine wine64 xvfb xauth nftables sudo curl gnupg
node --version      # 20.19 or newer (the backend and the Director still run on Node.js)
wine --version
```

**MongoDB** 7 or newer, from MongoDB's own repository. Only the server program (`mongod`) is needed:

```sh
curl -fsSL https://www.mongodb.org/static/pgp/server-8.0.asc | gpg --dearmor -o /usr/share/keyrings/mongodb-server-8.0.gpg
echo "deb [ signed-by=/usr/share/keyrings/mongodb-server-8.0.gpg ] http://repo.mongodb.org/apt/debian bookworm/mongodb-org/8.0 main" > /etc/apt/sources.list.d/mongodb-org-8.0.list
apt update && apt install -y mongodb-org-server
systemctl disable --now mongod     # the host kit starts its own MongoDB, as a replica set
```

MongoDB names the Debian releases it supports on
[its install page](https://www.mongodb.com/docs/manual/tutorial/install-mongodb-on-debian/): use
the newest one listed there (above, `bookworm`) if Debian 13 (`trixie`) is not listed yet, or is listed without `mongodb-org-server` (`apt` then says "Unable to locate package").

## 3. A user for the server

The server never runs as root (it faces the internet, and Wine refuses a folder another user
owns). Make a user, let it use `sudo` for the two install scripts, and sign in as that user:

```sh
adduser reforged
usermod -aG sudo reforged
sudo -iu reforged
```

Everything below runs as `reforged`.

## 4. Dauntless 1.14.7 on the VPS

Either copy the `Dauntless` folder from your Windows PC (about 15 GB), for example with `scp -r`
from PowerShell to `reforged@<VPS IP>:~/games/`, or install it on the VPS with
[Legendary](https://github.com/legendary-gl/legendary):

```sh
sudo apt install -y pipx && pipx install legendary-gl && export PATH="$HOME/.local/bin:$PATH"
legendary auth          # prints a link: sign in to Epic in your browser and paste the code back
legendary install Jackal --manifest "https://api.mysticfox.dev/v2/Dauntless_1.14.7.647472.manifest" --base-url "https://egs-cloudfront-chunks.epicgamescdn.com/Builds/Org/o-krlzxj88qrtb69fredeuaf887bl5az/Jackal/default" --base-path ~/games --skip-sdl
legendary verify Jackal
```

The game is then in `/home/reforged/games/Dauntless` (the folder with `Archon` inside). Never run
`legendary update Jackal`: it would turn it into 2.1.1.

## 5. Get the server release and set up the server

The server comes built, as a release on GitHub
(<https://github.com/SilentSpud/Dauntless-Reforged-Revival/releases>, the `server-v<version>` ones):
`ReforgedRevivalServer-<version>-linux-x64.tar.gz` and `SHA256SUMS.txt`. No Rust, git or npm on the VPS.
Pick the version (the newest `server-v` release), then:

```sh
v=<version>             # for example 0.3.0
cd ~
curl -fLO "https://github.com/SilentSpud/Dauntless-Reforged-Revival/releases/download/server-v$v/SHA256SUMS.txt"
curl -fLO "https://github.com/SilentSpud/Dauntless-Reforged-Revival/releases/download/server-v$v/ReforgedRevivalServer-$v-linux-x64.tar.gz"
grep " ReforgedRevivalServer-$v-linux-x64.tar.gz\$" SHA256SUMS.txt | sha256sum --check    # must say OK
tar -xzf "ReforgedRevivalServer-$v-linux-x64.tar.gz" && mv "reforged-revival-server-$v" ~/Reforged
rm "ReforgedRevivalServer-$v-linux-x64.tar.gz" SHA256SUMS.txt
cd ~/Reforged
ip -br addr             # the network interface with the public IP, usually eth0
bin/reforged-host setup --mode public --host <VPS public IPv4> --name "Friday Hunts" --game /home/reforged/games/Dauntless --firewall-interface eth0
```

With the [GitHub CLI](https://cli.github.com/) signed in, `gh attestation verify ReforgedRevivalServer-$v-linux-x64.tar.gz --repo SilentSpud/Dauntless-Reforged-Revival`
also checks that GitHub's release workflow built it from the project's source (`reforged-host update` does
this by itself when `gh` is there).

With a DNS name, add `--game-address <VPS public IPv4>` (the game servers announce an address).
Setup checks every file of the release against its `release.json` (and downloads it again if some are missing), writes the configuration
and the gateway certificate (it prints the fingerprint that goes into every invite), writes the
Wine launcher and the systemd units, and makes the Wine prefix. The server preset in `preset/` fills
most of the game data.

Linux options (each is kept in `host.json` until you change it):

| Option | What |
|---|---|
| `--firewall-interface eth0` | the allowlist filters the game ports on this interface only (`--no-firewall-interface`: on every interface but loopback) |
| `--wine /usr/bin/wine64` | a particular Wine (default: the first `wine` on `PATH`) |
| `--proton <path to proton>` | Proton or GE-Proton instead of Wine (`--no-proton` goes back) |
| `--no-xvfb` | no virtual display (only if the VPS has a real X display in `DISPLAY`) |
| `--max-hunts 1-10` | at most this many hunts at once (default 3; above 6 the game ports reach below 8770, see step 1) |
| `--public-port 4443` | the port players connect to, when 443 is taken (a Hostinger Game Panel plan serves its panel there); `--public-port 443` goes back |

## 6. The runtime and the game data, from your Windows PC

On the Windows PC with your 1.14.7, follow [hosting.md](hosting.md) up to a server that plays
(the launcher's Host tools page does most of it), then:

1. **Runtime.** After publishing the runtime there (`reforged-host publish-runtime`, or `npm run publish-runtime` on a Node-kit host), `D:\Reforged\.workspace\runtime\` holds
   `MysticParadox.dll` and `winmm.dll`. Copy that folder to the VPS, for example
   `scp -r D:\Reforged\.workspace\runtime reforged@<VPS IP>:~/runtime`.
2. **Game data.** `reforged-host gamedata --export` (Node kit: `npm run gamedata -- --export`) there writes a game data pack
   (`.workspace\gamedata-packs\reforged-gamedata-1.14.7-<date>.json.gz`). Copy it to the VPS too.

On the VPS:

```sh
cd ~/Reforged
bin/reforged-host publish-runtime --from ~/runtime
bin/reforged-host gamedata --import ~/reforged-gamedata-1.14.7-<date>.json.gz
```

The pack carries what the preset cannot (Lady Luck's Store and the platinum store, built from your
CatalogExporter export). For a first start-up test without it, `bin/reforged-host gamedata --examples`
fills the two store files with examples: the server starts, but those stores are empty.

## 7. Firewall, services, invites

```sh
sudo sh ~/Reforged/.workspace/config/firewall.sh            # opens 443 and the game ports in ufw, if ufw is active
sudo sh ~/Reforged/.workspace/config/systemd/install.sh     # installs and starts both units; they start at boot from now on
journalctl -u reforged -f                                   # the server's output (Ctrl+C leaves it running)
~/Reforged/bin/reforged-host doctor                         # checks every part
~/Reforged/bin/reforged-host invite --for "Alex"            # one invite per friend
```

The first game servers take longer than on Windows (Wine loads the game): the Director waits up to
90 seconds for each one.

## Running it

- **Stop / start / restart:** `sudo systemctl stop reforged` (every game server stops with it),
  `sudo systemctl start reforged`, `sudo systemctl restart reforged` (after changing the runtime,
  the game data or the multipliers).
- **Logs:** `journalctl -u reforged -f`, `journalctl -u reforged-allowlist -f`, and
  `~/Reforged/.workspace/logs/` (one file per service; the game servers in `gameservers/`).
- **Updating:** `~/Reforged/bin/reforged-host update` (`--check` only compares). It downloads the
  newest server release, checks it against `SHA256SUMS.txt` (and its attestation with `gh`, when it
  is installed and signed in), installs it over `~/Reforged` (`.workspace/` and your own files stay)
  and runs setup again. Then `sudo sh ~/Reforged/.workspace/config/systemd/install.sh` (it installs
  the units again and restarts the server). When the update says the runtime changed, build it again
  on the Windows PC and publish it with `--from`. `sh ~/Reforged/legacy/ReforgedHost/linux/setup-vps.sh`
  as root does the same.
- **A server set up before the release** (a `git clone` with `legacy/ReforgedHost` and `npm run`):
  as root, `sh setup-vps.sh` (the newest one, from GitHub) moves it over, or by hand as `reforged`:
  download and check the archive as in step 5, put its `bin/reforged-host` in `~/Reforged`, run
  `cd ~/Reforged && ./reforged-host update`, then `rm ~/Reforged/reforged-host` and use
  `~/Reforged/bin/reforged-host` from then on. `.workspace/`, `host.json` and the env files stay as they are.
- **Bans, registration, news, backups:** as on Windows ([hosting.md](hosting.md#running-it)).
  Back up MongoDB with `mongodump`, and keep `.workspace/config/` and `.workspace/tls/` private.
- **Supplies refilled at every login:** list per account, in `~/Reforged/.workspace/config/session-supplies.json`,
  the stacked items (tonics, pylons, grenades...) that go back to a fixed quantity each time that player
  logs in: `{"accounts": {"<user id>": {"QI_GRENADE_ICE": 1000}}}`. It is read at every login, so no restart
  is needed; see [patch 0023](paradox-patches.md#0023-supplies-refilled-at-every-game-login).
- **The allowlist table:** `sudo nft list table inet reforged_revival` shows who can reach the
  game ports right now.

## CPU and RAM

Almost all of the server's CPU and RAM goes to the game servers: Ramsgate, the Training Grounds and one
per hunt, each a whole Dauntless process under Wine. Before patches 0019 to 0021 (see
[paradox-patches.md](paradox-patches.md)) every one of them, empty or not, walked all ~208,000 game
objects on every frame, built a function name and ran about 90 text searches for every reflected call (tens
of thousands a second with players), asked the disk for flag files several times a frame (a whole folder
scan under Wine), and ran its frames as fast as the CPU allowed. A 4 vCPU VPS sat near 40% with nobody
online and above 80% with two players.

What the server does now (each part can be changed):

| | Default | Change it with `bin/reforged-host setup ...` |
|---|---|---|
| Frames per second of a game server with players (runtime patch 0020) | 30 (the network rate) | `--server-fps 10-240`, `0` for no cap |
| Frames per second after 15 s without players | 10 | `--idle-fps 1-240`, `0` for the same as above |
| Training Grounds (Director patches 0021 and 0033) | started the first time someone goes there, stopped again after 5 min without players | `--training-idle-minutes 0-1440` (0 keeps them running), `--training-at-boot` (`--training-on-demand` goes back) |
| Game-server logs | warnings and errors | `--verbose-game-logs` (`--quiet-game-logs` goes back) |
| Hunts at once | 3 (UDP 8770-8777; above 6, one more port below 8770 per hunt) | `--max-hunts 1-10` |
| A new hunt only when its memory is free (Director patch 0022) | on: 1536 MB for the hunt plus 1024 MB left free | `--no-memory-guard` (`--memory-guard`), `--hunt-ram <MB>`, `--min-free-ram <MB>` |
| MongoDB's cache | 1 GB (it used to take up to half the RAM) | |

Restart the server after a change (`sudo systemctl restart reforged`).

**The runtime has to be built again.** The frame cap and the CPU fixes are in `MysticParadox.dll`, which is
built on the Windows PC: there, `npm run update`, build the runtime again (the launcher's Host tools page),
`npm run publish-runtime`, then send it to the VPS (`Deploy-FromWindows.ps1 ... -SkipGame`, or copy
`.workspace\runtime` and `npm run publish-runtime -- --from ~/runtime` on the VPS) and restart. Players
accept the new runtime once in the launcher. `npm run doctor` says whether the published runtime has
patches 0019 and 0020; the Training Grounds, the logs and MongoDB's cache work as soon as setup has run.

**See where the CPU goes:**

```sh
top -o %CPU                                                # P sorts by CPU, M by memory, q quits
ps -eo pid,pcpu,pmem,rss,etime,args --sort=-pcpu | head -15 # the busiest processes
free -h                                                    # "available" is what is really free
grep -h "FramePace\|PENameCache\|PECategoryCache\|FlagCache\|ExpectedPlayerCount\] lookup" ~/games/Dauntless/Archon/Binaries/Win64/mysticparadox_dll_port*.log | tail
```

`Dauntless-Win64-Shipping.exe` lines are the game servers (Ramsgate on port 8777, the Training Grounds on
8776, hunts below). The runtime writes its own log beside the game executable, one file per port
(`mysticparadox_dll_port8777.log` for Ramsgate; use your `--game` folder if it is not `~/games/Dauntless`).
The `grep` shows whether the running runtime has the new code: `[FramePace] server frame cap: 30 fps, idle
(no client for 15 s): 10 fps` at start, `no client for 15 s: 10 fps` when a server goes idle and
`client connected: 30 fps` when someone joins.

**If `wineserver` itself uses a lot of CPU** (a line of its own in `top`, often 10-30% while players are on):
plain Wine sends every thread wait of the game through that one process. Proton (or GE-Proton) has fsync,
which keeps those waits in the kernel (Debian 13's kernel supports it). It is optional and less tested here:
download a GE-Proton release from <https://github.com/GloriousEggroll/proton-ge-custom/releases>, unpack it into
`~/.local/share/Steam/compatibilitytools.d/`, then

```sh
cd ~/Reforged
bin/reforged-host setup --proton ~/.local/share/Steam/compatibilitytools.d/<GE-Proton folder>/proton
sudo systemctl restart reforged
```

and compare `top` with players on. `bin/reforged-host setup --no-proton` goes back to Wine.

## When something goes wrong

| What you see | What to do |
|---|---|
| `do not run the server as root` | sign in as the server's user (`sudo -iu reforged`) and run it there |
| `port 443 ... needs a privilege` / `the allowlist changes the firewall` | run it through the systemd units (step 7), not `reforged-host start` in a terminal |
| a game server never becomes ready | its log in `.workspace/logs/gameservers/`; `.workspace/logs/wine.log`; for Wine's own messages, `sudo systemctl edit reforged` and add `Environment=WINEDEBUG=err+all`, then restart |
| `wine: ... is not owned by you` | the Wine prefix belongs to another user: `sudo chown -R reforged: ~/Reforged/.workspace/wine` |
| `xvfb-run was not found` / `Wine was not found` | `sudo apt install xvfb xauth wine wine64`, then `~/Reforged/bin/reforged-host setup` |
| players log in but cannot reach Ramsgate | UDP 8770-8777 in the provider's firewall (step 1), and `--firewall-interface` naming the right interface (`ip -br addr`) |
| a hunt does not start ("matchmaking failed") while others run | the hunt cap or the memory guard: `journalctl -u reforged \| grep -i "MemoryGuard\|capacity"` says which; `free -h` shows the RAM left |
| players reach Ramsgate but some hunts never load (more than 6 hunts) | the extra game ports: UDP 8768-8777 (8 hunts) or 8766-8777 (10) in the provider's firewall |
| the launcher says the certificate does not match | another program holds the gateway's port (a hosting panel on 443): `bin/reforged-host doctor` names it; give the server another port with `bin/reforged-host setup --public-port 4443` (or `Deploy-FromWindows.ps1 -PublicPort 4443 -SkipGame`), allow TCP 4443 in the provider's firewall, restart, and send new invites |
| `port 27017 is already taken` | MongoDB's own service: `sudo systemctl disable --now mongod` |

The Director passes the game-server key on each game server's command line, as on Windows; on a
VPS that only the server's user signs in to, nobody else can read it.

## Resumo em português

**O jeito rápido (um comando no Windows):** libere TCP 443 e UDP 8770-8777 no firewall da VPS na
Hostinger, baixe o [`Deploy-FromWindows.ps1`](../legacy/ReforgedHost/linux/Deploy-FromWindows.ps1) e rode no
PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File .\Deploy-FromWindows.ps1 -Server <IP da VPS> -Name "Seu Servidor" -Game C:\Dauntless
```

Ele confere o jogo, pede a senha do root da VPS uma vez, instala tudo, envia o jogo (cerca de 15 GB),
liga o servidor e mostra o primeiro convite. Pode rodar de novo quando quiser: o que já está feito é
mantido, e o jogo não é reenviado se já estiver lá.

**Plano Game Panel da Hostinger:** o painel já usa a porta 443. Rode o script com `-PublicPort 4443`
e libere TCP 4443 (no lugar da 443) no firewall da VPS. Sem isso, o launcher chega no painel e recusa o
certificado dele ("o certificado de segurança não confere").

**Passo a passo à mão:**

1. **Firewall da Hostinger:** libere TCP 443 e UDP 8770-8777 (e mantenha o SSH, TCP 22).
2. **Pacotes, como root:** `apt install -y nodejs wine wine64 xvfb xauth nftables sudo curl gnupg`
   e o MongoDB (passo 2 acima).
3. **Usuário:** `adduser reforged`, `usermod -aG sudo reforged`, `sudo -iu reforged`. O servidor
   nunca roda como root.
4. **Jogo 1.14.7:** copie a pasta `Dauntless` do seu PC ou instale com o Legendary em `~/games`.
5. **Servidor:** baixe `ReforgedRevivalServer-<versão>-linux-x64.tar.gz` e `SHA256SUMS.txt` da release
   `server-v<versão>` mais nova, confira com `sha256sum --check` e extraia em `~/Reforged` (passo 5 em inglês acima); depois
   `cd ~/Reforged && bin/reforged-host setup --mode public --host <IP da VPS> --name "Seu Servidor" --game /home/reforged/games/Dauntless --firewall-interface eth0`.
6. **No PC com Windows (uma vez):** gere o runtime e os dados do jogo, copie a pasta
   `.workspace\runtime` e o pacote de dados para a VPS, e rode
   `bin/reforged-host publish-runtime --from ~/runtime` e `bin/reforged-host gamedata --import <pacote>`.
7. **Ligar:** `sudo sh ~/Reforged/.workspace/config/firewall.sh` e
   `sudo sh ~/Reforged/.workspace/config/systemd/install.sh`. Depois, `~/Reforged/bin/reforged-host invite --for "Nome"`.

**Suprimentos reabastecidos a cada login:** em `~/Reforged/.workspace/config/session-supplies.json`,
liste por conta os itens empilháveis (tônicos, pilões, granadas...) que voltam para uma quantidade fixa
toda vez que o jogador entra: `{"accounts": {"<id da conta>": {"QI_GRENADE_ICE": 1000}}}`. O arquivo é
lido a cada login, sem reiniciar nada.

É experimental: o jogo 1.14.7 de verdade ainda não foi rodado no Wine. Se algo falhar, veja
`journalctl -u reforged -f` e `~/Reforged/bin/reforged-host doctor`.

**CPU e RAM** ([detalhes](#cpu-and-ram)): quase todo o consumo vem dos servidores de jogo (Ramsgate, Training
Grounds e um por caçada), cada um um processo inteiro do Dauntless no Wine. Agora:

- cada servidor de jogo roda a no máximo 30 FPS, e a 10 FPS depois de 15 s sem jogadores
  (`--server-fps`, `--idle-fps`; `0` desliga);
- o Training Grounds só sobe quando alguém vai para lá e desliga de novo depois de 5 min vazio
  (`--training-idle-minutes`, `0` deixa ligado; `--training-at-boot` volta ao antigo);
- os servidores de jogo registram só avisos e erros (`--verbose-game-logs` volta ao antigo);
- o MongoDB usa no máximo 1 GB de cache (antes podia usar metade da RAM);
- uma caçada nova só abre quando há RAM livre para ela: 1536 MB para a caçada mais 1024 MB de folga
  (`--hunt-ram`, `--min-free-ram`, `--no-memory-guard`). Se faltar, o jogador vê a busca falhar e tenta
  de novo depois, e o servidor não cai por falta de memória.

**Mais caçadas ao mesmo tempo:** `--max-hunts` vai até 10. Até 6 as portas são UDP 8770-8777; acima disso
cada caçada usa uma porta a mais abaixo de 8770 (UDP 8768-8777 para 8, 8766-8777 para 10), que precisa
estar liberada no firewall da Hostinger. Por exemplo, para 8, como root:

```sh
sudo -iu reforged bash -c 'cd ~/Reforged && bin/reforged-host update --no-setup && bin/reforged-host setup --max-hunts 8 --skip-install'
sh /home/reforged/Reforged/.workspace/config/systemd/install.sh
```

O limite de FPS e as correções de CPU (patches 0019 e 0020) ficam no runtime (`MysticParadox.dll`), que é
compilado no PC com Windows: lá, `npm run update`, compile o runtime de novo (página Host tools do launcher),
`npm run publish-runtime`, e envie para a VPS (`Deploy-FromWindows.ps1 ... -SkipGame`). Depois
`sudo systemctl restart reforged`. Os jogadores aceitam o runtime novo uma vez no launcher. `npm run doctor`
diz se o runtime publicado já tem os patches. Para ver quem usa a CPU: `top -o %CPU`. Se o `wineserver`
aparecer alto, experimente o GE-Proton (`bin/reforged-host setup --proton <pasta>/proton`, seção em inglês acima).
