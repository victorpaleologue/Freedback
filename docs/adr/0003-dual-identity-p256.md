# ADR 0003 — Self-signed P-256 identity (federating) + OAuth silos

- **Status:** accepted (extended by ADR 0021: the key also authorizes erasure)
- **Date:** 2026-06-21

## Context

Federation requires that any server can verify *who said what* without sharing a
secret with the issuer. At the same time, apps that already own their users want
to attribute feedback without minting keypairs for everyone.

## Decision

Two identities (INVARIANT 4):

1. **Self-signed ECDSA P-256** (the federating identity). Mirrors Mangrove. The
   public key (SPKI PEM) is the portable issuer id and the JWS `kid`. Each
   annotation carries a detached **ES256** signature over its RFC 8785 canonical
   bytes (ADR 0002). Implemented with the pure-Rust **`p256`** crate (RustCrypto,
   pinned to the stable 0.13 line), so it compiles to native *and* `wasm32`.
   A compact `urn:freedback:key:<sha256-of-SPKI>` is offered as `creator.id`.
2. **App-managed OAuth**, keyed by composite `(app_id, user_id)`. Creates a
   **local-authority silo**: valid within the app's domain, does **not**
   federate. The feedback-server's auth middleware accepts either.

## Why P-256 (not Ed25519)

- **Mangrove parity** and the **browser**: WebCrypto (`crypto.subtle`) implements
  ECDSA P-256 natively, so widgets can sign without shipping private keys through
  WASM linear memory — both paths produce the same ES256 signature over the same
  canonical bytes. Ed25519 in `crypto.subtle` is newer and less universally
  available. We pay ~245 µs/sign in WASM vs Ed25519; irrelevant for batch HTTP.

## Consequences

- The signing input is the *single* canonical form from ADR 0002 — there is no
  second serialization path to drift.
- Private keys are PKCS#8 PEM; public keys SPKI PEM. `Identity` round-trips both.
- WASM widgets MAY delegate to WebCrypto; the Rust path remains the reference and
  the test oracle (`identity::tests`).
- OAuth identity is intentionally non-federating; the collection server must
  treat siloed feedback as lower-trust and never merge it across apps.
- The creating identity is the OWNER of the annotation: the same key (or the
  same OAuth `(app_id, user_id)`) that created it authorizes its erasure —
  right to be forgotten, ADR 0021.
