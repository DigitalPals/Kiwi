# syntax=docker/dockerfile:1
#
# Kiwi in a container: the server, its web app and the Python that Kiwi Builds' Android
# runner needs, on Debian 13. Kiwi listens on 127.0.0.1:8742 inside the container's network
# namespace; compose.yaml runs it next to a Tailscale container that publishes it on your
# tailnet. See docs/install.md ("Docker").
#
#   docker build -t kiwi .

# --- the server -----------------------------------------------------------------------------
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS server
WORKDIR /src
# The image's toolchain is the one rust-toolchain.toml pins (as in CI).
COPY rust-toolchain.toml ./
RUN rustup toolchain install && rustc --version
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The server embeds Kiwi Builds' runners and recipes.
COPY automation automation
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p kiwi-server \
    && install -D target/release/kiwi /out/kiwi

# --- the web app ----------------------------------------------------------------------------
FROM node:24-trixie-slim@sha256:0820eaa85e07d0490446dd01d14707e17e6108a36988cf374899cd057eb3812d AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
# Vite's config loads the mock API, which reads the recipes.
COPY automation/recipes.json automation/android.json /src/automation/
COPY web ./
RUN npm run build

# --- Kiwi -----------------------------------------------------------------------------------
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
LABEL org.opencontainers.image.title="Kiwi" \
      org.opencontainers.image.description="A private, self-hosted app store for your own iPhones and Android phones" \
      org.opencontainers.image.source="https://github.com/DigitalPals/Kiwi" \
      org.opencontainers.image.licenses="MIT"
# python3 (3.13) runs Kiwi Builds' Android runner, which fetches its own pinned Java.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates python3 tini \
    && rm -rf /var/lib/apt/lists/*
# A fixed uid, so a bind-mounted data directory can be given to it (chown 10001:10001).
RUN groupadd --system --gid 10001 kiwi \
    && useradd --system --uid 10001 --gid kiwi --home-dir /data --no-create-home \
       --shell /usr/sbin/nologin kiwi \
    && install -d -o kiwi -g kiwi -m 700 /data
COPY --from=server /out/kiwi /opt/kiwi/kiwi
# Kiwi finds its web app next to the program (<exe dir>/web).
COPY --from=web /src/web/dist /opt/kiwi/web
ENV KIWI_DATA_DIR=/data \
    RUST_LOG=info \
    NO_COLOR=1
VOLUME /data
WORKDIR /data
USER kiwi
# /api/health on the listen address (requests must name it as their host).
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD ["python3", "-c", "import os, urllib.request as u; u.build_opener(u.ProxyHandler({})).open('http://%s/api/health' % os.environ.get('KIWI_LISTEN', '127.0.0.1:8742'), timeout=4)"]
# tini reaps the processes Kiwi Builds starts; Kiwi itself stops gracefully on SIGTERM. The
# shell sets umask 077, as kiwi.service does, so the database, builds, downloads and signing
# keys Kiwi writes are private to the kiwi user, then replaces itself with Kiwi.
ENTRYPOINT ["/usr/bin/tini", "--", "/bin/sh", "-c", "umask 077 && exec /opt/kiwi/kiwi \"$@\"", "kiwi"]
