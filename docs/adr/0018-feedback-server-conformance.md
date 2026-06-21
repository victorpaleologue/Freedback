# ADR 0018 — feedback-server conformance hardening

- **Status:** accepted
- **Date:** 2026-06-21
- **Implements:** issue #28 (M3 hardening): W3C WAP / LDP container conformance,
  batch POST partial-failure semantics, and the full Mangrove review-schema
  mapping for the JWT export profile (extends ADR 0010).

## Context

M3 shipped the feedback server with a happy-path container, an all-or-nothing
batch POST, and a Freedback-annotation JWT export profile. Issue #28 asks for
three pieces of hardening. This ADR records the decisions.

## Decision

### 1. Container conformance (W3C WAP / LDP)

`crates/feedback-server/src/collection.rs` now emits the full RFC 8288 /
WAP §3.3.3 navigation set on every collection read:

- `Link` rels `canonical`, `type` (`ldp:Page`), **`first`**, **`last`**, and —
  when an adjacent page exists — `next` / `prev`. A single-page collection has
  `first == last == canonical`; an empty collection is still a valid single
  page 0. The `AnnotationPage` body mirrors this: `partOf` gains
  `type: AnnotationCollection`, `first`, and `last`.
- `Content-Type: application/ld+json; profile="http://www.w3.org/ns/anno.jsonld"`
  so a content-negotiating client can tell the page from plain JSON.
- An `Allow: GET, HEAD, POST, OPTIONS` header on collection responses, and an
  `OPTIONS /annotations/` handler returning `204` with `Allow` + `Accept-Post`.
- **Content negotiation:** a request whose `Accept` cannot be satisfied by our
  JSON-LD media type (e.g. `text/html` only) earns `406 Not Acceptable`;
  `*/*`, `application/*`, `application/json`, `application/ld+json`, and
  `application/activity+json` are accepted. `HEAD` is derived by axum from the
  `GET` handler (headers, no body).

A dedicated suite (`tests/conformance.rs`) exercises all of the above plus the
edge cases (empty / out-of-range page).

### 2. Batch POST partial-failure — **persist-valid-items**

The POST body shape selects the semantics:

- A **single JSON object** keeps the legacy all-or-nothing contract:
  `201 Created` + `Location`, or a flat `4xx`/`422` for the whole request.
- A **JSON array** is a *batch* with partial-failure semantics. Each item is
  authorized, validated, and persisted **independently**; the response is
  **`207 Multi-Status`** whose body lists every item's outcome **in submission
  order**:

  ```json
  {
    "@context": "https://freedback.org/ns/batch/1",
    "type": "BatchResult",
    "total": 3, "succeeded": 2, "failed": 1,
    "results": [
      { "index": 0, "status": 201, "id": ".../annotations/<dedup>" },
      { "index": 1, "status": 422, "error": "SHACL validation failed",
        "report": { "conforms": false, "violations": [ ... ] } },
      { "index": 2, "status": 201, "id": ".../annotations/<dedup>" }
    ]
  }
  ```

  **Policy:** valid items are persisted even when siblings fail; an invalid item
  never blocks a valid one. The batch is `207` whenever it parses (including
  all-success and all-failure), so a client always reads outcomes from the same
  place. Each per-item `status`/body mirrors what that item would have returned
  standalone (`201`, `401`, `422` + SHACL report, …).

  **Authorization** is still evaluated per item: a *valid* OAuth bearer
  authorizes the whole batch (each item is then only content-validated), while a
  self-signed batch is authorized item-by-item so one bad signature fails only
  itself (`401`). The one exception that stays fatal is a **present-but-invalid
  bearer** (`401` for the whole request): with no usable identity we cannot
  attribute any item.

Rationale for persist-valid-items over report-only: the protocol is a bulk,
batch-oriented ingest (INVARIANT 7); making a client re-submit an entire bulk
upload because one row tripped a bound would defeat the point. Idempotency by
dedup id (INVARIANT, unchanged) makes a selective client retry of just the
failed rows safe.

### 3. Full Mangrove review-schema mapping

ADR 0010 delivered a *Freedback* JWT (payload = our annotation). `protocol-lib`
now also maps the **Mangrove review schema** itself, in a pure-Rust,
dual-target module `mangrove.rs` (`to_mangrove_jwt` / `from_mangrove_jwt`,
exposed via the server as `PUT /submit/mangrove/{jwt}`, capability
`mangrove-review`):

| Mangrove claim | Freedback |
|---|---|
| `iss` | JWS `kid` → `creator` (stable `urn:freedback:key:` issuer id) |
| `iat` (unix s) | `created` (RFC 3339 UTC) |
| `sub` | `target` |
| `rating` (int `0..=100`) | `freedback:ScalarRating` on `[0,100]` (ratings rescaled both ways; thumb up/down ⇒ 100/0) |
| `opinion` | `oa:TextualBody` / `oa:commenting` |
| `images` | carried on the body / used for `tag:` markers |
| `metadata.creator_type`, `metadata.conformsTo`, … | preserved under `metadata` |

A Mangrove review must carry at least one of `rating` / `opinion` (mirroring
Mangrove's own check); value bounds remain SHACL's job on the resulting
annotation (INVARIANT 3). The signature path is unchanged ES256 JWS, so a
Mangrove server verifies an emitted token normally.

## Consequences / limitations

- Validation stays entirely in SHACL; the handler only *wires* it (INVARIANT 3).
- `mangrove.rs` adds no native-only deps, so the wasm build is unaffected.
- **Deferred:** Mangrove `metadata` sub-fields beyond creator/profile
  (`nickname`, `client_id`, `is_personal_experience`, `is_affiliated`, `age`,
  `gender`, `openid`, …) are *passed through verbatim* on export but are not yet
  given first-class slots in our `Creator`/annotation model, because doing so
  would touch the stable `@context`/ontology (the stable-URL policy) for fields
  with no Freedback consumer today. Reviewer demographics and the `images`
  `{src,label}` round-trip can be added behind the same functions when a
  consumer needs them.
