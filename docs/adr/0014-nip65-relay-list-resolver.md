# ADR 0014 — NIP-65-style relay list: the outbox discovery model

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** the M5 deferral of a "NIP-65-style resolver" (#6).
- **Builds on:** ADR 0003 (self-signed P-256 identity), ADR 0006 (discovery:
  flat list first, resolver second).

## Context

The discovery registry started with a **flat list** (ADR 0006): servers announce
themselves (verified via their `/.well-known/freedback`), and `GET
/resolve?target=` finds which of them hold feedback for a URI by **polling every
announced server live**. That is correct and simple, but it scales with the
number of servers, not with relevance — every resolution fans out across the
whole registry, and there is no way to ask the more natural federated question:
*where does this particular author publish?*

Nostr solved the analogous problem with **NIP-65** (the "outbox model"): each
key publishes a small, replaceable record listing the relays it **writes** to and
**reads** from. To find a user's events you consult the relays they declared they
write to — discovery follows the author, not a central index.

## Decision

Add a self-signed **relay list** to the registry.

`relays::RelayList` is `{ issuer, read[], write[], updated, signature }`. It
**federates** because it is signed by the issuer's P-256 key (INVARIANT 4a):

- the `signature` is ES256 over the **RFC 8785 canonical bytes** of the record
  minus `signature` — reusing the same JCS the annotation signature uses, via the
  new generic `protocol-lib::canonical_json`; and
- `verify()` additionally requires `issuer == issuer_id_from_pem(kid)`, binding
  the declared issuer to the signing key (a valid signature over key A cannot
  claim to be issuer B's list).

Endpoints:

- `POST /relays` — verify the signature **and** the issuer/key binding, then
  store. **Replaceable**: a newer `updated` supersedes an older one; a stale
  re-publish is ignored (`stored: false`). The registry never takes an issuer's
  claimed servers on faith — exactly as `POST /announce` never trusts a URL
  without the well-known fetch.
- `GET /relays?issuer=` — the stored signed record (and it still verifies after
  the round-trip).
- `GET /resolve?issuer=` — the **outbox** answer: the issuer's `write` servers,
  with no fan-out polling. `GET /resolve?target=` keeps the original flat-list
  behavior.

## Why these specifics

- **Reuse the annotation signature machinery.** One canonicalization (JCS) and
  one signature scheme (ES256 over the canonical bytes) for every signed
  Freedback record keeps a single, well-tested trust path — hence
  `canonical_json` was lifted to a generic helper rather than re-implemented.
- **Issuer/key binding is not optional.** Without it the record's `issuer` field
  would be an unauthenticated claim; checking it against the key turns the list
  into a real statement *by that key about itself*.
- **Replaceable, not append-only.** A relay list is current-state metadata, not
  history; newest-wins by `updated` matches NIP-65's replaceable-event semantics
  and keeps the store O(issuers).
- **Additive, not a replacement.** Target-based flat resolution still works; the
  outbox path is a faster, author-centric overlay. Operators can adopt it
  incrementally.

## Consequences

- Discovery can answer "where does key X publish?" in one lookup, the federated
  question the flat list could not — the groundwork for clients that follow
  authors across servers.
- A new capability `relay-list` is advertised in the registry's `/.well-known`.
- The registry stays "just another conformant server" — the relay list is signed
  data it stores and serves, with the same verify-before-trust discipline as
  announce.
- Limits: the registry does not (yet) verify that an issuer's declared `write`
  servers actually carry its feedback, nor does it expire stale lists; relay
  lists are not themselves propagated between registries. These are natural
  follow-ups, not blockers.
