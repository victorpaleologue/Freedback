# Freedback

> A federated, open protocol for typed feedback on anything with a URI —
> stars, scalar ratings, thumbs, comments and tags — carried as **W3C Web
> Annotations** and signed with portable keys, so feedback is no longer locked
> inside the silo that collected it.

Freedback lets anyone attach feedback to any resource, publish it to a server
they choose, and have it discovered and aggregated across servers — without a
central gatekeeper. The wire format is a standard Web Annotation (JSON-LD), so
existing annotation tooling can already read it.

## Status

Early implementation. The protocol core (`protocol-lib`) is functional and
green on **native + wasm32**:

- ✅ Web Annotation model with typed rating/comment/tag bodies
- ✅ Content-addressed dedup id (RFC 8785 JCS + SHA-256)
- ✅ Self-signed ECDSA P-256 identity (detached ES256 signatures)
- ✅ Shapes-driven SHACL-Core-subset validation (native; browsers validate via
  the server, per ADR 0004)
- ✅ Storage trait + in-memory and Oxigraph backends (shared conformance suite)
- ✅ Feedback server (axum): WAP container, paging, `/sync`, dual auth, well-known
- ✅ Basic client (native + wasm32): read / write / sync over endpoints & files
- ✅ Discovery server (registry): announce-with-verify + resolve (federation)
- ✅ Collection server: index, URI equivalence, polite caching, rate limiting
- ✅ Advanced client: local redb sync copy with resume cursor + dedup-on-merge
- ✅ Equivalence-detection prompt (`agent-prompts/equivalence.md`)
- ✅ Deployment: `docker compose up` for the full stack; Pages serves the ontology
- ✅ Web widgets (vanilla Web Components), Firefox MV3 popup, W3C interop demo

All 10 milestones have a working deliverable; the Rust backbone is done and
tested (43 Rust tests + JS helper tests).
**Naming note:** the canonical IRIs and the Pages site use the owned
**`freedback.net`** domain; an unrelated dormant npm `freedback` holds
`freedback.dev` — see [`docs/naming.md`](docs/naming.md).

## Quick start

```bash
# Whole stack in one command (ephemeral in-memory storage):
docker compose up --build      # feedback :8080 · discovery :8090 · collection :8100

# Native: build, lint, test the whole workspace
cargo test --workspace

# Browser target: the dual-target core must build for wasm32
rustup target add wasm32-unknown-unknown
cargo build -p freedback-protocol --no-default-features --features wasm \
  --target wasm32-unknown-unknown
```

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

## Documentation

- [Architecture overview](docs/architecture.md)
- [Design decisions (ADRs)](docs/adr/)
- [Roadmap & issue map](docs/roadmap.md)
- [Agent invariants](CLAUDE.md) — the rules every contributor works under
- [Attributions](docs/attributions.md)

## License

MIT © Victor Paléologue and the Freedback contributors. Portions of the design
adapt ideas (and, where noted, code) from Mangrove (Apache-2.0), the Hypothesis
client (BSD-2-Clause), and the Nostr NIPs — see [attributions](docs/attributions.md).
