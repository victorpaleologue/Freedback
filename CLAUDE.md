# CLAUDE.md — Freedback agent instructions

Freedback is a federated, open feedback protocol whose native wire format is
**W3C Web Annotation JSON-LD**. This file is the contract every contributor
(human or agent) works under. Read it before changing anything.

## The seven fixed invariants — NEVER violate

1. **The annotation is the envelope.** The native wire format is a W3C Web
   Annotation (JSON-LD): `target` + `body` + `motivation` (+ optional
   `Selector`). The Mangrove-signed JWT is an **export profile only**, never the
   native format.
2. **Typed feedback lives in the BODY.** `freedback:StarRating` /
   `freedback:ScalarRating` / `freedback:ThumbRating` are
   `rdfs:subClassOf schema:Rating`; their motivation specializes `oa:assessing`
   via `skos:broader`. Only `freedback:ThumbRating` is net-new vocabulary.
   Comments/tags reuse `oa:TextualBody` + `oa:commenting` / `oa:tagging`.
3. **Validation lives ENTIRELY in SHACL** (datatype, bounds, required, reject) —
   `ontology/shapes.ttl`. NEVER put validation in OWL/RDFS (open-world,
   monotonic, infer-only). Pin the profile via `dcterms:conformsTo`.
4. **Dual identity; authorship = ownership.** (a) Self-signed ECDSA **P-256**
   keypair identity (PEM public key = portable issuer id; signed payloads)
   which **federates at query time**. (b) App-managed OAuth identity keyed by
   composite `(app_id, user_id)` which creates **local-authority silos** and does
   **not** federate. The creating identity OWNS the annotation: signed **edits**
   supersede (`(issuer, target)`, newest wins) and signed **deletes** actually
   erase — right to be forgotten, ADR 0021. The server keeps only a content-free
   tombstone (`dedup_id` + proof) so caches/sync propagate the erasure and the
   id cannot be re-ingested. Readers and caches are never owners.
5. **Everything in Rust except the web widgets and the Firefox extension** (JS).
   Rust targets native + WASM. WASM = **wasm32-unknown-unknown in a browser
   only** (wasm-bindgen / browser fetch). NOT wasm32-wasi, NOT server-side WASM.
6. **Storage is abstracted behind a trait.** Primary impl = Oxigraph (embedded
   RDF/SPARQL). Mock impl = SQLite / in-memory (for tests).
7. **HTTP/1.1 batch, not real-time.** No WebSocket. Optimize for bulk POST and
   paginated bulk reads.

## Dedup id (content address)

`dedup_id = lowercase_hex( SHA-256( JCS( annotation \ {id, signature} ) ) )`,
where JCS is RFC 8785. Inspired by Nostr NIP-01, but JCS removes NIP-01's
JSON-serialization ambiguity. `creator` IS included (different issuers do not
collapse); server-assigned `id` and the `signature` blob are NOT.

## Workspace layout

```
crates/
  protocol-lib/     # native + wasm32: model, JSON-LD, SHACL, signing, JCS
  storage/          # FeedbackStore trait + oxigraph + sqlite/memory impls
  server-lib/       # WAP semantics + Freedback net-new (reusable)
  feedback-server/  # bin: axum server (component 1)
  discovery-server/ # bin: registry (component 2)
  collection-server/# bin: aggregation/cache/index/equivalence (component 7)
  cli-client/       # bin + wasm lib (component 4)
  advanced-client/  # bin+lib: local sync store + resume cursor (component 6)
ontology/           # served as stable URLs: context.jsonld, freedback.ttl, shapes.ttl
widgets/            # JS (component 3)
firefox-extension/  # JS (component 9)
demo-third-party/   # interop demo (component 5)
agent-prompts/      # equivalence-detection prompt (component 8)
```

`protocol-lib` MUST compile to **both** targets. Native-only deps (json-ld,
oxigraph, shacl_validation) are gated behind the `jsonld` / `shacl` features.
WASM consumers depend on it with `default-features = false`.

## Crate verdicts (WASM/native)

| Need | Crate | WASM | Notes |
|---|---|---|---|
| HTTP client | reqwest | ✅ Fetch backend | tls/cookie/timeout opts ignored in browser |
| HTTP server | axum | ❌ native only | |
| ECDSA P-256 | p256 (RustCrypto) | ✅ pure Rust | primary signer; pinned to 0.13 |
| JCS | serde_json_canonicalizer | ✅ | RFC 8785 |
| JSON-LD | json-ld (haudebourg) | ⚠️ partial | server-side framing; pre-compacted on WASM |
| SHACL | shacl_validation (rudof) | ⚠️ | native validation; server validates for WASM clients |
| RDF store | oxigraph | ✅ in-memory (`default-features=false`) | RocksDB native-only |
| Local KV | redb | ✅ | advanced-client store |
| SQLite mock | rusqlite | ❌ C dep | native test mock only |

## Stable-URL policy

`ontology/context.jsonld`, `ontology/freedback.ttl`, `ontology/shapes.ttl` are
published at stable URLs under `https://freedback.net/` and MUST NOT change
incompatibly once released. The `@context` and profile IRIs are pinned in
`protocol-lib::context`.

## Testing rules

- Use the `FeedbackStore` trait's in-memory/SQLite mock for integration tests
  (fast, deterministic, no RocksDB/Clang).
- Use **deterministic fixed keypairs and fixed timestamps** so signatures and
  dedup ids are stable across runs. Fixtures live in `tests/fixtures/`.
- Multi-server tests use the `TestCluster` harness (in-process axum apps on
  ephemeral ports).

## "Verify before quoting" (harvested code)

The Mangrove JSDoc line numbers are **stale**; the Hypothesis anchoring
functions were not confirmed to line level. Open each upstream file in-repo and
confirm its `LICENSE` before porting. Attribution snippets live in
`docs/attributions.md`.

## CI is the gate

Every change must keep CI green: `cargo fmt --check`, `cargo clippy -D warnings`,
native tests, the `wasm32-unknown-unknown` build of `protocol-lib`/`cli-client`,
and ontology validation. See `.github/workflows/ci.yml`. CI only runs the suites
affected by what changed (a `changes` job in `ci.yml` gates the rest).

## Per-package versions & releases

Every releasable unit (the crates, `widgets`, the `mobile` app, the Firefox
extension) is versioned, tagged, and released **independently** —
`.github/packages.json` is the single source of truth. Two rules:

1. **Bump-on-touch.** A PR that changes a package's code MUST bump that
   package's own `version` (docs-only changes are exempt). `versions.yml`
   enforces this and will fail the PR otherwise. Crates carry an explicit
   `version` in their own `Cargo.toml` (NOT `version.workspace = true`).
2. **Release-on-merge.** `tag-and-release.yml` tags `<name>-v<version>` and cuts
   a GitHub Release for each package whose version is new — one tag per package,
   idempotent. See `docs/deployment.md`.
