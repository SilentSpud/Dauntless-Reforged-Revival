# Hosting in Docker

The whole server in containers, with Docker Compose: the same services `reforged-host start` runs, one
container each, from two images built from this repository. A public server on a Linux VPS is the main use;
a private server over Tailscale works too.

> [!WARNING]
> **Experimental**, as the [Linux server](hosting-linux.md) is: the game servers run through Wine, and
> **the real Dauntless 1.14.7 has not yet been run under Wine**. The containers also run the **Rust**
> gateway, content server and allowlist helper (MIGRATION_PLAN.md RS2–RS4), which `reforged-host start` on a
> plain host only switches to at the front-door cutover. Report what you find.

## What runs

| Container | Image | What it is |
| --- | --- | --- |
| `net` | front | Holds the network namespace every other container joins, and the published ports |
| `mongo` | `mongo:8.0` | MongoDB with the one-member replica set `rs0` the backend needs (until RB14) |
| `backend` | server | The Node backend (`paradox/backend`), 127.0.0.1:61000 |
| `director` | server | The Node Director, 127.0.0.1:61001; starts Ramsgate, the Training Grounds and hunts through Wine |
| `content` | front | The content server, 127.0.0.1:61002 |
| `allowlist` | front | Public servers: opens the game ports to players who logged in (nftables) |
| `gateway` | front | The front door: TLS on 443 (public) or HTTP on the Tailscale address (private) |
| `host` | server | Not a service: `docker compose run --rm host <command>` runs `reforged-host` |

**One network namespace.** Every container joins `net`'s, so they reach each other on 127.0.0.1 exactly as
the env files `reforged-host setup` writes say, and nothing but the gateway and the game ports is reachable
from outside, not even from other containers. The allowlist's nftables table lives in that namespace too: it
filters the game servers' traffic without any privilege on the host and without touching the host's firewall.
Only the allowlist has `CAP_NET_ADMIN`, and only through `nft`'s file capability.

**The same files as a Linux host.** Both images are laid out as the server release is, under `/opt/reforged`,
and every container mounts the volumes at the same paths, so `host.json`, `secrets.env` and the env files are
the host kit's, unchanged, and a `.workspace` can move between a container host and a plain one.

| Volume | Path | Holds |
| --- | --- | --- |
| `workspace` | `/opt/reforged/.workspace` | Configuration and secrets, the certificate, the published runtime, branding, logs, the Wine prefix |
| `backend-game-data`, `director-game-data` | `/opt/reforged/paradox/*/game-data` | Your game data |
| `exports`, `captures` | `/opt/reforged/paradox/Items_Analysis`, `…/DauntlessEndpointDocumentation` | CatalogExporter exports and store captures, for `gamedata` |
| `mongo-data`, `mongo-config` | MongoDB's | The database |
| bind mount `REFORGED_GAME_DIR` | `/game` | Your Dauntless 1.14.7 folder (the one with `Archon`) |
| bind mount `REFORGED_IMPORT_DIR` | `/import` (read-only, `host` only) | Files to bring in: the runtime's DLLs, game data packs |

## Before you start

- **Docker Engine with Compose 2.24 or newer** on Linux. A public server needs Docker Engine on the Linux
  machine itself: Docker Desktop (Windows, macOS) hides the players' addresses behind its own, so the
  gateway and the allowlist see one address for everyone. Docker Desktop is fine for a private server or a
  try-out.
- The kernel's nftables (`nf_tables`, loaded on any distribution whose Docker uses iptables-nft, which is the
  default on Debian 12 and 13 and Ubuntu 22.04 and newer).
- Your Dauntless 1.14.7 folder on the server, as for [a Linux host](hosting-linux.md) (about 15 GB), owned by
  or writable for uid 1000: the game servers write their logs and a switch file next to the executable.
- The game runtime and game data, from a Windows PC with your own game ([sdk-and-game-data.md](sdk-and-game-data.md)).
- Public: the provider's firewall open for TCP 443 and UDP 8770-8777, as in [hosting-linux.md](hosting-linux.md).

## A public server

```bash
git clone https://github.com/SilentSpud/Dauntless-Reforged-Revival.git
cd Dauntless-Reforged-Revival
cp .env.example .env
```

In `.env`, set `REFORGED_GAME_DIR` to your Dauntless folder and leave `COMPOSE_PROFILES=public`. Then:

```bash
REFORGED_COMMIT=$(git rev-parse --short=12 HEAD) docker compose build
docker compose run --rm host setup --mode public --host play.example.org --name "Friday Hunts"
```

`setup` takes every option it takes on a Linux host (`--max-hunts`, `--public-port`, `--registration` …;
`docker compose run --rm host setup --help`). In the container it also gets `--skip-install` (the services are
in the image), `--no-manage-mongo --mongo-uri mongodb://127.0.0.1:27017/?replicaSet=rs0` (the `mongo`
container) and `--game /game`, unless you give those yourself. Ignore what it says about systemd and ufw:
compose runs the server. On a DNS name, give `--game-address <public IPv4>` as on a Linux host.

