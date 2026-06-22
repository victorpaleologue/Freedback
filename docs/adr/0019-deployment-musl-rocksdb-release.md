# ADR 0019 — Deployment: musl static binaries, RocksDB durable backend, release pipeline

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes (partially):** issue #11 (M10 deployment). Core image/compose/Pages
  landed earlier (`191d210`); this covers the deferred build/release/durable
  pieces.
- **Builds on:** ADR 0008 (snapshot persistence), ADR 0005 (storage trait).

## Context

The M10 core shipped a multi-stage `Dockerfile` (in-memory Oxigraph → no
Clang/RocksDB), `docker compose`, and Pages serving the `@context`/ontology at
stable URLs. Three items were explicitly deferred: a static
`x86_64-unknown-linux-musl` build, a durable RocksDB backend, and a tagged
release pipeline that publishes artifacts.

## Decision

### Static musl binaries via `cargo-zigbuild`

The release and CI builds target `x86_64-unknown-linux-musl` with
**`cargo-zigbuild`**, which uses `zig` as the C cross-compiler. The only C in the
default build is `ring` (rustls's crypto); every backend is otherwise pure Rust
(in-memory Oxigraph) and every HTTP client is **rustls**, not OpenSSL (verified:
no `openssl-sys`/`native-tls` in `Cargo.lock`). So a single fully static binary
links with no glibc, no OpenSSL, and no RocksDB — it runs on any x86-64 Linux.
`cargo-zigbuild` was chosen over a `musl-gcc` cross toolchain because zig bundles
the cross sysroot and handles `ring` cleanly with zero host setup.

A `musl` job is added to `ci.yml` so the target stays green and a tag never fails
to cut; `release.yml` reuses it to produce the artifacts.

### Durable RocksDB backend as an opt-in feature

`oxigraph`'s on-disk store (`Store::open`) is gated behind its `rocksdb` feature
(pulls `oxrocksdb-sys`, a C/C++ build). We expose it as:

- `freedback-storage` feature `rocksdb` → `oxigraph/rocksdb`, with
  `OxigraphStore::open(path)` (cfg-gated) returning a durable store that passes
  the same `conformance::run` suite **and** a new "survives reopen" test.
- `freedback-feedback-server` feature `rocksdb` → `freedback-storage/rocksdb`.
  At run time, `FREEDBACK_ROCKSDB_PATH` selects the durable store; writes persist
  directly and the JSON-Lines snapshot loop is skipped. Without the feature the
  variable is ignored with a warning, so the default build is unchanged.
- `Dockerfile` gains `--build-arg FEEDBACK_FEATURES=rocksdb` for a durable image.

The feature is **opt-in, never default**: it requires a C/C++ toolchain and is
native-only (never wasm). The lightweight "one-command demo" image stays
in-memory + snapshot (ADR 0008), so the common path needs no Clang.

### Tagged release pipeline

`release.yml` triggers on `v*` tags: one job builds the musl binaries, one builds
the wasm package (protocol core + cli-client + bundled ontology), and a third
publishes both (with `.sha256` sums) to the GitHub Release via
`softprops/action-gh-release`. `workflow_dispatch` builds the artifacts without
publishing, as a smoke test.

## Consequences

- "CI publishes artifacts on tags" (the remaining M10 acceptance criterion) is
  met; `docker run` and the stable ontology URLs were already satisfied.
- Two build profiles: the default portable in-memory image, and an opt-in
  durable RocksDB image/binary — no Clang imposed on the common case.
- Still **out of scope** (external, not code): the `freedback.net` Pages custom
  domain, pending registrar confirmation (`docs/naming.md`).
