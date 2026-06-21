# ADR 0015 — Storage durability: SQLite mock + persistent collection state

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** issue #23 (the M2 "SQLite mock deferred" item and the ADR 0012
  "persistent index/cache across restarts remains future work" item).
- **Not in scope:** the durable RocksDB feedback-server backend (issue #11).

## Context

Two pieces of durability were deferred:

1. **M2** shipped the in-memory and Oxigraph `FeedbackStore` backends and noted a
   SQLite mock as pending. The roadmap and CLAUDE.md both call for a SQLite mock
   as a durable, dependency-light store.
2. **M6 / ADR 0012** made the collection server polite (per-`(server, uri)` cache,
   freshness, validators) but its derived state — registered servers, the cache,
   and the URI equivalence union-find — was purely in-memory and lost on restart.
   A cold aggregator re-fans-out to every upstream and forgets every asserted
   equivalence.

## Decision

### 1. SQLite `FeedbackStore` (`freedback-storage`, feature `sqlite`)

`SqliteStore` (rusqlite, `bundled` so no system SQLite) stores each annotation as
one row keyed by its dedup id, with `target` / `issuer` / `iat` denormalized for
SQL filtering/ordering and the raw JSON-LD kept verbatim. `INSERT OR IGNORE` on
the dedup-id primary key gives idempotent `put`. It passes the **shared
`conformance::run` suite** identically to the memory/Oxigraph backends, plus the
`conformance::persistence` snapshot suite and a file-reopen durability test.

It is gated behind the **`sqlite` feature** (off by default) and pulls
`rusqlite` only as an `optional` dependency. rusqlite is a native-only C dep, so
the gate keeps it out of any wasm consumer (INVARIANT 5/6). Storage already
depends on Oxigraph and is native-only, but the feature gate is the contract.

### 2. Persistent collection-server state (`redb`)

A new `persist` module backs `AppState`'s `servers` / `cache` / `equivalence`
with a single embedded **redb** database (pure Rust, the same KV the advanced
client uses — no Clang/RocksDB, wasm-capable though used native-only here),
write-through on every mutation:

- **servers** — a set table, written on `add_server`.
- **equivalence** — an *append-only log* of asserted `(a, b, proof)` unions,
  replayed in order on boot to rebuild the union-find. We store the proofs (the
  audit trail), not the collapsed parent map, so the structure stays mergeable
  and inspectable.
- **cache** — per-`(server, uri)` entries as JSON, written on every 200/304.
  The freshness deadline (`fresh_until`, an `Instant`) is intentionally **not**
  persisted: an `Instant` is meaningless across a process restart, and a
  reloaded entry *should* be treated as stale so the first post-restart read
  **revalidates** (cheap `304`) rather than serving possibly-stale data
  unchecked. The validators (`ETag` / `Last-Modified`) and last items *are*
  persisted, so that first revalidation is conditional.

Persistence is **opt-in**: `AppState::with_persistence(base, rate, path)` (wired
to `FREEDBACK_STATE_PATH` in the binary). Unset ⇒ the original ephemeral
in-memory behavior; no behavior change for existing callers/tests.

## Why redb (not SQLite) for the collection server

- It is the codebase's existing embedded KV (advanced client), pure-Rust, no C
  toolchain — consistent with the polite, single-binary collection server.
- The collection state is a small set + log + blob map: a KV fits it directly
  with no schema/SQL ceremony. SQLite would have worked too; redb minimizes new
  surface and matches a precedent.

## Consequences

- `cargo test --all-features` now exercises `sqlite` (the CI gate is all-features),
  so the SQLite backend is conformance-checked on every run.
- A collection-server restart resumes its servers, equivalence class, and cached
  pages; the `persisted_state_survives_restart` cluster test asserts exactly this
  across a real stop/reopen of the redb file.
- `FeedbackStore`'s trait signature is **unchanged** — the SQLite store is purely
  additive.
- Still not transactional across a crash mid-write at the byte level beyond what
  each backend guarantees; both rusqlite and redb are individually durable per
  committed write, which is the bar for this scope.
