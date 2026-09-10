# syntax=docker/dockerfile:1
# Keep the toolchain and sources for the portal's existing runtime payload builder.
FROM rust:1.90-bookworm AS sources
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src
COPY static ./static
COPY migrations ./migrations

FROM sources AS builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --locked --release -p naughtywolf --bin naughtywolf

# Build the real Rust musl implant once so the container smoke test can execute
# the same startup path used by payload artifacts. Rust supplies the final
# self-contained linker; musl-gcc remains available only for C dependencies.
FROM sources AS musl-smoke-builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl
ENV CC_x86_64_unknown_linux_musl=musl-gcc
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    NW_ENDPOINT=http://127.0.0.1:8080 \
    NW_PSK=container-smoke-only \
    NW_INTERVAL=250 \
    NW_JITTER=0 \
    cargo build --locked --release -p nw-implant --target x86_64-unknown-linux-musl

FROM sources AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils-mingw-w64-x86-64 \
        ca-certificates \
        curl \
        gcc-mingw-w64-x86-64 \
        musl-tools \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-gnu x86_64-unknown-linux-musl \
    && groupadd --gid 10001 naughtywolf \
    && useradd --uid 10001 --gid naughtywolf --create-home naughtywolf \
    && mkdir -p /data/evidence /data/payloads /app/target \
    && chown -R naughtywolf:naughtywolf /data /app /usr/local/cargo /usr/local/rustup

COPY --from=builder /app/target/release/naughtywolf /usr/local/bin/naughtywolf
COPY --from=musl-smoke-builder /app/target/x86_64-unknown-linux-musl/release/nw-implant /usr/local/libexec/nw-implant-musl-smoke

ENV NAUGHTYWOLF_BIND=0.0.0.0:8080 \
    NAUGHTYWOLF_DATABASE_URL=sqlite:/data/naughtywolf.db?mode=rwc \
    NAUGHTYWOLF_EVIDENCE_DIR=/data/evidence \
    NAUGHTYWOLF_COOKIE_SECURE=true \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    RUST_LOG=info
WORKDIR /data
USER naughtywolf
EXPOSE 8080 4630
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent --show-error --max-time 4 http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["naughtywolf"]
CMD ["serve"]
