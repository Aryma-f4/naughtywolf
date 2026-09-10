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

FROM sources AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 naughtywolf \
    && useradd --uid 10001 --gid naughtywolf --create-home naughtywolf \
    && mkdir -p /data/evidence /data/payloads /app/target \
    && chown -R naughtywolf:naughtywolf /data /app /usr/local/cargo /usr/local/rustup

COPY --from=builder /app/target/release/naughtywolf /usr/local/bin/naughtywolf

ENV NAUGHTYWOLF_BIND=0.0.0.0:8080 \
    NAUGHTYWOLF_DATABASE_URL=sqlite:/data/naughtywolf.db?mode=rwc \
    NAUGHTYWOLF_EVIDENCE_DIR=/data/evidence \
    NAUGHTYWOLF_COOKIE_SECURE=true \
    RUST_LOG=info
WORKDIR /data
USER naughtywolf
EXPOSE 8080 4630
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent --show-error --max-time 4 http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["naughtywolf"]
CMD ["serve"]
