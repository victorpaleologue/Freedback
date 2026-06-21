# ADR 0010 — JWT export profile (`PUT /submit/{jwt}`)

- **Status:** accepted
- **Date:** 2026-06-21
- **Implements:** INVARIANT 1 ("the Mangrove-signed JWT is an export profile
  only, never the native format") and the M3 `PUT /submit/{jwt}` deferral.

## Context

Mangrove submits reviews as a signed JWT (`PUT ${api}/submit/${jwt}`). To
interoperate with that shape — and to offer a single self-contained,
signature-carrying transport for an annotation — Freedback needs a JWT
**export/transport profile**. It must never become the native format: the native
wire format is the W3C Web Annotation (INVARIANT 1).

## Decision

`protocol-lib::export`:
- `to_jwt(ann, identity)` encodes the annotation as a compact ES256 JWS
  (`header.payload.signature`): header `{alg:ES256, typ:JWT, kid:<SPKI PEM>}`,
  payload = the annotation (inner detached `signature`/`id` dropped — the JWT
  signature replaces the former).
- `from_jwt(jwt)` verifies the ES256 signature over `header.payload` against the
  `kid` public key, then **normalizes the payload through `from_jsonld`** (ADR
  0007), so any conformant serialization round-trips. The issuer (`kid`) is
  stamped as `creator` when absent.

The feedback server exposes `PUT /submit/{jwt}`: the JWT signature is the issuer
proof, so this path needs no bearer/self-signature; the decoded annotation goes
through the normal SHACL-validate + store pipeline. The capability is advertised
as `jwt-export` in `/.well-known/freedback`.

## Why reuse our own ES256, not a JWT crate

We already have a pinned, dual-target ES256 (P-256) implementation and a
canonical-bytes signer. A compact-JWS encode/decode is ~40 lines on top of it and
avoids adding a JOSE dependency (the plan flagged `jsonwebtoken` as
WASM-discouraged). The format is standard ES256 JWS, so it interoperates.

## Consequences / limitations

- One signed token transports an annotation end-to-end; servers can accept it
  with no other auth.
- This is a **Freedback** JWT (payload = our annotation). Full **Mangrove review
  schema** mapping (their `sub`/`rating`/`opinion` fields → our model) is a
  further step; this ADR delivers the transport + signature half.
- JWT remains export-only; nothing in the native path produces or requires it.
