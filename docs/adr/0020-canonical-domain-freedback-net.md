# ADR 0020 — Canonical domain: `freedback.net`

- **Status:** accepted
- **Date:** 2026-06-21
- **Supersedes:** the placeholder `freedback.org` base used through M1–M10.
- **Relates to:** the stable-URL policy (CLAUDE.md), ADR 0011 (pinned `@context`),
  `docs/naming.md`.

## Context

The protocol's IRIs — the `freedback:` namespace, the `@context` URL, and the
`dcterms:conformsTo` profile — were pinned under a `freedback.org` base that the
project did not own (and `freedback.org` was never verified at a registrar).
`freedback.dev` is taken by an unrelated dormant npm homonym (`docs/naming.md`).
The owner has now registered **`freedback.net`**.

Because only `id` and `signature` are stripped before canonicalization
(`canonical.rs`), the `@context` URL and `conformsTo` value **are** part of the
RFC 8785 bytes that are hashed (dedup id) and signed. The base domain is
therefore content-affecting: it is cheap to change *now* (nothing is released)
and expensive later (stable-URL policy). This is the moment to set it.

## Decision

Adopt **`freedback.net`** as the single canonical base everywhere:

- `protocol-lib::context`: `FREEDBACK = https://freedback.net/ns#`,
  `CONTEXT_URL = https://freedback.net/ns/context.jsonld`,
  `PROFILE_URL  = https://freedback.net/profile/1`.
- The served artifacts (`ontology/context.jsonld`, `freedback.ttl`, `shapes.ttl`),
  the SHACL shape targets, the RDF mapping, the widget JS (`ANNO_CTX`/`PROFILE`),
  the Firefox extension, and all docs use `freedback.net`.
- GitHub Pages (`pages.yml`) serves them at the matching URLs and writes a
  `CNAME` of `freedback.net`; `docs/deployment.md` documents the DNS + repo
  settings the owner completes to attach the domain.

The committed browser-signed interop fixture (`widget-signed.json`) was
**regenerated** with the widget's own ES256 signer over the new bytes (a fresh
test keypair), since the old signature was over `freedback.org` bytes.

## Consequences

- All content addresses (dedup ids) and self-signatures are now computed over
  `freedback.net` IRIs. As nothing was released, there is no migration burden;
  going forward the stable-URL policy applies to `freedback.net` and these URLs
  MUST NOT change incompatibly.
- The protocol artifacts resolve at their pinned URLs once DNS is attached; until
  then they are reachable at the `*.github.io` Pages URL.
- `freedback.org` no longer appears anywhere in the tree.
