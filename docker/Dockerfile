# syntax=docker/dockerfile:1
#
# Dauntless Reforged Revival server images (docs/hosting-docker.md). Built from the repository root:
#   docker compose build            (compose.yaml), or
#   docker build -f docker/Dockerfile --target server .
#
# Two images, both laid out as the server release is (RD8, .github/workflows/server-release.yml), under
# /opt/reforged, so every path reforged-host writes into the env files is the same in every container:
#   front   the internet-facing Rust services on Debian slim: gateway, allowlist (with nftables), content
#   server  reforged-host, the Node backend and Director (until RB14), Wine for the game servers, and
#           MongoDB's dump tools for `reforged-host backup`; also the image the host kit's commands run in
#
# When RB14 ships, backend and director become Rust bins built in rust-build, the Node stage goes, and the
# backend and Director move to a slim image like front; Wine stays with the Director.

ARG RUST_IMAGE=rust:1-slim-trixie
ARG NODE_IMAGE=node:22-trixie-slim
ARG DEBIAN_IMAGE=debian:trixie-slim
ARG MONGO_IMAGE=mongo:8.0

# ------------------------------------------------------------------ the Rust binaries
FROM ${RUST_IMAGE} AS rust-build
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src/rust
# The pinned toolchain (rust-toolchain.toml), in its own layer
COPY rust/rust-toolchain.toml ./
RUN rustup show active-toolchain || rustup toolchain install
COPY rust/ ./
# The release workflow's build: the host kit, gateway (and allowlist) and content server. The launcher is a
# workspace member but is not built (only the lock file is read). backend and director join once they are real.
RUN --mount=type=cache,id=reforged-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=reforged-cargo-target,target=/src/rust/target \
    cargo build --release --locked -p host -p gateway -p content --bins \
 && mkdir -p /out/bin \
 && for bin in reforged-host gateway allowlist content; do cp "target/release/$bin" /out/bin/; done

# ------------------------------------------------------------------ the Node services (until RB14)
FROM ${NODE_IMAGE} AS node-build
WORKDIR /src
# Every dependency, dev ones included, as the server release has them: gamedata runs Mystic Paradox's
# generators, some with tsx. The lock files first, so a source change does not reinstall.
COPY paradox/backend/package.json paradox/backend/package-lock.json paradox/backend/
COPY paradox/director/package.json paradox/director/package-lock.json paradox/director/
COPY legacy/ReforgedGateway/package.json legacy/ReforgedGateway/package-lock.json legacy/ReforgedGateway/
RUN --mount=type=cache,id=reforged-npm,target=/root/.npm \
    (cd paradox/backend && npm ci --no-audit --no-fund) \
 && (cd paradox/director && npm ci --no-audit --no-fund) \
 && (cd legacy/ReforgedGateway && npm ci --omit=dev --no-audit --no-fund)
COPY paradox/ paradox/
COPY legacy/ReforgedGateway/ legacy/ReforgedGateway/
RUN (cd paradox/backend && npm run build) \
 && (cd paradox/director && npm run build) \
 && find . -type d -name .bin -path '*/node_modules/*' -prune -exec rm -rf {} +

# ------------------------------------------------------------------ front: gateway, allowlist, content
FROM ${DEBIAN_IMAGE} AS front
# nftables for the allowlist (ALLOWLIST_NFT is /usr/sbin/nft). The helper runs as the reforged user with
# CAP_NET_ADMIN in its bounding set (compose.yaml); nft gets it from its file capability, nothing else does.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates nftables libcap2-bin \
 && setcap cap_net_admin+ep /usr/sbin/nft \
 && apt-get purge -y libcap2-bin && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 reforged \
 && useradd --uid 1000 --gid 1000 --home-dir /opt/reforged --no-create-home --shell /usr/sbin/nologin reforged \
 && mkdir -p /opt/reforged/.workspace && chown reforged:reforged /opt/reforged/.workspace
