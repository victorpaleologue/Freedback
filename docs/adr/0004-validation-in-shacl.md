# ADR 0004 — All validation in SHACL, never OWL/RDFS

- **Status:** accepted
- **Date:** 2026-06-21

## Context

Ratings have hard rules: a star value must be in `[1,5]`, a thumb must be `0` or
`1`, a body is required, a comment must be a non-empty string. These are
**closed-world, rejecting** constraints. RDFS/OWL are the wrong tool: they are
open-world and monotonic — they *infer* rather than *reject*. Asserting
`ratingValue 7` against an OWL bound does not fail; it just adds a triple.

## Decision

- **Structure** (subclass, motivation relations) lives in `ontology/freedback.ttl`
  (RDFS/OWL).
- **All validation** (datatype, bounds, required, reject) lives in
  `ontology/shapes.ttl` as **SHACL Core**. The feedback-server runs it on write,
  before persistence; invalid bodies are rejected `422` with the SHACL report.
- Engine: **rudof `shacl_validation`** (native). Shapes are loaded once at boot.
- The profile is pinned per-annotation via `dcterms:conformsTo`
  (`https://freedback.net/profile/1`).

## SHACL Core, not SHACL-SPARQL

We restrict to **SHACL Core** so any conformant validator agrees with rudof and
the constraints stay declarative. Consequence: bounds are validated against the
**default scales** (stars 1..5, scalar 0..1, thumb {0,1}). Validating a custom
scale (`ratingValue` between the body's own `worstRating`/`bestRating`) needs a
sibling-property comparison that Core cannot express; that is a future profile
(tracked as an issue), not a reason to leak validation into code.

## WASM

The `shacl_validation`/`json-ld`/`oxigraph` stack is native-first. **WASM clients
do not validate locally**; they ship pre-compacted payloads using the pinned
`@context` and rely on the server to validate on write. This matches the plan's
portability fallback and keeps the browser bundle small. `protocol-lib` exposes
SHACL only under the `shacl` feature (off for `default-features = false`).

## Consequences

- There is exactly one place to change a rule: `shapes.ttl`. Code never encodes
  bounds. `Annotation::structural_check` only guards un-processable input (e.g.
  empty body) — it is a convenience, not the authority.
- The validation report returned to clients is the SHACL report, so clients get
  machine-readable, profile-anchored errors.
