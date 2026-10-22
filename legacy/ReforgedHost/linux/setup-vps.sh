#!/bin/sh
# Dauntless Reforged Revival: sets up a public server on a Debian 13 VPS (docs/hosting-linux.md).
# Run it as root. Deploy-FromWindows.ps1 runs both steps for you from your Windows PC.
#
#   sh setup-vps.sh [--name "<server name>"] [--host <public IPv4>] [options]
#       Step 1: installs the packages and MongoDB, makes the server's user, gets the project and
#       runs npm run setup. Safe to run again (it updates what is there).
#
#   sh setup-vps.sh --finish [--game-from <folder>] [--runtime <folder>] [--gamedata <pack>]
#       Step 2, once the game is on the VPS: moves the uploaded game into place, publishes the
#       runtime, imports the game data, installs the systemd units, starts the server and makes
#       a first invite.
set -eu

REPO_URL=https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival
BRANCH=main
SERVICE_USER=reforged
SERVER_NAME="Dauntless Reforged Revival"
HOST=
GAME_ADDRESS=
IFACE=
MAX_HUNTS=
PUBLIC_PORT=
FINISH=0
GAME_FROM=
RUNTIME=
GAMEDATA=
SKIP_APT=0
INSTALL_ONLY=0
NO_SERVICES=0
PINNED_EXE_SHA256=f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b

say() { printf '%s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
warn() { printf '! %s\n' "$*" >&2; }
die() { printf '\nError: %s\n' "$*" >&2; exit 1; }

usage() {
    cat >&2 <<'EOF'
usage: sh setup-vps.sh [--name "<server name>"] [--host <public IPv4 or DNS name>] [--game-address <IPv4>]
                       [--interface <eth0>] [--max-hunts 1-10] [--public-port <port>] [--user reforged] [--branch main] [--repo <url>]
                       [--skip-apt] [--install-only]
       sh setup-vps.sh --finish [--game-from <folder>] [--runtime <folder>] [--gamedata <pack>] [--no-services]
EOF
    exit 2
}

need_value() {
    if [ "$#" -lt 2 ] || [ -z "$2" ]; then warn "$1 needs a value"; usage; fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --name) need_value "$@"; SERVER_NAME=$2; shift 2 ;;
        --host) need_value "$@"; HOST=$2; shift 2 ;;
        --game-address) need_value "$@"; GAME_ADDRESS=$2; shift 2 ;;
        --interface) need_value "$@"; IFACE=$2; shift 2 ;;
        --max-hunts) need_value "$@"; MAX_HUNTS=$2; shift 2 ;;
        --public-port) need_value "$@"; PUBLIC_PORT=$2; shift 2 ;;
        --user) need_value "$@"; SERVICE_USER=$2; shift 2 ;;
        --branch) need_value "$@"; BRANCH=$2; shift 2 ;;
        --repo) need_value "$@"; REPO_URL=$2; shift 2 ;;
        --skip-apt) SKIP_APT=1; shift ;;
        --install-only) INSTALL_ONLY=1; shift ;;
        --finish) FINISH=1; shift ;;
        --game-from) need_value "$@"; GAME_FROM=$2; shift 2 ;;
        --runtime) need_value "$@"; RUNTIME=$2; shift 2 ;;
        --gamedata) need_value "$@"; GAMEDATA=$2; shift 2 ;;
        --no-services) NO_SERVICES=1; shift ;;
        -h|--help) usage ;;
        *) warn "unknown option $1"; usage ;;
    esac
done

[ "$(id -u)" -eq 0 ] || die "run it as root: sudo sh $0 ..."
printf '%s' "$SERVICE_USER" | grep -Eq '^[a-z_][a-z0-9_-]{0,31}$' || die "--user must be a plain user name like reforged"
[ -z "$MAX_HUNTS" ] || printf '%s' "$MAX_HUNTS" | grep -Eq '^([1-9]|10)$' || die "--max-hunts must be 1 to 10"
[ -z "$PUBLIC_PORT" ] || printf '%s' "$PUBLIC_PORT" | grep -Eq '^[0-9]{1,5}$' || die "--public-port must be a TCP port number, like 4443"
[ -z "$IFACE" ] || printf '%s' "$IFACE" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9_.@-]{0,14}$' || die "--interface must be a network interface name like eth0"

USER_HOME=/home/$SERVICE_USER
PROJECT=$USER_HOME/Reforged
KIT=$PROJECT/legacy/ReforgedHost
GAMES=$USER_HOME/games
GAME=$GAMES/Dauntless
EXE=$GAME/Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe
CONFIG=$PROJECT/.workspace/config

