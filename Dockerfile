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
# Opt-in durable storage: `docker build --build-arg FEEDBACK_FEATURES=rocksdb .`
# builds the feedback server with the on-disk Oxigraph/RocksDB backend (selected
# at run time via FREEDBACK_ROCKSDB_PATH). Empty (default) keeps the lightweight
# in-memory demo image.
ARG FEEDBACK_FEATURES=""
# The rocksdb variant runs bindgen (oxrocksdb-sys), which needs libclang — the
# rust image ships gcc/g++ but NOT libclang (confirmed the hard way: the first
# freedback-default deploy died with "Unable to find libclang"). Installed only
# for that variant so the plain in-memory image stays a lean, fast build.
RUN if [ -n "$FEEDBACK_FEATURES" ]; then \
        apt-get update \
        && apt-get install -y --no-install-recommends clang libclang-dev \
        && rm -rf /var/lib/apt/lists/*; \
    fi
# Build only the server binaries (clients aren't needed in the server image).
# The discovery/collection servers have no `rocksdb` feature, so only the
# feedback server takes the optional features.
RUN cargo build --release \
        -p freedback-discovery-server \
        -p freedback-collection-server \
    && cargo build --release -p freedback-feedback-server \
        ${FEEDBACK_FEATURES:+--features "$FEEDBACK_FEATURES"}

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
# Durable demo storage: point the feedback server at a snapshot file under the
# mounted volume to survive restarts (see docs/adr/0008). Unset by default.
RUN mkdir -p /data
VOLUME ["/data"]
EXPOSE 8080

# Default to the feedback server; compose / `docker run ... <cmd>` overrides this.
CMD ["freedback-feedback-server"]
