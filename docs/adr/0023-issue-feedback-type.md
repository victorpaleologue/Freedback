# ADR 0023 — The issue / problem-report feedback type

- **Status:** accepted
- **Date:** 2026-07-05

## Context

The original 2014 Freedback proto defined exactly three feedback kinds:
`Comment`, `Rating`, and
[`Issue`](https://github.com/Toover/freedback/blob/master/freedback_grpc_python/freedback.proto)
(`message Issue { string text = 1; }`) — a free-text problem report about the
subject. Comments and ratings have long been ported to the annotation model
(INVARIANT 2); the issue type was still missing.

INVARIANT 2 allows only `freedback:ThumbRating` as net-new vocabulary, so an
issue must be expressed entirely with standard terms: a W3C `oa:TextualBody`
under a standard motivation.

**Which standard motivation?** The task that introduced this type assumed
`oa:flagging`. Verification against the authoritative sources
(`https://www.w3.org/ns/anno.jsonld` and `https://www.w3.org/ns/oa.ttl`,
checked 2026-07-05) shows **`oa:flagging` does not exist** — it appears in
neither the Web Annotation JSON-LD context nor the vocabulary's `oa:Motivation`
instances. Emitting it would mint an undefined IRI inside W3C's namespace:
worse than net-new vocabulary in our own namespace, and a violation of
INVARIANT 2's spirit (reuse *real* standard terms).

The standard motivation whose definition actually matches a problem report is
**`oa:editing`**: *"The motivation for when the user intends to request a
change or edit to the Target resource."* Reporting an issue ("the checkout
button does nothing") is precisely a request that the target be fixed.

## Decision

An issue is an ordinary W3C Web Annotation:

- **body**: `oa:TextualBody` with `rdf:value` = the issue text,
  `format` = `text/plain`, and `purpose` = `oa:editing`;
- **motivation**: the standard `oa:editing`;
- **zero new vocabulary** — no ontology change at all. `editing` is already a
  term of the pinned W3C anno context, so the JSON-LD `@context` is untouched
  and the stable ontology URLs are byte-identical in meaning (only a comment
  was added to `shapes.ttl`).

In the Rust model this is `Motivation::Editing` plus a distinct
`Body::Issue { value }` variant (rather than reusing `Body::Comment` under a
different motivation), so the wire `purpose` mirrors the motivation exactly as
comments (`commenting`) and tags (`tagging`) do. The serialization remains an
ordinary `TextualBody`, byte-identical in shape to a comment except for the
`purpose` string. Validation reuses the existing `TextualBodyShape` (non-empty
`rdf:value`); motivations are not enumerated anywhere in the SHACL profile, so
admitting issues changes no constraint — stable-URL-compatible.

User-facing surfaces keep the domain name "issue": `freedback write --issue`,
`<freedback-issue>` (textarea + "Report" + a ⚠-marked list), and the Mangrove
export maps the text onto the review `opinion` (lossy, like tags).

## Alternatives considered

- **`oa:flagging`** — rejected: not defined by the W3C Web Annotation
  vocabulary (verified against the published context and ontology); using it
  would squat an IRI in the `http://www.w3.org/ns/oa#` namespace.
- **A net-new `freedback:Issue` class or `freedback:reporting` motivation** —
  rejected: violates INVARIANT 2 ("only `freedback:ThumbRating` is net-new"),
  and unnecessary since a standard motivation fits.
- **`oa:moderating`** — rejected: it is about moderating *annotations* (e.g.
  voting an annotation up/down in a trust network), not reporting problems
  with the target resource.
- **Reusing `Body::Comment` under the issue motivation** — rejected: the body
  `purpose` would read `commenting` while the motivation read `editing`,
  breaking the purpose-mirrors-motivation pattern comments and tags establish,
  and making issues indistinguishable from comments in multi-body annotations.

## Consequences

- Any conformant Web Annotation consumer degrades gracefully: an issue reads
  as "a textual annotation requesting an edit" — exactly right.
- `editing` joins the motivations the ingest normalizer and the model accept;
  the dedup id, signing, erasure, and federation machinery apply unchanged.
- The canonical bytes for issues are pinned cross-language (Rust
  `canonical.rs` ↔ widgets `test.cjs`), like stars and licensed content.