# A command as the server's user, in the host kit's folder. The arguments go through as they are.
kit() {
    sudo -H -u "$SERVICE_USER" sh -c 'cd "$1" && shift && exec "$@"' sh "$KIT" "$@"
}

route_field() {
    ip -4 route get 1.1.1.1 2>/dev/null | awk -v want="$1" '{ for (i = 1; i < NF; i++) if ($i == want) { print $(i + 1); exit } }'
}

# ---------------------------------------------------------------- step 1: install

install_packages() {
    step "Packages"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends git nodejs npm xvfb xauth nftables sudo curl gnupg ca-certificates iproute2 procps
    apt-get install -y wine
    # The 64-bit Wine has its own package on some releases; nothing is lost when there is none.
    apt-get install -y wine64 >/dev/null 2>&1 || true
    if ! node -e 'const [a, b] = process.versions.node.split(".").map(Number); process.exit(a > 20 || (a === 20 && b >= 19) ? 0 : 1)' 2>/dev/null; then
        say "Node.js $(node --version 2>/dev/null || echo '(none)') is too old: installing Node.js 22 from NodeSource"
        curl -fsSL https://deb.nodesource.com/setup_22.x -o /root/nodesource_setup.sh
        bash /root/nodesource_setup.sh
        apt-get install -y nodejs
    fi
    say "Node.js $(node --version), $(wine --version 2>/dev/null || echo 'Wine: not found')"
}

MONGODB_REPO=${MONGODB_REPO:-https://repo.mongodb.org/apt/debian}

# Does this repository (version, Debian release) hold mongod for amd64? A release can be listed
# before its packages are there (trixie/8.0 was, with only the tools).
mongodb_has_server() {
    for index in Packages.gz Packages; do
        url="$MONGODB_REPO/dists/$2/mongodb-org/$1/main/binary-amd64/$index"
        case "$index" in
            *.gz) curl -fsSL "$url" 2>/dev/null | gzip -dc 2>/dev/null | grep -q '^Package: mongodb-org-server$' && return 0 ;;
            *) curl -fsSL "$url" 2>/dev/null | grep -q '^Package: mongodb-org-server$' && return 0 ;;
        esac
    done
    return 1
}

# "<version> <release>" of the first repository with mongod: this Debian's own, then the one before.
find_mongodb_repo() {
    for pick in "8.0 $1" "8.2 $1" "8.0 bookworm" "7.0 bookworm"; do
        # shellcheck disable=SC2086
        if mongodb_has_server $pick; then printf '%s\n' "$pick"; return 0; fi
    done
    return 1
}

