# syntax=docker/dockerfile:1
# Builder stage
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml build.rs ./
COPY protobuf ./protobuf/
COPY src ./src/
COPY static ./static/
COPY migrations ./migrations/

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/naughtywolf /usr/local/bin/naughtywolf

USER nobody
EXPOSE 8080

ENTRYPOINT ["naughtywolf", "serve"]
