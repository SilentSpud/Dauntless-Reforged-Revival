#!/bin/sh
# Dauntless Reforged Revival: the Director's container (compose.yaml). Does what `reforged-host start` does
# before the Director on Linux (rust/services/host/src/commands/start/mod.rs: preflight, linux_steps), then
# runs the Node Director with its env file. The Director starts each game server through
# .workspace/bin/reforged-gameserver (`reforged-host gameserver`, Wine on a virtual display), in this
# container, so the game servers stop with it.
set -eu

root=/opt/reforged
workspace="$root/.workspace"
env_file="$workspace/config/director.env"

if [ ! -f "$env_file" ] || [ ! -x "$workspace/bin/reforged-gameserver" ]; then
	echo "director | not set up yet: docker compose run --rm host setup ... (docs/hosting-docker.md)" >&2
	exit 78
fi

# The first start of Wine makes its prefix, a minute or more: done here rather than inside the first game
# server's readiness timeout. setup usually made it already (the same image, the same volume).
prefix="$workspace/wine"
if [ ! -f "$prefix/system.reg" ]; then
	echo "director | making the Wine prefix (once, a minute or two)"
	mkdir -p "$prefix"
	WINEPREFIX="$prefix" WINEDEBUG=-all WINEDLLOVERRIDES="winmm=n,b;mscoree=d;mshtml=d" \
		xvfb-run -a wine wineboot --init >"$workspace/logs/wine.log" 2>&1 || true
	WINEPREFIX="$prefix" wineserver -w || true
	[ -f "$prefix/system.reg" ] || echo "director | ! wineboot did not make the prefix (see .workspace/logs/wine.log)" >&2
fi

# The runtime switches the game servers need beside the executable (publish.rs, SERVER_RUNTIME_FLAGS)
exe="$(sed -n 's/^REFORGED_GAME_EXE=//p' "$env_file" | tr -d '"')"
if [ -n "$exe" ] && [ -f "$exe" ]; then
	win64="$(dirname "$exe")"
	[ -e "$win64/CALL_HUNTPASS_QUERY.flag" ] || : >"$win64/CALL_HUNTPASS_QUERY.flag" || true
else
	echo "director | ! no game executable at ${exe:-(none)}: the Director runs, but no game server can start." >&2
	echo "director |   Mount your Dauntless 1.14.7 folder (REFORGED_GAME_DIR) and run setup with --game /game" >&2
fi

cd "$root/paradox/director"
exec node --env-file="$env_file" dist/server.js
