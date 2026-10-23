#!/usr/bin/env bash
# Dauntless Reforged Revival: the Docker images' smoke test (CI's docker job, and by hand from the repository root:
# bash docker/smoke-test.sh). A public server set up in throwaway volumes on example game data, without a game:
# every service comes up healthy, the gateway serves the certificate setup made, the allowlist's table is in the
# shared namespace, the backend makes an invite, and everything stops. Builds the images unless SMOKE_NO_BUILD=1.
set -euo pipefail

export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-reforged-smoke}"
export COMPOSE_PROFILES=public
export COMPOSE_FILE=compose.yaml
export REFORGED_GATEWAY_PORT=443
export REFORGED_GATEWAY_PUBLISH="127.0.0.1:${SMOKE_PORT:-18443}"
export REFORGED_GAME_PUBLISH_ADDRESS=127.0.0.1
smoke_dir="$(mktemp -d)"
export REFORGED_GAME_DIR="$smoke_dir/game" REFORGED_IMPORT_DIR="$smoke_dir/import"
mkdir -p "$REFORGED_GAME_DIR" "$REFORGED_IMPORT_DIR"

compose() { docker compose "$@"; }
step() { printf '\n== %s\n' "$*"; }
cleanup() {
	status=$?
	if [ "$status" -ne 0 ]; then
		compose ps -a || true
		compose logs --no-color --tail 80 || true
	fi
	compose down --volumes --remove-orphans >/dev/null 2>&1 || true
	rm -rf "$smoke_dir"
	exit "$status"
}
trap cleanup EXIT

if [ "${SMOKE_NO_BUILD:-0}" != 1 ]; then
	step build
	compose build
fi

step setup
compose run --rm host setup --mode public --host smoke.example.org --game-address 203.0.113.7 --name "Smoke Test"
compose run --rm host gamedata --examples

step up
compose up -d --wait --wait-timeout 300
compose ps

step "the gateway serves setup's certificate"
fp_file="$(compose run --rm --entrypoint sh host -c 'cat /opt/reforged/.workspace/tls/gateway-cert.pem')"
served="$(echo | openssl s_client -connect "$REFORGED_GATEWAY_PUBLISH" -servername smoke.example.org 2>/dev/null | openssl x509 -noout -fingerprint -sha256)"
expected="$(printf '%s\n' "$fp_file" | openssl x509 -noout -fingerprint -sha256)"
echo "served:   $served"
echo "expected: $expected"
[ "$served" = "$expected" ]

step "the allowlist's table closes the game ports"
compose exec -T allowlist nft list table inet reforged_revival | tee "$smoke_dir/nft.txt"
grep -q 'udp dport 8770-8777 drop' "$smoke_dir/nft.txt"

step invite
compose run --rm host invite --for "Smoke" | tee "$smoke_dir/invite.txt"
grep -q 'reforged-revival://' "$smoke_dir/invite.txt"

step doctor
compose run --rm host doctor --no-hash || true

step "clean stop"
compose stop
[ -z "$(compose ps --status running --quiet)" ]
echo "smoke test passed"
