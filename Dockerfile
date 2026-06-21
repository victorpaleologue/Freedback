# Multi-stage build for the three Freedback servers.
#
# Storage note: this image uses Oxigraph's in-memory backend (the workspace pins
# `oxigraph` with default-features=false, so RocksDB/Clang are NOT required).
# Data is therefore EPHEMERAL — this is the "one-command demo" image. A durable
# RocksDB build is tracked as future work (see docs/roadmap.md, M10).

FROM rust:1-bookworm AS builder
WORKDIR /app

# Cache dependencies first if the build context allows; here we just copy all.
COPY . .
# Build only the server binaries (clients aren't needed in the server image).
RUN cargo build --release \
    -p freedback-feedback-server \
    -p freedback-discovery-server \
    -p freedback-collection-server

FROM debian:bookworm-slim AS runtime
# ca-certificates: the discovery/collection servers make outbound HTTPS calls.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/freedback-feedback-server   /usr/local/bin/
COPY --from=builder /app/target/release/freedback-discovery-server  /usr/local/bin/
COPY --from=builder /app/target/release/freedback-collection-server /usr/local/bin/
# Ship the ontology so a deployment can serve it locally if desired.
COPY --from=builder /app/ontology /srv/ontology

ENV FREEDBACK_BIND=0.0.0.0:8080
EXPOSE 8080

# Default to the feedback server; compose / `docker run ... <cmd>` overrides this.
CMD ["freedback-feedback-server"]
