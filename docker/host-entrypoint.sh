#!/bin/sh
# Dauntless Reforged Revival: `docker compose run --rm host <command>` (compose.yaml's host service) runs
# reforged-host inside the server image, in the server's network, with its volumes. Every command works as on a
# Linux host (docs/hosting-linux.md), with these differences (docs/hosting-docker.md):
#  - setup: the services are in the image already (--skip-install), MongoDB is the mongo service
#    (--no-manage-mongo, the replica set at 127.0.0.1:27017), and the game folder is the one mounted at /game.
#    Each is added only when the command line does not say otherwise.
#  - start and update are compose's: `docker compose up -d` runs the server, a new image updates it.
set -eu

has() {
	want="$1"
	shift
	for arg in "$@"; do
		case "$arg" in "$want" | "$want"=*) return 0 ;; esac
	done
	return 1
}

command="${1:-}"
case "$command" in
start)
	echo "In Docker the services are compose's: docker compose up -d (docker compose logs -f shows them)." >&2
	exit 2
	;;
update)
	echo "In Docker a new version is a new image: git pull && docker compose build && docker compose up -d" >&2
	echo "(then docker compose run --rm host setup, which keeps your settings, if the release notes say so)." >&2
	exit 2
	;;
setup)
	shift
	has --skip-install "$@" || set -- "$@" --skip-install
	if ! has --manage-mongo "$@" && ! has --no-manage-mongo "$@" && ! has --mongo-uri "$@"; then
		set -- "$@" --no-manage-mongo --mongo-uri "mongodb://127.0.0.1:27017/?replicaSet=rs0"
	fi
	if ! has --game "$@" && [ -d /game/Archon ]; then
		set -- "$@" --game /game
	fi
	reforged-host setup "$@"
	echo
	echo "In Docker (docs/hosting-docker.md): ignore the systemd and ufw steps above; compose runs the server."
	echo "  docker compose up -d                 starts it (COMPOSE_PROFILES=public in .env for a public server)"
	echo "  docker compose run --rm host invite --for \"Alex\""
	exit 0
	;;
esac
exec reforged-host "$@"
