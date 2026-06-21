# ADR 0011 — Full JSON-LD via compaction against the pinned context

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** issue #12 (the "arbitrary third-party `@context`" extension that
  ADR 0007 deferred).
- **Builds on:** ADR 0007 (JSON-LD primary on ingest, alias normalizer).

## Context

ADR 0007 made ingest normalize any serialization expressed over the **pinned**
Freedback/anno vocabulary: compact terms, prefixed IRIs, single-or-array
shapes. That covers everything the W3C annotation ecosystem actually emits over
the standard anno context. It does **not** cover a document that names the same
concepts with a third party's *own* terms bound to the canonical IRIs by an
inline `@context`:

```json
{
  "@context": { "about": { "@id": "http://www.w3.org/ns/oa#hasTarget", "@type": "@id" }, … },
  "@type": "Rating",
  "about": "https://example.com/item/1",
  "scores": { "@type": "Stars", "stars": 4 }
}
```

The alias normalizer keys off term *names* (`target`, `body`, `oa:hasTarget`),
so `about`/`scores` are invisible to it even though they expand to the exact
IRIs we use. Resolving them needs a real JSON-LD processor that interprets the
declared `@context`.

## Decision

Add `protocol-lib::jsonld_full::normalize_full` (native, behind the `jsonld`
feature) that **compacts the incoming document against our pinned `@context`**
using the Haudebourg `json-ld` crate, then feeds the predictable result to the
existing `from_jsonld` normalizer.

Compaction is the key trick. The processor first **expands** the input with
whatever `@context` it declares — turning `about`/`scores`/`stars` into the full
IRIs `oa:hasTarget`/`oa:hasBody`/`schema:ratingValue` — and then **re-serializes
using our term definitions**, producing exactly the compact shape (`target`,
`body`, `motivation`, `ratingValue`, …) that `from_jsonld` already parses. So we
reuse the entire downstream pipeline (model → dedup id → signature → SHACL)
unchanged, and a foreign vocabulary content-addresses **identically** to the
canonical form (tested in
`jsonld_full::tests::normalizes_a_foreign_vocabulary_to_the_same_dedup_id`).

The pinned context is embedded at build time via `include_str!` of
`ontology/context.jsonld` — the same document served at the stable URL — so the
compaction target can never drift from what clients pin.

### Server wiring

`feedback-server` enables the `jsonld` feature and tries the **fast path first**:
`from_jsonld`; on failure it falls back to `normalize_full`. The fast path keeps
the common case allocation-light and offline; full compaction only runs for
genuinely foreign documents. If both fail, the fast-path error is surfaced
(with the compaction error appended) and the POST is a `400`.

## Why compaction, not expansion + a custom walker

Expanding and walking the expanded form ourselves would mean re-implementing
value-object/`@list`/`@id` handling that compaction already does correctly, and
the expanded shape (full IRIs, value-object wrappers) is *further* from what
`from_jsonld` wants, not closer. Compacting to our own context lands precisely on
the shape we already parse. The `ExpandedDocument → JSON` conversion also has no
ergonomic path without standing up vocabulary machinery, whereas `compact`
returns a `json_syntax::Value` directly.

## Scope and limits

- **Native only.** The `json-ld` stack is large and async; it is not part of the
  wasm core (which relies on server-side normalization, per ADR 0004). Gated
  behind the `jsonld` feature so non-server consumers and the wasm build never
  pull it.
- **Inline contexts and well-known remote contexts resolve offline.** The
  processor is driven by a **preloaded allowlist loader** (`preloaded_loader`)
  seeded with the well-known remote `@context` documents bundled at compile time
  (`ontology/vendor/`): the canonical W3C Web Annotation context
  (`http(s)://www.w3.org/ns/anno.jsonld`, verbatim) and a **curated schema.org
  rating subset** (`http(s)://schema.org/…`, the rating terms Freedback's typed
  bodies use). So a document that references one of those URLs — e.g. a bare
  `"http://www.w3.org/ns/anno.jsonld"` alongside otherwise non-pinned terms —
  normalizes to the same dedup id without any network call (issue #24). The
  loader is a fixed `HashMap`; **every other URL is refused** (`EntryNotFound`),
  so there is still **no network I/O on the validation path** — no arbitrary
  fetch, no SSRF, no latency/availability coupling. The full schema.org context
  (~205 KB, with a global `@vocab` catch-all that would mis-expand foreign
  terms) is intentionally not bundled; see `ontology/vendor/README.md` for
  provenance and the exact loader keys.
- Documents already expressible over the pinned vocabulary never reach this path
  — the fast normalizer handles them, so there is no performance regression for
  the common case.

## Consequences

- Freedback now ingests genuinely arbitrary conformant Web Annotation JSON-LD,
  not only the pinned-vocabulary serializations — the strongest reading of
  INVARIANT 1.
- Dedup id and signatures remain serialization- **and vocabulary**-independent.
- One new dependency surface (`json-ld`/`json-syntax`/`static-iref`/`iref`/
  `futures`), confined to native builds behind a feature.
