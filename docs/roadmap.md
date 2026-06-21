# Roadmap & issue map

This is the project decomposed into issues with their dependencies. Each issue
maps to a GitHub issue (the epic links them all). Status reflects this branch.

## Dependency graph (milestones)

```
M1 protocol-lib core ─┬─► M2 storage ─┬─► M3 feedback-server ─┬─► M4 cli-client
   (done)             │               │                      ├─► M5 discovery-server ─► M6 collection-server ─► M7 advanced-client
                      │               │                      └─► (interop) ───────────────────────────────────► M8 widgets/extension/demo
                      └───────────────┴─────────────────────────────────────────────────────────────────────► M9 equivalence agent
                                                                                                                M10 deployment + Pages
```

## Milestones / issues

### M1 — `protocol-lib` core ✅ (done on this branch)
Model, `@context`/ontology/shapes, JCS dedup id, P-256 sign/verify, RDF mapping,
shapes-driven SHACL validation. Compiles native **and** wasm32.
- **Depends on:** —
- **Acceptance:** dual-target build green; signing tamper-rejected; dedup id
  stable & content-sensitive; SHACL rejects out-of-bounds bodies. ✅

### M2 — storage trait + mocks ✅ (memory + Oxigraph; SQLite pending)
`FeedbackStore` trait; in-memory mock (fast tests); Oxigraph impl (primary,
in-memory backend); optional SQLite. Put/query/dedup/sync semantics.
- **Depends on:** M1
- **Acceptance:** put is idempotent by dedup id; `query` pages; `sync(gt_iat)`
  returns strictly newer; `latest_edits_only` collapses per (issuer, target). ✅
  (shared `conformance::run` suite; both backends green. SQLite mock deferred.)

### M3 — feedback-server (axum) ✅ [#component-1]
POST-to-container (WAP), paginated reads (`AnnotationPage` + `Link` rels),
`/sync` cursor, dual auth (JWS + OAuth), `/.well-known/freedback`, SHACL-reject
→ 422 with report.
- **Depends on:** M2
- **Acceptance:** signed-payload tamper rejected (401); SHACL-invalid → 422 +
  report; identical re-POST idempotent; OAuth bearer stamps app-scoped creator;
  paging emits `Link rel=canonical/type/next/prev` + `ETag`. ✅ (7 in-process
  integration tests). Remaining: full W3C container conformance suite, batch
  partial-failure semantics, `PUT /submit/{jwt}` export ingest.

### M4 — cli-client (native + wasm) ✅ [#component-4]
`read` / `write` / `sync`; distinguish collection points (read aggregates) from
publication points (POST) as distinct types. `Transport` trait abstracts I/O
(native fs + reqwest; wasm Fetch). The native `freedback` CLI signs & posts.
- **Depends on:** M3
- **Acceptance:** same code path reads a file fixture and an endpoint; lib builds
  green for **both** targets in CI (wasm job added). ✅ (3 e2e tests:
  write→read→sync against a live server + file read/append). WASM uses the core
  protocol-lib (no validation chain) per ADR 0004.

### M5 — discovery-server (registry) [#component-2]
`/.well-known/freedback` self-description; `POST /announce` with verifying
fetch; `GET /servers`; `GET /resolve?target=`. Flat list first; NIP-65-style
resolver behind the same interface.
- **Depends on:** M3 (+ `TestCluster` harness comes online here)
- **Acceptance:** announce rejected if well-known missing/invalid; registry
  never trusts the POSTed URL without the verifying fetch.

### M6 — collection-server (aggregation) [#component-7]
Multi-server cache with conditional requests (ETag/If-None-Match) + per-host
rate limiting; per-URI index; URI equivalence table (transitively closed via
SPARQL property paths); `POST /equivalence`.
- **Depends on:** M5
- **Acceptance:** repeated queries hit cache (observable 304s upstream);
  equivalent URIs return a unified set; cross-server dedup by SHA-256 id.

### M7 — advanced-client (local sync copy) [#component-6]
Local redb store keyed by dedup id; resume cursor per (server, target);
dedup-on-merge; optional negentropy reconciliation for backdated items.
- **Depends on:** M4, M6
- **Acceptance:** second sync transfers only `iat > cursor`; no-op when nothing
  new; duplicates from two servers collapse; backdated insert eventually
  reconciled.

### M8 — widgets + Firefox extension + interop demo [#components-3,9,5]
Drop-in Web Components (stars/scalar/thumb/comment/tag) importing the wasm
`protocol-lib`; Firefox extension listing feedback for the current page;
Annotorious/RecogitoJS demo loading a Freedback collection page.
- **Depends on:** M4 (read/write paths), M6 (aggregates)
- **Acceptance:** read-only widget renders aggregates; publish widget round-trips
  a POST; comment/tag bodies render in a pure W3C client without transformation.

### M9 — AI equivalence-detection agent [#component-8]
Scheduled job pulling candidate URI pairs from the index, prompting an LLM
(`agent-prompts/equivalence.md`), writing accepted pairs as `POST /equivalence`
with an auditable `proof`.
- **Depends on:** M6
- **Acceptance:** strict-JSON output; strong-identifier matches accepted;
  title-only never > 0.6 confidence; only above-threshold pairs written.

### M10 — deployment + Pages + release [#cross-cutting]
musl/RocksDB build matrix (cargo-zigbuild); container image; GitHub Pages for
static artifacts (`@context`, ontology, shapes, widget/wasm demos) at stable
URLs; release pipeline (binaries + wasm pkg).
- **Depends on:** M3 (a server to ship)
- **Acceptance:** `docker run … freedback/server` boots; ontology served at
  stable URLs; CI publishes artifacts on tags.

## Cross-cutting issues
- **CI** ✅ — fmt, clippy, native test, wasm32 build, ontology parse checks.
- **Test harness** — `TestCluster` (in-process axum apps on ephemeral ports);
  deterministic fixed keypairs + timestamps. (Lands with M5.)
- **JSON-LD interop** — expand/compact of *external* annotations via the
  `json-ld` crate with a local-context loader. (Supports M8 interop.)
- **Custom rating scales** — a SHACL profile validating `ratingValue` against a
  body's own `worstRating`/`bestRating` (needs SPARQL-based constraints; the
  default profile uses fixed scales). See ADR 0004.