COPY --from=rust-build /out/bin/gateway /out/bin/allowlist /out/bin/content /opt/reforged/bin/
COPY --chmod=0755 docker/healthcheck.sh /opt/reforged/bin/reforged-healthcheck
ENV PATH=/opt/reforged/bin:$PATH
WORKDIR /opt/reforged/.workspace
USER reforged
LABEL org.opencontainers.image.title="Dauntless Reforged Revival front door" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/SilentSpud/Dauntless-Reforged-Revival"

# ------------------------------------------------------------------ server: host kit, backend, Director
FROM ${MONGO_IMAGE} AS mongo-tools

FROM ${NODE_IMAGE} AS server
# Wine and a virtual display for the Windows game servers (docs/hosting-linux.md); tini reaps them.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates wine wine64 xvfb xauth tini \
 && rm -rf /var/lib/apt/lists/*
# `reforged-host backup` runs mongodump; the MongoDB image's tools are static binaries
COPY --from=mongo-tools /usr/bin/mongodump /usr/bin/mongorestore /usr/bin/
# The node user (uid 1000) owns everything a host writes: the workspace and the game-data folders, which
# compose.yaml mounts as volumes (a new volume takes this ownership and these files).
WORKDIR /opt/reforged
COPY --chown=node:node LICENSE.txt NOTICE.md ADDITIONAL_TERMS.md README.md ./
COPY --chown=node:node preset/ preset/
COPY --chown=node:node native/ native/
COPY --chown=node:node legacy/ReforgedHost/ legacy/ReforgedHost/
COPY --chown=node:node --from=node-build /src/legacy/ReforgedGateway/ legacy/ReforgedGateway/
COPY --chown=node:node --from=node-build /src/paradox/ paradox/
COPY --from=rust-build /out/bin/ bin/
COPY --chmod=0755 docker/director-entrypoint.sh bin/reforged-director
COPY --chmod=0755 docker/host-entrypoint.sh bin/reforged-host-docker
COPY --chmod=0755 docker/healthcheck.sh bin/reforged-healthcheck
RUN mkdir -p .workspace paradox/Items_Analysis paradox/DauntlessEndpointDocumentation \
 && chown node:node .workspace paradox/Items_Analysis paradox/DauntlessEndpointDocumentation
# The commit the backend names to players (REFORGED_SOURCE_COMMIT, the AGPL source offer). setup reads it from
# git or release.json, neither of which is in the image; the process environment wins over the env file.
ARG REFORGED_COMMIT=""
ENV REFORGED_SOURCE_COMMIT=${REFORGED_COMMIT} \
    PATH=/opt/reforged/bin:$PATH
USER node
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["reforged-host", "--help"]
LABEL org.opencontainers.image.title="Dauntless Reforged Revival server" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/SilentSpud/Dauntless-Reforged-Revival"

# ------------------------------------------------------------------ test: every server suite on the server image
# The server image with the Rust toolchain, the workspace's sources and its tests compiled in, so a test run
# (docker/test-in-image.sh, `docker compose run --rm test`, docker/run-tests.sh) checks the code on the same OS,
# Node, Wine and paths production runs on. Never deployed.
FROM server AS test
USER root
# procps: the host kit's start tests send Ctrl+C with kill(1), which the slim base images leave out
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config git nftables libcap2-bin procps \
 && setcap cap_net_admin+ep /usr/sbin/nft \
 && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /usr/local/cargo /usr/local/cargo
COPY --from=rust-build /usr/local/rustup /usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:/opt/reforged/bin:$PATH \
    CARGO_TERM_COLOR=never
COPY --chown=node:node rust/ rust/
COPY --chmod=0755 docker/test-in-image.sh bin/reforged-test
RUN chown node:node rust && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
USER node
# The server crates' tests, built now so a run starts at once (the downloaded crates stay in the image: a run
# rebuilds nothing and needs no network). The launcher (Tauri, WebKitGTK) is not a server crate and is not
# built; the fuzz targets are a workspace of their own.
RUN cd rust && cargo test --locked --no-run \
      -p protocol -p proxy -p service-kit -p store -p gateway -p content -p host
ENTRYPOINT ["/usr/bin/tini", "--", "reforged-test"]
CMD []
