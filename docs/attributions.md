# Attributions

Freedback adapts ideas and, where noted, code from prior art. **Before porting
any upstream code, open the file in-repo and confirm its `LICENSE`** — the
Mangrove JSDoc line numbers are stale and the Hypothesis anchoring functions
were not confirmed to line level during research.

## Design & code provenance

### Mangrove (Open Reviews Association) — Apache-2.0
- https://gitlab.com/open-reviews/mangrove
- Model for the self-signed P-256 keypair identity, the `signReview` /
  `getReviews` flow, the `gt_iat` + `latest_edits_only` sync cursor, and
  `claimEquivalence`.
- Attribution: *"Portions adapted from Mangrove (Open Reviews Association),
  https://gitlab.com/open-reviews/mangrove, licensed under the Apache License,
  Version 2.0."*

### Hypothesis client — BSD-2-Clause
- https://github.com/hypothesis/client (`src/annotator/anchoring/`)
- Model for `TextQuoteSelector` / `TextPositionSelector` anchoring (used when we
  build the widgets / extension).
- Attribution: *"Selector anchoring adapted from the Hypothesis client, ©
  Hypothesis contributors, BSD-2-Clause."*

### dom-anchor-text-quote / dom-anchor-text-position — MIT
- https://github.com/tilgovi/dom-anchor-text-quote ·
  https://github.com/tilgovi/dom-anchor-text-position
- Cleaner-licensed standalone alternative to porting the Hypothesis client.
- Attribution: *"from dom-anchor-text-quote / dom-anchor-text-position by Randall
  Leeds, MIT License."*

### Nostr NIP-01 — public domain (nostr-protocol)
- https://github.com/nostr-protocol/nips/blob/master/01.md
- Model for the deterministic content-addressed id. Freedback resolves NIP-01's
  JSON-serialization ambiguity (issue #354) by hashing the **RFC 8785 JCS**
  canonical form. See [ADR 0002](adr/0002-dedup-id-jcs.md).
- Attribution: *"Deterministic event-id scheme inspired by Nostr NIP-01."*

### Nostr NIP-65 / NIP-77 — public domain; negentropy — MIT
- NIP-65 (kind 10002 "Relay List Metadata") is the model for the discovery
  resolver (URI → preferred server set, the outbox model).
- NIP-77 negentropy is the model for backdated-item reconciliation; Rust crate
  `negentropy` (rust-nostr, MIT) for the advanced client.
- Attribution: *"Set reconciliation via the negentropy protocol (© Doug Hoyte,
  MIT); Rust crate `negentropy` (rust-nostr, MIT)."*

## Standards implemented (freely implementable)
- W3C Web Annotation Data Model / Vocabulary / Protocol
- W3C SHACL · W3C Activity Streams 2.0
- schema.org Rating / AggregateRating / Review
- RFC 8615 (Well-Known URIs) · RFC 8785 (JCS)

## Key Rust dependencies (licenses)
`p256` (Apache-2.0/MIT), `serde`/`serde_json` (MIT/Apache), `oxrdf`/`oxttl`
(MIT/Apache), `serde_json_canonicalizer` (MIT), `sha2` (MIT/Apache),
`axum`/`tokio` (MIT). Full list in each crate's `Cargo.toml`.
