# Freedback

> A federated, open protocol for typed feedback on anything with a URI —
> stars, scalar ratings, thumbs, comments and tags — carried as **W3C Web
> Annotations** and signed with portable keys, so feedback is no longer locked
> inside the silo that collected it.

## Why

We can't force every organization to open a feedback channel — so we make our
own. Freedback lets anyone attach feedback to any resource, publish it to a
server they choose, and have it discovered and aggregated across servers,
without a central gatekeeper. The wire format is a standard Web Annotation
(JSON-LD), so existing annotation tooling can already read it, and your
feedback stays yours: sign it with your own key, edit it, or delete it — real
deletion, not a flag — no matter who's hosting it. See it live at
[freedback.net](https://freedback.net/), or read the full vision in
**[the White Book](docs/white-book.md)**.

## Quick start

### Use the widgets (npm)

Drop-in, framework-agnostic Web Components — a side-effect import registers
six custom elements (`<freedback-stars>`, `<freedback-comment>`, …), config is
all `data-*`, so React (and everything else) just renders them:

```sh
npm add @freedback/widgets
```

```html
<script type="module">
  import "@freedback/widgets";
</script>
<freedback-stars
  data-target="https://shop.example/product/42"
  data-read="https://collect.example/index"
  data-publish="https://feedback.example/annotations/"
  data-sign
></freedback-stars>
```

No build step? The same script works from a plain `<script src="…">` tag too.
Full walkthrough (React, outcome events, a reusable wrapper) in
[`docs/widgets-react.md`](docs/widgets-react.md).

### Run your own server

Directly with Cargo (in-memory storage, zero config):

```sh
cargo run -p freedback-feedback-server   # binds 127.0.0.1:8080
```

Or the whole stack (feedback + discovery + collection) with Docker Compose:

```sh
docker compose up --build     # feedback :8080 · discovery :8090 · collection :8100
```

Or a single container:

```sh
docker build -t freedback .
docker run -p 8080:8080 -e FREEDBACK_BASE_URL=https://feedback.example.org \
  freedback freedback-feedback-server
```

There's no prebuilt public image yet — both commands above build locally
(tracked as [#73](https://github.com/victorpaleologue/Freedback/issues/73)).
Environment variables, storage backends, and hosting a public instance are
covered in [`docs/deployment.md`](docs/deployment.md) and
[`docs/hosting.md`](docs/hosting.md).

### Use it as a Rust library

The protocol core (`protocol-lib`) builds and validates annotations, signs
and verifies them, and computes the content-addressed dedup id — usable
directly, no server required:

```rust
use freedback_protocol::{Annotation, Body, Identity, Motivation, Target};
use freedback_protocol::{dedup_id, validate_annotation, verify_annotation};

let mut ann = Annotation::new(
    Motivation::Assessing,
    Target::Iri("https://example.com/item/42".into()),
    vec![Body::star(4.0)],
).with_created("2026-06-21T10:00:00Z");

// Validate against the SHACL profile, sign, verify, and content-address.
assert!(validate_annotation(&ann)?.conforms);
let me = Identity::generate();
me.sign_annotation(&mut ann)?;
verify_annotation(&ann)?;
let id = dedup_id(&ann)?; // stable across servers and re-POSTs
# Ok::<(), freedback_protocol::Error>(())
```

`protocol-lib` targets both native and `wasm32-unknown-unknown` (browser)
builds from the same source — see [`docs/architecture.md`](docs/architecture.md)
for the full crate map.

## Features

- **W3C Web Annotation wire format** — every piece of feedback is a standard
  Web Annotation (JSON-LD): a target, a body, a motivation. Existing
  annotation tooling reads it with zero Freedback-specific code.
  ([ADR 0001](docs/adr/0001-rust-workspace-and-wire-format.md))
- **Typed feedback bodies** — star/scalar/thumb ratings (subclasses of
  `schema:Rating`), comments, tags, and issue/problem reports, all reusing
  existing vocabulary bar one net-new term.
  ([ADR 0009](docs/adr/0009-custom-rating-scales.md),
  [ADR 0023](docs/adr/0023-issue-feedback-type.md))
- **Content-addressed dedup id** — SHA-256 over the RFC 8785 canonical bytes,
  so the same annotation re-posted to any server (or the same server twice)
  collapses to one id. ([ADR 0002](docs/adr/0002-dedup-id-jcs.md))
- **Self-signed portable identity** — an ECDSA P-256 keypair you own; your
  public key is your name, your signature travels with your feedback, any
  server can verify it with no shared secret or account.
  ([ADR 0003](docs/adr/0003-dual-identity-p256.md))
- **Right to erasure** — the signing key is the only key that can edit or
  delete what it signed, and deletion is real: content is erased, only a
  content-free tombstone remains so caches learn to forget too.
  ([ADR 0021](docs/adr/0021-right-to-erasure-deletion.md))
- **Data licensing** — an optional `rights` IRI on each annotation, and a
  server-wide default license advertised in `/.well-known/freedback`.
  ([ADR 0022](docs/adr/0022-data-licensing.md))
- **SHACL-driven validation** — every constraint (datatype, bounds, required
  fields) lives in one SHACL profile, pinned by
  `dcterms:conformsTo`, never in OWL/RDFS.
  ([ADR 0004](docs/adr/0004-validation-in-shacl.md))
- **Federated discovery & aggregation** — servers announce themselves to a
  discovery registry; a collection server finds, dedups, and caches feedback
  across all of them, with URI equivalence and polite rate limiting. No
  central authority.
- **Author identity as a target** — an author's public key is an IRI too, so
  it can itself carry feedback (a discreet, opt-in, text-only "review this
  author" view — deliberately not a public score).
- **Pluggable storage** — a `FeedbackStore` trait behind everything; Oxigraph
  (embedded RDF/SPARQL) in production, SQLite/in-memory for tests.
- **Drop-in surfaces** — vanilla Web Component widgets (npm + `<script src>`),
  a Firefox extension, and an Android-first mobile app (Tauri 2 + Rust),
  sharing the same protocol core.

## Documentation

Full docs live in [`docs/`](docs/README.md), the project wiki — start with
[the White Book](docs/white-book.md) for the vision, or
[the architecture overview](docs/architecture.md) for how the pieces above
fit together.

## License

MIT © Victor Paléologue and the Freedback contributors. Portions of the design
adapt ideas (and, where noted, code) from Mangrove (Apache-2.0), the Hypothesis
client (BSD-2-Clause), and the Nostr NIPs — see [attributions](docs/attributions.md).
