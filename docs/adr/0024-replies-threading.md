# ADR 0024 — Replies & threaded discussion via `oa:replying`

- **Status:** accepted
- **Date:** 2026-07-07
- **Builds on:** ADR 0023 (issue type — same "reuse a standard motivation, zero
  new vocabulary" pattern), INVARIANT 2 (typed feedback in the body), INVARIANT
  4 (authorship = ownership; federation at query time), ADR 0021 (erasure).

## Context

Text feedback (comments, and now issues) should be able to start a
**discussion**: a reader replies to a comment, someone replies to that reply,
and a thread forms. The question is how to model a reply without inventing
vocabulary, and how threads relate to the rest of the annotation graph.

Freedback annotations already have identities — a server-assigned `id` and,
more importantly, a content-address `dedup_id`. So a comment is itself a
resource with a URI, and "feedback on a comment" is just an annotation whose
**target is that comment**.

**Does W3C standardize this?** Yes, exactly. The Web Annotation Vocabulary
(W3C Recommendation, 23 Feb 2017) defines the `replying` motivation
(`https://www.w3.org/TR/annotation-vocab/#replying`, verified against
`https://www.w3.org/ns/oa.ttl`, 2026-07-07):

> **`oa:replying`** *(IRI `http://www.w3.org/ns/oa#replying`, an
> `oa:Motivation`)* — "The motivation for when the user intends to reply to a
> previous statement, **either an Annotation or another resource**."

The vocabulary's own Example 58 is precisely this shape — a reply annotation
whose `oa:hasTarget` is another annotation's IRI:

```turtle
<http://example.org/anno57> a oa:Annotation ;
    oa:hasBody [ a oa:TextualBody ; rdf:value "A reply to a question" ] ;
    oa:hasTarget <http://example.com/anno1> ;
    oa:motivatedBy oa:replying .
```

There is **no first-class `oa:Thread` / `oa:Discussion` class** in the
vocabulary (the full motivation set is assessing, bookmarking, classifying,
commenting, describing, editing, highlighting, identifying, linking,
moderating, questioning, replying, tagging). A thread is therefore not a
declared object but an **emergent structure**: the transitive closure of the
`target` links, which a reader reconstructs into a tree.

## Decision

A reply is an ordinary W3C Web Annotation:

- **motivation**: the standard `oa:replying`;
- **body**: `oa:TextualBody` with `rdf:value` = the reply text,
  `format` = `text/plain`, `purpose` = `replying`;
- **target**: the parent annotation, referenced by a **stable content-address
  URN** — `urn:freedback:annotation:<dedup_id>` — not the parent's
  server-assigned `id`;
- **zero new vocabulary**: `oa:replying` and `oa:TextualBody` are both standard
  W3C terms already resolvable in the pinned `@context`, so the ontology and
  the stable URLs are untouched (only a clarifying comment is added to
  `shapes.ttl`, exactly as with the issue type).

In the Rust model this is `Motivation::Replying` plus a distinct
`Body::Reply { value }` variant, so the wire `purpose` mirrors the motivation
just as `commenting`/`tagging`/`editing` do. `Target::annotation(dedup_id)`
builds the `urn:freedback:annotation:<dedup_id>` reference, and a helper reads
the referenced `dedup_id` back out.

**Distinguishing kinds** needs no new type: the *motivation* is the
discriminator. A top-level comment is `oa:commenting` with the subject as
target; a reply is `oa:replying` with an annotation as target. Tags, ratings,
and issues keep `oa:tagging` / `oa:assessing` / `oa:editing`. "Discussion" is
not a kind — it is the shape of the target graph.

**Why the content-address, not the server `id`.** Targeting the `dedup_id`
URN (rather than `https://<server>/annotations/<id>`) makes a thread:

- **federate** (INVARIANT 4): the same parent has the same `dedup_id` on every
  server that holds it, so a reply resolves against any of them — server URLs
  would pin a thread to one host;
- **survive erasure** (ADR 0021): when a parent is erased the server keeps a
  content-free tombstone keyed by `dedup_id`, so a reply targeting that
  `dedup_id` still resolves — to "[deleted]" — and the subtree stays intact
  instead of dangling at an unresolvable URL.

**Opt-in is a widget concern, not vocabulary.** The reply button is rendered
only when a widget sets `data-replies`; the wire format carries no opt-in flag.
(A future *per-author* opt-in — "I allow replies to my comment" — would be a
small net-new boolean gated by SHACL; deferred until wanted.)

## Consequences

- Any conformant Web Annotation consumer degrades gracefully: a reply reads as
  "a textual annotation replying to another annotation" — exactly right.
- Replies are ordinary annotations, so **signing, the `dedup_id` (over
  `creator` + `target` + `body`), erasure, and federation apply unchanged**. A
  reply is independently owned and independently erasable by *its* author;
  erasing a parent does **not** cascade (each author owns their own words).
- The one real cost is on the **read path**: assembling a thread needs a
  bounded second hop — after fetching annotations for the subject, fetch
  annotations whose target is one of those annotations' `dedup_id` URNs, to a
  capped depth (INVARIANT 7: HTTP/1.1 batch, so this is a bounded fan-out, not
  a per-node round trip). The collection server owns this, **opt-in behind
  `GET /index?target=…&replies=1`**. It is opt-in because the walk probes a
  reply-URN for *every* annotation in the aggregate: run unconditionally on a
  large non-threaded target that is a probe-per-annotation fan-out that drains
  the per-host rate budget, so the *next* read serves a stale cached page. The
  flat read stays a single hop; only a thread-rendering client asks for the
  walk. The widget (`data-replies`) does the equivalent hop **client-side** —
  repeated `target=<urn>` base queries — so it reconstructs threads against a
  plain feedback server too, without depending on `replies=1` server support.
- The canonical bytes for replies are pinned cross-language (Rust
  `canonical.rs` ↔ widgets `test.cjs`), like every other body kind.

## Alternatives considered

- **Target the parent's server URL** (`https://<server>/annotations/<id>`) —
  the most literal reading of Example 58, and simpler, but it pins a thread to
  one host and breaks on erasure (the URL stops resolving). Rejected for a
  federated, right-to-be-forgotten protocol.
- **A net-new `freedback:Reply`/`freedback:Thread` class or `freedback:reply`
  motivation** — rejected: violates INVARIANT 2, and unnecessary since
  `oa:replying` fits exactly.
- **`oa:questioning` / `oa:moderating`** — rejected: `questioning` is for
  posing a question about the target, `moderating` is for assessing an
  annotation's trust — neither is "reply to a statement." `replying` is the
  definitional match.
- **Reuse `Body::Comment` under the replying motivation** — rejected: the body
  `purpose` would read `commenting` while the motivation read `replying`,
  breaking the purpose-mirrors-motivation pattern and making replies
  indistinguishable from comments in a multi-body annotation.
