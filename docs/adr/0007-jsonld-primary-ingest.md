# ADR 0007 — JSON-LD is primary on ingest, not interop

- **Status:** accepted
- **Date:** 2026-06-21
- **Supersedes:** the "JSON-LD interop is a later milestone" framing of ADR 0001
  and issue #12.

## Context

INVARIANT 1 says the native wire format **is** a W3C Web Annotation (JSON-LD).
But the first implementation only accepted annotations that matched our exact
serde byte-shape (`body` as an array, compact term names, `@context` as our
two-element array, …). That is not "accepting JSON-LD" — it is accepting *one*
serialization of it. A conformant annotation from Hypothesis, Annotorious, or
RecogitoJS — semantically identical but serialized differently (a single `body`
object, a bare-IRI `target`, prefixed property names) — would be rejected.

There was also a subtler problem. The dedup id and the detached signature are
computed over `JCS(annotation)`, which is **serialization-dependent**: the same
feedback serialized two ways would produce two different content addresses and
two non-comparable signatures. That undermines content-addressing and dedup —
the whole point of the protocol.

## Decision

**Normalize on ingest.** `protocol-lib::jsonld::from_jsonld` parses any
conformant serialization that uses the pinned Freedback/anno vocabulary into the
canonical [`Annotation`] model, tolerating JSON-LD's serialization freedom:

- `@context` string or array (ignored — terms resolved by name);
- `body`/`target` single object or array; `target` bare IRI or `SpecificResource`;
- `type` string or array, prefixed (`freedback:`/`schema:`/`oa:`) or full IRI,
  matched by **local name**;
- properties in compact (`ratingValue`) or prefixed/expanded (`schema:ratingValue`)
  form, via a small per-field alias set.

The feedback server runs this on **every** POST (it is the first step of the
ingest pipeline), before dedup, validation, and storage. Because normalization
happens **before** the dedup id and signature are computed, two different
serializations of the same feedback now collapse to the **same content address**,
and a signature is verified against the normalized form.

`from_jsonld(serialize(ann)) == ann` for our own output (a round-trip test), so
normalizing our own clients' POSTs is a no-op — existing signatures keep
verifying.

## Why not the full `json-ld` crate (yet)

The Haudebourg `json-ld` crate is a complete processor (arbitrary `@context`
expansion). We will need it to ingest annotations that use **third-party
contexts outside our vocabulary**. But it is async, requires a non-trivial
local-context loader to stay offline, and is partial-`wasm32`. The normalizer
above covers the form every real W3C annotation tool actually emits (the compact
anno-context form) plus the prefixed variants, is pure Rust (native + wasm,
deterministic, offline), and ships green today. The full-processor path remains
the documented extension (issue #12, retargeted from "interop" to "arbitrary
external contexts"), behind a `jsonld` feature when added.

## Consequences

- The server is genuinely interoperable: it accepts what the W3C annotation
  ecosystem emits, not only our bytes.
- Dedup id and signatures are **serialization-independent** — a real robustness
  upgrade, tested in `jsonld::tests::accepts_varied_serializations_with_same_dedup_id`.
- One normalization seam owns all serialization tolerance; the rest of the
  pipeline keeps working on the strict typed model.
- Limitation: inputs whose terms come from a non-pinned `@context` are not yet
  expanded — they need the full processor (tracked).
