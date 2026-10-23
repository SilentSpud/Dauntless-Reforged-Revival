#!/usr/bin/env bash
# Dauntless Reforged Revival: the full test run on the built images (CI's docker job, and by hand from the
# repository root: bash docker/run-tests.sh). Builds the front, server and test images, then
#  1. every server test suite inside the test image, which is the server image plus the Rust toolchain
#     (docker/test-in-image.sh): the Rust server crates, with the allowlist's nftables test applied for real in
#     the container's own network namespace, and the Node backend's and Director's tests
#  2. the compose smoke test on the front and server images (docker/smoke-test.sh)
# Arguments pick the in-image suites (rust, backend, director); SKIP_SMOKE=1 leaves out step 2.
set -euo pipefail

image="${REFORGED_IMAGE:-reforged-revival}-test:${REFORGED_TAG:-local}"
export COMPOSE_FILE=compose.yaml

printf '\n== build\n'
docker compose build
docker compose --profile test build test

printf '\n== the test suites in the image\n'
docker run --rm --cap-add NET_ADMIN "$image" "$@"

if [ "${SKIP_SMOKE:-0}" != 1 ]; then
	printf '\n== the smoke test\n'
	SMOKE_NO_BUILD=1 bash docker/smoke-test.sh
fi
echo "full test run passed"
