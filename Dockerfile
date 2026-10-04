# Multi-stage build for axmg
FROM rust:1-slim as builder

WORKDIR /usr/src/axmg
COPY . .

RUN cargo build --release --bin harness

# Minimal runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/axmg/target/release/harness /usr/local/bin/axmg-harness

LABEL org.opencontainers.image.source="https://github.com/dchrnv/axmg"
LABEL org.opencontainers.image.description="axmg — Universal Merkle DAG Associative Memory Engine"
LABEL org.opencontainers.image.licenses="AGPL-3.0"

ENTRYPOINT ["axmg-harness"]
