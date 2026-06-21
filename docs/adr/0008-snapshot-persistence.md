# ADR 0008 — Durable demo storage via JSON-Lines snapshots

- **Status:** accepted
- **Date:** 2026-06-21

## Context

The default container uses Oxigraph's **in-memory** backend (ADR 0005 /
deployment), chosen so the image needs no Clang/RocksDB and stays a clean single
binary. The cost: data is lost on restart. That is fine for a throwaway demo but
surprising for anyone who runs `docker compose up`, posts feedback, and restarts.

A full durable backend (RocksDB, or a transactional WAL) is the long-term answer
but reintroduces the build complexity we deliberately avoided.

## Decision

Add **snapshot persistence** on top of any `FeedbackStore`, independent of the
backend:

- `FeedbackStore::dump_jsonl(path)` writes every stored annotation as one JSON
  line; `load_jsonl(path)` re-`put`s them (idempotent by content id). These are
  **default trait methods** built on `query` + `put`, so both the in-memory and
  Oxigraph backends get persistence for free, and they're exercised by the shared
  `conformance::persistence` suite.
- The feedback server, when `FREEDBACK_STORE_PATH` is set, **loads on boot**,
  **re-snapshots every 60 s**, and **snapshots on graceful shutdown**
  (Ctrl-C / SIGTERM). The compose stack mounts a named volume and sets the path,
  so the demo now survives restarts.

## Why JSON-Lines (not Oxigraph's RDF dump)

- It is backend-agnostic — the same code persists the memory mock and Oxigraph,
  and would persist a future SQLite backend.
- It reuses the existing `query`/`put` path, so there's no second serialization
  to keep in sync (annotations are already content-addressed JSON-LD).
- It's trivially inspectable and diffable.

## Consequences

- **Not transactional.** A crash between snapshots loses up to ~60 s of writes,
  and a write during a dump is simply caught by the next dump. This is the
  honest posture for a demo image; durability-critical deployments should use a
  real backend (tracked: RocksDB feature / WAL).
- The snapshot is `put`-idempotent, so re-loading or merging snapshots is safe.
- No new runtime dependency; `FREEDBACK_STORE_PATH` is opt-in (unset = the old
  ephemeral behavior).
