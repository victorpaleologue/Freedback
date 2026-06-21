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

## Resolved-by map (commit ↔ issue)

One self-contained, green commit per milestone. We don't open PRs on this branch,
so this table (and a comment on each GitHub issue) is the traceability record.

| Milestone | Issue | Resolving commit | Status |
|---|---|---|---|
| M1 protocol-lib core | #2 | `ac38bbe` | closed ✅ |
| M2 storage | #3 | `a5f30d9` | closed ✅ (SQLite mock deferred) |
| M3 feedback-server | #4 | `099c978` | closed ✅ |
| M4 cli-client | #5 | `98000ef` | closed ✅ |
| M5 discovery-server | #6 | `347073b` | closed ✅ (NIP-65 resolver deferred) |
| M6 collection-server | #7 | `dd997ad` | closed ✅ (Cache-Control honoring deferred) |
| M7 advanced-client | #8 | `1a70947` | closed ✅ (negentropy deferred) |
| M9 equivalence prompt | #10 | `feeebbd` | closed ✅ (prompt-only by scope) |
| M10 deployment (core) | #11 | `191d210` | open — musl/release/durable-store deferred |

**Convention going forward:** each milestone lands as one commit whose message
names the milestone; on completion, comment the commit SHA on the issue and add
the row here before closing it.

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

### M5 — discovery-server (registry) ✅ [#component-2]
`/.well-known/freedback` self-description; `POST /announce` with verifying
fetch; `GET /servers`; `GET /resolve?target=`. Flat list (NIP-65-style resolver
is a later refinement).
- **Depends on:** M3 (+ `TestCluster` harness now online)
- **Acceptance:** announce rejected if well-known missing/invalid (verified
  against a dead server and a 404 path); resolver returns the holding server for
  a target; registry never trusts the POSTed URL without the verifying fetch. ✅
  (2 cluster tests on real ephemeral ports). Remaining: NIP-65 resolver, server
  liveness/expiry, signed announces.

### M6 — collection-server (aggregation) ✅ [#component-7]
Multi-server cache with conditional requests (ETag/If-None-Match) + per-host
token-bucket rate limiting; per-URI index; URI equivalence (transitive, via a
union-find — see note); `POST /equivalence`.
- **Depends on:** M5
- **Acceptance:** repeated queries revalidate with observable upstream 304s;
  equivalent URIs return a unified set; cross-server dedup by SHA-256 id; per-host
  budget caps upstream bursts. ✅ (6 unit + 4 cluster tests). Conditional GET was
  added to the feedback server. **Note:** equivalence uses a union-find rather
  than Oxigraph SPARQL property paths (same transitive closure, no dependency,
  trivially testable; SPARQL remains an option behind the same API). Remaining:
  `Cache-Control`/`Last-Modified` honoring, persistent index.

### M7 — advanced-client (local sync copy) ✅ [#component-6]
Local redb store keyed by dedup id; resume cursor per (server, target);
dedup-on-merge with edit supersession; `reconcile_full` for backdated items.
- **Depends on:** M4, M6
- **Acceptance:** second sync transfers only `iat > cursor`; no-op when nothing
  new; duplicates from two servers collapse; backdated insert eventually
  reconciled. ✅ (1 unit + 3 integration tests; `freedback-sync` CLI). **Note:**
  backdated reconciliation uses a full pull as a stand-in for negentropy
  (NIP-77) — the efficient range-based protocol remains future work.

### M8 — widgets + Firefox extension + interop demo ✅ (core) [#components-3,9,5]
Drop-in Web Components (`<freedback-stars/thumb/comment>`) — vanilla JS, no build
step; read-only renders aggregates, `data-publish` POSTs a W3C annotation. A
Firefox (MV3) popup lists feedback for the active tab's URL. A dependency-free
interop demo renders a Freedback collection page as plain W3C annotations.
- **Depends on:** M4 (read/write paths), M6 (aggregates)
- **Acceptance:** read-only widget renders aggregates; publish widget builds &
  POSTs a valid annotation; comment/tag render in a pure W3C client without
  transformation, ratings as typed bodies. ✅ Pure helpers unit-tested in CI
  (`widgets/test.cjs`) + JS syntax/manifest checks. **Note:** not browser-E2E'd
  in CI (no headless browser infra); manual via `widgets/demo.html`. WebCrypto
  P-256 signing and `<freedback-scalar>`/`<freedback-tag>` + wasm `protocol-lib`
  glue are deferred; publishing currently uses the OAuth bearer path. Publish
  under **`@freedback/widgets`** (the bare npm name is taken — `docs/naming.md`).

### M9 — equivalence-detection prompt ✅ [#component-8]
**Scope (per maintainer): ship a prompt, not an LLM client.** A self-contained
prompt that any agent can drop in to decide URI equivalence, plus how it feeds
the collection server. The prompt is `agent-prompts/equivalence.md`; the write
path it targets (`POST /equivalence {a,b,proof}`) already exists (M6).
- **Depends on:** M6
- **Acceptance:** the prompt is strict-JSON, rejects title-only matches above 0.6
  confidence, and prefers strong identifiers — ✅ (shipped verbatim). A scheduled
  LLM *client/job* is intentionally out of scope.

### M10 — deployment + Pages ✅ (core) [#cross-cutting]
Multi-stage `Dockerfile` (in-memory Oxigraph → ephemeral demo image, no Clang),
`docker compose up` for the full 3-server stack, a `container.yml` CI job that
builds the image, and a `pages.yml` workflow serving the `@context`/ontology/
shapes at stable `/ns/*` URLs (+ landing page). See `docs/deployment.md`.
- **Depends on:** M3 (a server to ship)
- **Acceptance:** `docker compose up` boots all three servers; ontology served at
  stable URLs by Pages; container build validated in CI. ✅ (core). **Deferred:**
  musl static build via cargo-zigbuild, durable RocksDB backend, tagged release
  pipeline (binaries + wasm pkg), and the `freedback.org` custom domain (pending
  registrar — see `docs/naming.md`).

## Cross-cutting issues
- **CI** ✅ — fmt, clippy, native test, wasm32 build, ontology parse checks.
- **Test harness** — `TestCluster` (in-process axum apps on ephemeral ports);
  deterministic fixed keypairs + timestamps. (Lands with M5.)
- **JSON-LD interop** — expand/compact of *external* annotations via the
  `json-ld` crate with a local-context loader. (Supports M8 interop.)
- **Custom rating scales** — a SHACL profile validating `ratingValue` against a
  body's own `worstRating`/`bestRating` (needs SPARQL-based constraints; the
  default profile uses fixed scales). See ADR 0004.
