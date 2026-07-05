# ADR 0002 — Content-addressed dedup id via RFC 8785 JCS

- **Status:** accepted (amended by ADR 0021: deletion is real)
- **Date:** 2026-06-21

## Context

The same annotation will arrive at the collection server from multiple feedback
servers, be re-POSTed by retrying clients, and be reconciled by the advanced
client's local store. We need a **stable, content-derived identifier** so
duplicates collapse to one row and idempotent writes are free. Nostr's NIP-01
solves the same problem with `sha256` of a serialized event, but its own issue
#354 flags that "JSON itself does not specify a canonical way to serialize
strings and numbers" — the scheme is ambiguous.

## Decision

```
dedup_id = lowercase_hex( SHA-256( JCS( annotation \ {id, signature} ) ) )
```

- **JCS** = RFC 8785 JSON Canonicalization Scheme (via
  `serde_json_canonicalizer`), which removes NIP-01's ambiguity by fixing
  key ordering and number formatting.
- We strip **`id`** (server-assigned, not content) and **`signature`** (the
  proof is computed *over* this canonical form, so it cannot be part of it).
- We deliberately **keep `creator`**: two different issuers asserting the same
  rating are distinct feedback and must not collapse.

The same canonical bytes are the input to both the dedup id **and** the detached
ES256 signature, guaranteeing a verifier hashes exactly what the signer signed.

## Why not alternatives

- *NIP-01 array serialization as-is* — ambiguous (issue #354); two conformant
  encoders could disagree, splitting what should be one id.
- *Hash the raw POST bytes* — whitespace/key-order differences between clients
  would defeat dedup entirely.
- *Random UUIDs* — no idempotency; the collection server could not dedup across
  servers.

## Consequences

- Determinism is testable: `canonical::tests` assert stability, id-independence,
  and content-sensitivity. Fixtures use fixed keypairs + fixed timestamps.
- Edits are modeled as new annotations; the dedup id of an edit differs from
  the original (content differs), which is correct — they are different states.
  Deletion is NOT modeled as content (no "delete annotation") — it is a real,
  author-signed erasure of the record, leaving a content-free tombstone keyed
  by this dedup id (ADR 0021). Because `created` is part of the content
  address, a tombstone retires one record without preventing the author from
  restating the same opinion later (new `created` → new id).
- Backdated items (older than a sync cursor) can be missed by a plain `gt_iat`
  pull; the advanced client reconciles them with negentropy (see roadmap M7).
