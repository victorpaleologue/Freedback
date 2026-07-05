# Freedback architecture

Freedback is a **federated feedback protocol**: anyone can attach typed feedback
(stars, scalar, thumbs, comments, tags) to any URI, publish it to a server they
choose, and have it discovered and aggregated across servers — without a central
authority. The wire format is a **W3C Web Annotation** (JSON-LD), so existing
annotation tooling can read it.

## The big picture

```
                         ┌──────────────────────────────────────┐
                         │          protocol-lib (Rust)          │
                         │  model · JCS dedup · P-256 · JSON-LD   │
                         │  · SHACL   (native + wasm32 core)      │
                         └───────────────┬──────────────────────┘
            ┌────────────────────────────┼────────────────────────────┐
            ▼                            ▼                            ▼
   ┌────────────────┐          ┌──────────────────┐         ┌──────────────────┐
   │ feedback-server│  announce│ discovery-server │ resolve │ collection-server│
   │  (WAP container│◀────────▶│  (registry /     │◀───────▶│ (index · cache · │
   │   + /sync)     │          │  .well-known)    │         │  equivalence)    │
   └───────┬────────┘          └──────────────────┘         └────────┬─────────┘
           │ store                                                    │ index
           ▼                                                          ▼
   ┌────────────────┐                                        ┌──────────────────┐
   │  FeedbackStore │  Oxigraph (prod) · SQLite/memory (mock)│  AI equivalence  │
   └────────────────┘                                        │     agent        │
                                                             └──────────────────┘

   clients: cli-client (native+wasm) · advanced-client (local sync copy)
   surfaces: web widgets (JS) · Firefox extension (JS) · 3rd-party WA demo
```

## Components and responsibilities

| # | Component | Crate / dir | Native/WASM | Role |
|---|-----------|-------------|-------------|------|
| — | Protocol core | `protocol-lib` | both | model, dedup id, signing, JSON-LD, SHACL |
| — | Storage | `storage` | native | `FeedbackStore` trait + Oxigraph/SQLite/memory |
| — | Server core | `server-lib` | native | shared WAP semantics + Freedback net-new |
| 1 | Feedback server | `feedback-server` | native | POST-to-container, paging, `/sync`, `/.well-known` |
| 2 | Discovery server | `discovery-server` | native | announce + verify + resolve |
| 3 | Web widgets | `widgets/` | JS (+wasm) | drop-in stars/scalar/thumb/comment/tag |
| 4 | Basic client | `cli-client` | both | read/write/sync; collection vs publication points |
| 5 | Interop demo | `demo-third-party/` | JS | load Freedback output in Annotorious/RecogitoJS |
| 6 | Advanced client | `advanced-client` | native | local sync copy + resume cursor + dedup-on-merge |
| 7 | Collection server | `collection-server` | native | cache, per-URI index, equivalence, politeness |
| 8 | Equivalence agent | `agent-prompts/` | native job | propose URI equivalences for component 7 |
| 9 | Firefox extension | `firefox-extension/` | JS (+wasm) | list feedback for the current page |

## Two identities, two trust models (INVARIANT 4)

- **Self-signed P-256** — the public key (PEM) is the portable issuer id. Every
  annotation carries a detached ES256 signature over its RFC 8785 canonical
  bytes. This identity **federates**: any server can verify it with no shared
  secret. It is also the **ownership credential**: signed edits supersede
  (`(issuer, target)`, newest wins), and signed deletes actually erase — the
  author's right to be forgotten (ADR 0021). Only a content-free tombstone
  remains, so caches learn to forget and the id cannot be re-ingested.
- **App-managed OAuth** — keyed by `(app_id, user_id)`. Creates a
  **local-authority silo**: trustworthy only within that app's domain; it does
  **not** federate. Useful when an app already owns its users.

## Data lifecycle

```
write:  build Annotation → (optional) P-256 sign → POST /annotations/
        → auth (verify JWS  OR  OAuth bearer→(app,user))
        → JSON-LD expand → SHACL validate (reject→422+report)
        → FeedbackStore::put (dedup by content id)

read:   GET /annotations/?target=&page=  → FeedbackStore::query
        → JSON-LD frame/compact to pinned @context → OrderedCollectionPage

sync:   GET /sync?target=&gt_iat=&latest_edits_only=true
        → only items with iat > cursor, edit-chains collapsed to latest

delete: DELETE /annotations/{dedup_id} + author proof
        (ES256 over JCS of {"type":"Delete","annotation":id,"created":ts}
         by the annotation's own key  OR  OAuth bearer→same (app,user))
        → content erased; content-free tombstone {dedup_id, deleted_at, proof}
        → GET/re-POST of that id → 410 Gone; caches evict on next sync
```

## Why these choices

The non-obvious decisions are written up as ADRs in [`docs/adr/`](./adr):

- [0001 — Rust workspace & Web Annotation wire format](./adr/0001-rust-workspace-and-wire-format.md)
- [0002 — Content-addressed dedup id via RFC 8785 JCS](./adr/0002-dedup-id-jcs.md)
- [0003 — Self-signed P-256 identity (federating)](./adr/0003-dual-identity-p256.md)
- [0004 — All validation in SHACL, never OWL/RDFS](./adr/0004-validation-in-shacl.md)
- [0005 — Storage behind a trait; Oxigraph primary](./adr/0005-storage-trait-oxigraph.md)
- [0006 — Discovery: flat list first, resolver second](./adr/0006-discovery-flat-then-resolver.md)
- [0007 — JSON-LD is primary on ingest, not interop](./adr/0007-jsonld-primary-ingest.md)
- [0008 — Durable demo storage via JSON-Lines snapshots](./adr/0008-snapshot-persistence.md)
- [0009 — Custom rating scales via `sh:lessThanOrEquals`](./adr/0009-custom-rating-scales.md)
- [0010 — JWT export profile (`PUT /submit/{jwt}`)](./adr/0010-jwt-export-profile.md)
- [0011 — Full JSON-LD via compaction against the pinned context](./adr/0011-full-jsonld-compaction.md)
- [0012 — HTTP cache freshness + validators (collection ↔ feedback)](./adr/0012-cache-freshness-and-validators.md)
- [0013 — WebCrypto P-256 signing in the widgets](./adr/0013-webcrypto-widget-signing.md)
- [0014 — NIP-65-style relay list (outbox discovery)](./adr/0014-nip65-relay-list-resolver.md)

See [`roadmap.md`](./roadmap.md) for milestones and the issue map, and
[`attributions.md`](./attributions.md) for harvested-code provenance.
