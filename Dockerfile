# syntax=docker/dockerfile:1.7
#
# mcp-ableton-music-maker: MCP server for Ableton Live, run over stdio by an MCP client
# (Claude Desktop, Claude Code, Cursor). Live itself runs on the host; the
# container only makes an outbound TCP connection to it on port 9877.
#
# Stages:
#   builder  - compiles the release binaries
#   test     - builder plus `cargo test`; building this stage runs the suite
#   runtime  - distroless (no shell, no package manager) + the two binaries,
#              non-root, telemetry hard-off
#
# Build:   docker build -t mcp-ableton-music-maker:local .
# Test:    docker build --target test .
# Verify:  docker/verify-image.sh mcp-ableton-music-maker:local

ARG RUST_VERSION=1

# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

WORKDIR /build
ENV CARGO_TERM_COLOR=never

# Dependencies first, so source edits do not re-download or rebuild them.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && \
    echo 'pub fn placeholder() {}' > src/lib.rs && \
    echo 'fn main() {}' > src/bin/ableton-music-maker.rs && \
    echo 'fn main() {}' > src/bin/ableton-music-maker-install-script.rs && \
    printf '# placeholder\n' > README.md && \
    mkdir -p AbletonMusicMaker_Remote_Script && \
    printf 'SCRIPT_VERSION = "0.0.0"\nSCRIPT_CAPABILITIES = []\n' > AbletonMusicMaker_Remote_Script/__init__.py
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked && rm -rf src

# Then the real sources. The Remote Script is embedded into the binary here.
COPY src/ ./src/
COPY tests/ ./tests/
COPY AbletonMusicMaker_Remote_Script/ ./AbletonMusicMaker_Remote_Script/
COPY README.md ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    touch src/lib.rs src/bin/*.rs && \
    cargo build --release --locked && \
    mkdir -p /out && \
    cp target/release/ableton-music-maker target/release/ableton-music-maker-install-script /out/

# ---------------------------------------------------------------------------
FROM builder AS test

ENV ABLETON_MCP_DISABLE_TELEMETRY=true \
    ABLETON_MCP_DISABLE_DATASET=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    touch src/lib.rs src/bin/*.rs && \
    cargo test --release --locked

# ---------------------------------------------------------------------------
# distroless/cc: glibc, libgcc and CA certificates, nothing else. No shell,
# no package manager, runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="ableton-music-maker" \
      org.opencontainers.image.description="Ableton Live integration through the Model Context Protocol" \
      org.opencontainers.image.source="https://github.com/defoAI/mcp-ableton-music-maker" \
      org.opencontainers.image.licenses="MIT"

COPY --from=builder --chown=nonroot:nonroot /out/ableton-music-maker /out/ableton-music-maker-install-script /app/

# Defaults. Any of these can be overridden with `docker run -e`.
#   ABLETON_HOST / ABLETON_PORT   where Live is, from inside the container
#   *_DISABLE_*                   telemetry and dataset upload are hard-off;
#                                 the server then opens no socket to anything
#                                 except Live
#   ABLETON_MCP_STATE_DIR / ABLETON_MCP_DATA_DIR / HOME
#                                 every path the server can write is under
#                                 /state, so the root filesystem can be
#                                 mounted read-only
ENV ABLETON_HOST=host.docker.internal \
    ABLETON_PORT=9877 \
    ABLETON_MCP_DISABLE_TELEMETRY=true \
    ABLETON_MCP_DISABLE_DATASET=true \
    ABLETON_MCP_STATE_DIR=/state \
    ABLETON_MCP_DATA_DIR=/state \
    HOME=/state \
    RUST_LOG=info

WORKDIR /app
VOLUME ["/state"]

# No HEALTHCHECK: the server speaks MCP over stdio and exposes no port.
ENTRYPOINT ["/app/ableton-music-maker"]
