#!/bin/bash
# Dauntless Reforged Revival: compose.yaml's health check for a service, `reforged-healthcheck <name>`.
# Healthy once its port takes a connection, the port and address read as the service reads them: the
# process environment first, then its env file (written by reforged-host setup). The host kit's `start`
# waits the same way (rust/services/host/src/services.rs, Ready).
set -u
name="${1:?usage: reforged-healthcheck gateway|content|admin|allowlist|backend|director}"
file="/opt/reforged/.workspace/config/$name.env"

setting() {
	local value="${!1-}"
	if [ -z "$value" ] && [ -f "$file" ]; then
		value="$(sed -n "s/^$1=//p" "$file" | tail -n 1)"
		value="${value#\"}"
		value="${value%\"}"
	fi
	printf '%s' "${value:-$2}"
}

case "$name" in
	gateway) host="$(setting GATEWAY_BIND 0.0.0.0)" port="$(setting GATEWAY_PORT 443)" ;;
	allowlist) host="$(setting ALLOWLIST_BIND 127.0.0.1)" port="$(setting ALLOWLIST_PORT 61005)" ;;
	content) host="$(setting BIND_HOST 127.0.0.1)" port="$(setting PORT 61002)" ;;
	admin) host="$(setting BIND_HOST 127.0.0.1)" port="$(setting PORT 61003)" ;;
	backend) host="$(setting BIND_HOST 127.0.0.1)" port="$(setting PORT 61000)" ;;
	director) host="$(setting BIND_HOST 127.0.0.1)" port="$(setting PORT 61001)" ;;
	*) echo "unknown service $name" >&2; exit 2 ;;
esac
[ "$host" = 0.0.0.0 ] && host=127.0.0.1
exec 3<>"/dev/tcp/$host/$port"