install_mongodb() {
    step "MongoDB"
    if command -v mongod >/dev/null 2>&1; then
        say "already installed: $(command -v mongod)"
    else
        # shellcheck disable=SC1091
        codename=$(. /etc/os-release && printf '%s' "${VERSION_CODENAME:-bookworm}")
        pick=$(find_mongodb_repo "$codename") || die "found no MongoDB repository with mongod for Debian $codename or bookworm: install MongoDB 7+ by hand (docs/hosting-linux.md), then run this again"
        version=${pick% *}
        dist=${pick#* }
        # Only ours: an earlier run may have left a repository without mongod.
        rm -f /etc/apt/sources.list.d/mongodb-org-*.list
        curl -fsSL "https://www.mongodb.org/static/pgp/server-$version.asc" | gpg --dearmor --yes -o "/usr/share/keyrings/mongodb-server-$version.gpg"
        echo "deb [ signed-by=/usr/share/keyrings/mongodb-server-$version.gpg ] $MONGODB_REPO $dist/mongodb-org/$version main" > "/etc/apt/sources.list.d/mongodb-org-$version.list"
        apt-get update
        apt-get install -y mongodb-org-server || die "MongoDB $version did not install from the $dist repository: see https://www.mongodb.com/docs/manual/tutorial/install-mongodb-on-debian/ and run this again"
        say "MongoDB $version installed from MongoDB's $dist repository: $(command -v mongod)"
    fi
    # The host kit starts its own mongod, as a replica set, on the same port.
    if command -v systemctl >/dev/null 2>&1; then systemctl disable --now mongod >/dev/null 2>&1 || true; fi
}

make_user() {
    step "The server's user: $SERVICE_USER"
    if id -u "$SERVICE_USER" >/dev/null 2>&1; then
        say "already there"
    elif command -v adduser >/dev/null 2>&1; then
        adduser --disabled-password --gecos "" "$SERVICE_USER" >/dev/null
        say "made (no password: sign in to it with sudo -iu $SERVICE_USER)"
    else
        useradd --create-home --shell /bin/bash "$SERVICE_USER"
        say "made (no password: sign in to it with sudo -iu $SERVICE_USER)"
    fi
    [ -d "$USER_HOME" ] || die "$SERVICE_USER has no home folder at $USER_HOME"
    mkdir -p "$GAMES"
    chown "$SERVICE_USER:" "$GAMES"
}

get_project() {
    step "The project"
    if [ -d "$PROJECT/.git" ]; then
        if sudo -H -u "$SERVICE_USER" git -C "$PROJECT" pull --ff-only; then say "updated: $PROJECT"; else warn "could not update $PROJECT (changed by hand?): keeping it as it is"; fi
    else
        sudo -H -u "$SERVICE_USER" git clone --branch "$BRANCH" "$REPO_URL" "$PROJECT"
    fi
    kit npm install --no-audit --no-fund
}

run_setup() {
    step "Server setup (npm run setup)"
    if [ -z "$HOST" ]; then
        HOST=$(route_field src)
        [ -n "$HOST" ] || die "could not find this VPS's public IPv4: pass --host <IPv4>"
        say "public address: $HOST (from the default route; pass --host to change it)"
    fi
    # A DNS name: the game servers announce an IPv4, this VPS's own.
    if [ -z "$GAME_ADDRESS" ] && ! printf '%s' "$HOST" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
        GAME_ADDRESS=$(route_field src)
        [ -n "$GAME_ADDRESS" ] && say "game servers announce: $GAME_ADDRESS (pass --game-address to change it)"
    fi
    if [ -z "$IFACE" ]; then
        IFACE=$(route_field dev)
        [ -n "$IFACE" ] && say "network interface: $IFACE"
    fi
    set -- --mode public --host "$HOST" --name "$SERVER_NAME" --game "$GAME"
    [ -n "$GAME_ADDRESS" ] && set -- "$@" --game-address "$GAME_ADDRESS"
    [ -n "$IFACE" ] && set -- "$@" --firewall-interface "$IFACE"
    [ -n "$MAX_HUNTS" ] && set -- "$@" --max-hunts "$MAX_HUNTS"
    [ -n "$PUBLIC_PORT" ] && set -- "$@" --public-port "$PUBLIC_PORT"
    kit npm run setup -- "$@"
    step "Host firewall"
    sh "$CONFIG/firewall.sh"
}

# ---------------------------------------------------------------- step 2: finish

place_game() {
    step "The game"
    if [ -n "$GAME_FROM" ]; then
        [ -f "$GAME_FROM/Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe" ] || die "no Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe in $GAME_FROM"
        mkdir -p "$GAMES"
        if [ -e "$GAME" ]; then
            rm -rf "$GAME.old"
            mv "$GAME" "$GAME.old"
        fi
        if ! mv "$GAME_FROM" "$GAME" 2>/dev/null; then
            # Another filesystem: copy, then remove the upload.
            cp -a "$GAME_FROM" "$GAME" && rm -rf "$GAME_FROM"
        fi
        rm -rf "$GAME.old"
        say "moved into place: $GAME"
    fi
    [ -f "$EXE" ] || die "no game at $GAME (send it with Deploy-FromWindows.ps1, or copy your Dauntless folder there)"
    chown -R "$SERVICE_USER:" "$GAMES"
    say "checking the executable (SHA-256)..."
    hash=$(sha256sum "$EXE" | awk '{ print $1 }')
    [ "$hash" = "$PINNED_EXE_SHA256" ] || die "the game in $GAME is not Dauntless 1.14.7 (its executable's SHA-256 is $hash)"
    say "Dauntless 1.14.7: OK"
}

finish() {
    if [ ! -f "$KIT/package.json" ] || [ ! -f "$CONFIG/host.json" ]; then die "the server is not installed yet: run this script without --finish first"; fi
    place_game
    step "Setup again, with the game in place"
    kit npm run setup -- --skip-install

    step "Runtime"
    if [ -n "$RUNTIME" ]; then
        if [ ! -f "$RUNTIME/MysticParadox.dll" ] || [ ! -f "$RUNTIME/winmm.dll" ]; then die "--runtime $RUNTIME needs MysticParadox.dll and winmm.dll"; fi
        drop=$USER_HOME/.reforged-runtime-upload
        rm -rf "$drop" && mkdir -p "$drop"
        cp "$RUNTIME/MysticParadox.dll" "$RUNTIME/winmm.dll" "$drop/"
        chown -R "$SERVICE_USER:" "$drop"
        kit npm run publish-runtime -- --from "$drop"
        rm -rf "$drop"
    elif [ -f "$PROJECT/.workspace/runtime/runtime.json" ]; then
        # A game sent again comes without the server's copy of the runtime: put the published one back.
        kit cp "$PROJECT/.workspace/runtime/MysticParadox.dll" "$PROJECT/.workspace/runtime/winmm.dll" "$GAME/Archon/Binaries/Win64/"
        say "already published, and in place for the game servers (send a new one with --runtime <folder>)"
    else
        warn "no runtime yet: players can join but not play. Build MysticParadox.dll and winmm.dll on Windows and send them (Deploy-FromWindows.ps1 -Runtime <folder>)"
    fi

    step "Game data"
    if [ -n "$GAMEDATA" ]; then
        [ -f "$GAMEDATA" ] || die "--gamedata $GAMEDATA: no such file"
        drop=$USER_HOME/.reforged-gamedata-upload.json.gz
        cp "$GAMEDATA" "$drop" && chown "$SERVICE_USER:" "$drop"
        kit npm run gamedata -- --import "$drop"
        rm -f "$drop"
    fi
    # Whatever the preset and the pack left missing (the two stores without a pack): the examples, so it starts.
    kit npm run gamedata -- --examples

    if [ "$NO_SERVICES" -eq 1 ] || ! command -v systemctl >/dev/null 2>&1; then
        step "Not starting it (--no-services, or no systemd here)"
        say "Start it with: sh $CONFIG/systemd/install.sh"
        return
    fi
    step "Starting the server (systemd)"
    sh "$CONFIG/systemd/install.sh"

    say "waiting for the backend (up to 3 minutes)..."
    up=0
    i=0
    while [ "$i" -lt 90 ]; do
        if curl -fs -o /dev/null --max-time 3 http://127.0.0.1:61000/dauntless-status; then up=1; break; fi
        i=$((i + 1))
        sleep 2
    done
    if [ "$up" -eq 1 ]; then
        step "Your first invite"
        kit npm run --silent invite -- --for "owner" || warn "could not make an invite: sudo -iu $SERVICE_USER, then cd Reforged/legacy/ReforgedHost && npm run invite"
    else
        warn "the backend did not answer yet: journalctl -u reforged -f shows why"
    fi
    step "Check"
    kit npm run --silent doctor -- --no-hash || true
}

# ---------------------------------------------------------------- main

if [ "$FINISH" -eq 1 ]; then
    finish
else
    arch=$(uname -m)
    [ "$arch" = "x86_64" ] || die "the game servers need an x86-64 VPS (this one is $arch)"
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        os=$(. /etc/os-release && printf '%s %s' "${ID:-?}" "${VERSION_ID:-?}")
        [ "$os" = "debian 13" ] || warn "made for Debian 13; this is $os"
    fi
    if [ "$SKIP_APT" -eq 0 ]; then
        install_packages
        install_mongodb
    fi
    make_user
    get_project
    run_setup
    if [ -f "$EXE" ] && [ "$INSTALL_ONLY" -eq 0 ]; then
        finish
    elif [ "$INSTALL_ONLY" -eq 1 ]; then
        step "Installed"
    else
        step "Installed. Next: the game"
        say "Send Dauntless 1.14.7 from your Windows PC (Deploy-FromWindows.ps1 does it), or copy the Dauntless"
        say "folder to $GAME and run:  sh $KIT/linux/setup-vps.sh --finish"
    fi
fi

# The gateway's port, as host.json has it (443 unless --public-port moved it), and the game ports: 8770-8777
# up to 6 hunts, one more port below 8770 for each hunt above 6 (8766-8777 at 10).
port=443
first=8770
if [ -f "$CONFIG/host.json" ]; then
    port=$(node -e 'try { const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(String(c.publicPort || 443)); } catch { process.stdout.write("443"); }' "$CONFIG/host.json" 2>/dev/null || echo 443)
    first=$(node -e 'try { const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); const h = Math.min(Math.max(Number(c.maxHuntServers) || 3, 1), 10); process.stdout.write(String(Math.min(8770, 8776 - h))); } catch { process.stdout.write("8770"); }' "$CONFIG/host.json" 2>/dev/null || echo 8770)
fi
step "Remember"
say "Allow inbound TCP $port and UDP $first-8777 (and keep TCP 22) in your provider's firewall: the VPS's Firewall page in Hostinger's hPanel."