The runtime and the game data: put the files in `./import` (or `REFORGED_IMPORT_DIR`), then

```bash
docker compose run --rm host publish-runtime --from /import/runtime
docker compose run --rm host gamedata --import /import/gamedata.zip
```

Start it, and make an invite:

```bash
docker compose up -d
docker compose ps                       # every service healthy
docker compose run --rm host invite --for "Alex"
```

`docker compose logs -f` shows every service (`docker compose logs -f director` one); the game servers' own
logs are in the `workspace` volume, `logs/gameservers/`. `docker compose run --rm host doctor` checks the
setup from inside the server's network. It checks for a plain Linux host, so in Docker a few of its lines
are expected and harmless: git, nftables and the systemd units are not in the `host` container, and the
gateway and content server count as "not built" because their Rust bins run in the `front` image instead.

**Another port than 443** (`setup --public-port 4443`): set `REFORGED_GATEWAY_PORT=4443` and
`REFORGED_GATEWAY_PUBLISH=4443` in `.env` too, and allow TCP 4443 in the provider's firewall. **More than six
hunts** (`--max-hunts 10`): `REFORGED_GAME_PORTS=8766-8777`.

## A private server (Tailscale)

The `net` container becomes a Tailscale node of its own ([compose.tailscale.yaml](../compose.tailscale.yaml)):
the server's Tailscale address is inside the shared namespace, the gateway and the game servers listen on it,
and nothing is published on the host. Make a reusable auth key in the Tailscale admin console, then in `.env`:

```bash
COMPOSE_PROFILES=
COMPOSE_FILE=compose.yaml:compose.tailscale.yaml     # Windows: compose.yaml;compose.tailscale.yaml
TS_AUTHKEY=tskey-auth-...
```

```bash
docker compose up -d net
docker compose exec net tailscale ip -4                 # the server's Tailscale address
docker compose run --rm host setup --mode private --host 100.x.y.z --name "Friday Hunts"
docker compose up -d
```

The node is a machine of its own in your tailnet (`reforged-revival`, or `TS_HOSTNAME`): share that one with
your friends, and give its share link to setup with `--share`.

## Day to day

| What | How |
| --- | --- |
| Stop, start | `docker compose stop`, `docker compose up -d`; a clean stop includes the game servers |
| Change a setting | `docker compose run --rm host setup …` (or `multipliers`), then `docker compose up -d --force-recreate` |
| Update | `git pull`, `docker compose build`, `docker compose up -d`. `reforged-host update` is not used in Docker |
| Back up | `docker compose run --rm host backup` (into the `workspace` volume, `backups/`), and keep a copy of the `workspace` volume: it holds the certificate every invite names and the secrets |
| A shell | `docker compose run --rm --entrypoint bash host` |

Never delete the `workspace` volume of a server that has players: a new certificate changes the fingerprint
in every invite, and new secrets log everyone out.

## Testing the images

`bash docker/run-tests.sh` (from the repository root) is the full test run CI's `docker` job does on the built
images:

1. It builds `front`, `server` and `test`. The `test` image is the `server` image plus the Rust toolchain, with
   the server crates' tests compiled in, so the code is tested on the OS, Node, Wine and paths production runs on.
2. Inside the `test` image: `cargo test` for the server crates (protocol, proxy, service-kit, store, gateway,
   content, host), with the allowlist's nftables test applying its rules for real in the container's own network
   namespace, then the Node backend's and Director's suites.
3. The compose smoke test (`docker/smoke-test.sh`): a public server set up on example game data in throwaway
   volumes, every service healthy, the gateway serving setup's certificate, the allowlist's table in place, an
   invite, a clean stop.

`bash docker/run-tests.sh rust` (or `backend`, `director`) runs only those suites in step 2; `SKIP_SMOKE=1`
leaves out step 3. On a server, `docker compose run --rm test` runs step 2 with your game-data volumes mounted
read-only, so the backend's and Director's game-data tests (skipped without real data) check your data too.

## Moving an existing Linux host into Docker

Stop the systemd units, then copy the host's `.workspace` into the `workspace` volume and its game data into
the game-data volumes (`docker compose run --rm -v /home/reforged/Reforged:/old:ro --entrypoint bash host`,
then `cp -a` from `/old`), and the MongoDB database with `mongodump` and `mongorestore` into the `mongo`
container. Run setup in the container once, so the env files name the container's paths, and start it.

## Later in the migration

When the Rust backend and Director ship (RB14), the `backend` and `director` containers run the Rust bins, and
the `mongo` container gives way to Postgres (RD15) after the rollback window; the volumes and the `host`
commands stay. Until RS5 the server image also carries `legacy/ReforgedGateway`'s `make-cert`, which `setup`
still uses for a new certificate.
