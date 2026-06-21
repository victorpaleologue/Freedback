# ADR 0013 — WebCrypto P-256 signing in the widgets (browser-native federating identity)

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** the M8 deferral of WebCrypto signing + `<freedback-scalar>` /
  `<freedback-tag>` (#9).
- **Builds on:** ADR 0002 (JCS dedup id), ADR 0003 (self-signed P-256 identity).

## Context

The drop-in widgets could previously only publish via an **OAuth bearer** — the
siloed, non-federating identity. The federating identity (INVARIANT 4a) is a
self-signed P-256 annotation, but producing one in the browser was deferred:
publishing required a server-side token. That left the most interesting property
of the protocol — *anyone can sign feedback that any server can verify with no
shared secret* — unavailable to the zero-build widgets.

The hard part is not the signature; it is that the browser must sign **exactly
the bytes the Rust server reconstructs and verifies**. The server does not verify
the bytes it received — it normalizes the POST through `from_jsonld` into the
canonical model and recomputes `canonical_bytes = JCS(model \ {id, signature})`
(ADR 0002). So a browser signature is only valid if the widget canonicalizes the
*model's* shape with an RFC 8785 implementation byte-identical to Rust's
`serde_json_canonicalizer`.

## Decision

Add WebCrypto signing to `widgets/freedback-widgets.js`, opt-in via a
`data-sign` attribute (it wins over `data-token`).

**Identity.** First use generates an ECDSA P-256 keypair; the private key is
re-imported **non-extractable** and persisted in IndexedDB alongside the public
SPKI DER. The private key never leaves the page — only signatures and the public
key go out. `creator.id` is `urn:freedback:key:<hex(sha256(SPKI DER))>` and the
signature `kid` is the SPKI PEM, matching `identity.rs` exactly.

**Canonicalization.** A ~15-line `jcs()` mirrors RFC 8785: keys sorted by UTF-16
code unit, numbers via the ECMAScript Number→String form the RFC mandates (which
`JSON.stringify` implements), strings via `JSON.stringify`'s
(RFC-8785-compatible) escaping. The widget signs `JCS(content)` where `content`
is the canonical model shape (pinned `@context`, sorted on the wire by JCS) with
bodies already in the Rust `BodyWire` form, so `from_jsonld` reconstructs an
identical model.

**Signing.** `crypto.subtle.sign({name:"ECDSA",hash:"SHA-256"}, priv, bytes)`
returns the raw `R‖S` (64 bytes) that `p256`'s `Signature::from_slice` expects;
base64url-nopad encoded into `signature.sig`. WebCrypto's non-deterministic `k`
is irrelevant — verification accepts any valid `(r,s)`.

**New widgets.** `<freedback-scalar>` (a range input over a configurable
`data-worst`/`data-best`/`data-step` scale → `freedback:ScalarRating` carrying
its scale for SHACL, ADR 0009) and `<freedback-tag>` (`oa:tagging` TextualBody,
rendering distinct tag chips with counts). Both ride the same sign/OAuth submit
path.

## How we keep the two canonicalizers from drifting

This is the failure mode that would silently break browser writes, so it is
pinned two ways:

1. **`widgets/test.cjs`** asserts `jcs(content)` equals the exact string Rust's
   `serde_json_canonicalizer` emits for the equivalent annotation (a committed
   literal), and that the WebCrypto signature verifies over those bytes.
2. **`crates/protocol-lib/tests/widget_interop.rs`** loads a committed
   annotation **signed by the widget** (`tests/fixtures/widget-signed.json`),
   normalizes it with `from_jsonld`, and verifies it with `verify_annotation` —
   a real end-to-end cross-language check that runs in CI's native job (no Node
   needed at test time). It also asserts tamper-rejection and that `creator.id`
   is derived from the signing key.

If either canonicalizer changes, one of these tests fails.

## Consequences

- The widgets can publish **federating, self-signed** feedback with no server
  token and no build step — the protocol's headline property, in a `<script>`
  tag.
- The browser and the Rust core share one canonical form, enforced by tests in
  both languages.
- Scalar and tag feedback now have first-class widgets.
- Limits: `data-sign` needs a secure context (so `crypto.subtle` exists);
  per-device key with no recovery/rotation UI yet; the `jcs()` string escaping is
  validated for the ASCII-dominant data feedback carries (URLs, ISO dates, tag
  text) — exotic-Unicode equivalence with Rust is not separately fuzzed.
