# ADR 0022 — Data licensing: a `rights` IRI on the annotation, a default license in the well-known

- **Status:** accepted
- **Date:** 2026-07-05

## Context

The requirement is as old as the project. The original 2014 Freedback whitebook
([Toover/freedback, "Data Distribution Policies"](https://github.com/Toover/freedback/blob/master/whitebook.md))
demanded that every piece of feedback carry **explicit licensing**, recommended
Open Data / permissive Creative Commons defaults, and asked that licensing
information be exposed at a designated endpoint so it is "contractually
valuable for both collectors and publishers". The modern protocol had no
licensing story at all: collectors aggregating annotations across servers had
no machine-readable terms to rely on, and publishers had no way to state any.

Two constraints shape the solution:

1. **Zero net-new vocabulary if the standards already provide it** (the spirit
   of INVARIANT 2). The W3C Web Annotation model already defines a **`rights`**
   property on annotations — an IRI identifying the license — mapped to
   `dcterms:rights` in the anno `@context` we pin.
2. **A license is a statement by the author.** Under the content-addressing
   scheme (ADR 0002) and detached signatures (ADR 0003), anything that changes
   the meaning of an annotation must live inside its canonical bytes; anything
   appended by intermediaries must not.

## Decision

1. **Adopt the W3C `rights` property — no new vocabulary.** The model gains an
   optional `rights: Option<String>` (a license IRI, e.g.
   `https://creativecommons.org/licenses/by/4.0/`). The pinned Freedback
   `@context` gains the matching `rights` → `dcterms:rights` (`@type: @id`)
   term — an **additive** context change, allowed by the stable-URL policy.
   Both ingest paths (the alias normalizer, ADR 0007, and full JSON-LD
   compaction, ADR 0011) preserve it.
2. **`rights` is content.** When present it participates in the RFC 8785
   canonical bytes: the same feedback under a different license is a different
   statement (different dedup id), and the author's signature covers the
   license. When absent, the canonical bytes are **byte-identical to the
   pre-licensing form** (`skip_serializing_if`), so every existing fixture,
   signature, and dedup id remains valid — pinned by test.
3. **Validation stays in SHACL** (INVARIANT 3): `dcterms:rights` is optional,
   `sh:maxCount 1`, `sh:nodeKind sh:IRI`. A pure relaxation of the profile —
   no previously-valid data becomes invalid — so the published `shapes.ttl`
   stays stable-URL-compatible.
4. **A server-level default license at the designated endpoint.** The feedback
   server takes `FREEDBACK_DEFAULT_LICENSE` (an IRI) and, when set, surfaces it
   in `/.well-known/freedback` as `"license"`. Semantics: **annotations served
   without an explicit `rights` are distributed under the server's default
   license; an annotation's own `rights` always takes precedence.** The server
   never stamps the default into stored annotations (that would forge content
   into signed bytes) and enforces nothing beyond the SHACL IRI check — the
   protocol carries the terms; contract law does the rest.
5. **Every writer can set it.** `freedback write --license <IRI>` (CLI) and a
   `data-license` attribute on every widget kind (signed and bearer paths — in
   the signed path the license is part of the signed content).

## Why not the alternatives

- **Per-server-only licensing (no per-annotation field).** Simple, but the
  license would change when an annotation federates through a differently-
  configured cache, and authors could not choose terms at all. The author owns
  the annotation (INVARIANT 4); the license is theirs to state.
- **A mandatory `rights` field.** Faithful to the whitebook's letter, but too
  much friction: every existing annotation, fixture, and signing client would
  break (a required field changes every canonical byte stream), and casual
  feedback would need a licensing decision up front. The server default covers
  the common case; the field covers the explicit one.
- **License in the body.** The body is the feedback itself (INVARIANT 2);
  licensing is metadata about the whole annotation. The envelope already has a
  standard slot for it.

## Consequences

- `Annotation` gains `rights` + `with_rights`; canonical-bytes tests pin both
  directions (absence unchanged, presence changes the id). The widget JCS
  cross-language pin gains a licensed variant.
- The RDF mapping emits `dcterms:rights` as an IRI node (non-IRI values become
  literals that fail the SHACL node-kind check → `422`), and the SHACL-subset
  interpreter learns `sh:nodeKind`.
- `/.well-known/freedback` may carry `"license"`; discovery/collection tooling
  can read a server's terms before harvesting. Aggregators that mix servers
  should track the per-annotation `rights` (or the origin server's default) if
  they need to honor per-item terms — future work if it ever matters.
- Deletion is unaffected: erasure (ADR 0021) removes the record regardless of
  its license — the right to be forgotten is not waived by licensing.
