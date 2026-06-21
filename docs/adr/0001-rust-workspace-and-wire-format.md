# ADR 0001 — Rust workspace & W3C Web Annotation wire format

- **Status:** accepted
- **Date:** 2026-06-21

## Context

Freedback needs one protocol implementation shared by ~9 components that span
servers, native CLIs, browser widgets, and a browser extension. The format must
be interoperable with existing tooling and survive federation across
independently operated servers.

## Decision

1. A single **Cargo workspace** with a shared `protocol-lib` crate compiling to
   **native + `wasm32-unknown-unknown`**. Everything is Rust except the JS
   widgets and the Firefox extension.
2. The **native wire format is a W3C Web Annotation (JSON-LD)** — `target` +
   `body` + `motivation` (+ `Selector`). Typed feedback is carried *in the body*
   as `schema:Rating` subclasses, not as a bespoke envelope.
3. The Mangrove-style signed JWT is an **export profile only**.

## Why

- **Web Annotation is a W3C Recommendation** with an existing ecosystem
  (Annotorious, RecogitoJS, Hypothesis, dokieli). Choosing it means pure W3C
  clients can already render our comments/tags, and only the net-new rating
  bodies degrade gracefully. We get interop for free instead of inventing a
  format nobody reads.
- **One Rust core, two targets** removes the classic drift between a server
  implementation and a JS client implementation: signing, canonicalization, and
  the model are defined once and shipped to the browser as WASM.
- **JSON-LD** lets us layer typed semantics (`schema:Rating`, SHACL) on top of a
  plain-JSON surface, so a non-RDF consumer can still treat an annotation as
  ordinary JSON.

## Consequences / trade-offs

- `protocol-lib` must stay dual-target. Heavy native-only RDF crates (json-ld,
  oxigraph, shacl_validation) are **feature-gated** (`jsonld`, `shacl`); WASM
  consumers use `default-features = false`. CI builds both targets on every
  change so drift is caught immediately.
- JSON-LD's polymorphism (body/target may be object or array) is a determinism
  hazard for content-addressing. We neutralize it by always serializing `body`
  as an array and by hashing the **RFC 8785 canonical** form (see ADR 0002).
- We accept that pure W3C clients show `freedback:*Rating` bodies generically;
  that is the price of not forking the data model.
