# ADR 0015 — Discovery hardening: liveness, signed announces, relay-list gossip

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** issue #25 (discovery hardening).
- **Builds on:** ADR 0003 (self-signed P-256 identity), ADR 0006 (discovery:
  flat list first, resolver second), ADR 0014 (NIP-65 relay list / outbox).

## Context

The registry started as a flat list (ADR 0006): a server `POST /announce`s its
URL and the registry verifies it by fetching that server's
`/.well-known/freedback`. ADR 0014 added signed, replaceable relay lists for the
outbox model. Three gaps remained, all called out as follow-ups in ADR 0014:

1. **Stale entries accumulate.** Announce verifies *once*. A server that later
   dies stays in `/servers` and keeps getting polled by `/resolve?target=`.
2. **Announce proves reachability, not key control.** Anyone who can reach a
   conformant well-known URL can announce it; the announcer never proves it
   holds the server's key.
3. **Relay lists do not federate between registries.** A list published to
   registry A is invisible on registry B, defeating the point of a signed,
   relayable record.

## Decision

All three are **additive** and reuse the existing P-256 signing path
(`protocol-lib`), never reinventing signatures.

### 1. Liveness / expiry

Each announced server now carries a `last_verified` timestamp. `AppState::sweep`
re-fetches every server's well-known: success refreshes the stamp; failure
evicts the server **only once it is also past `RegistryConfig::server_ttl_secs`**
(a grace window so a transient blip does not drop a just-verified server).
`/servers` and `/resolve?target=` only ever see live entries.

`sweep` is an **explicit entry point**, and time comes from an injectable
`Clock` (`SystemClock` in production, `TestClock` in tests). Tests advance the
clock and call `sweep` directly — no wall-clock sleeps. The binary runs `sweep`
on a background interval (`FREEDBACK_SWEEP_INTERVAL_SECS`, TTL via
`FREEDBACK_SERVER_TTL_SECS`).

### 2. Signed announces

`POST /announce` accepts an optional detached **ES256** signature over the JCS
canonical bytes of `{ "url": <normalized url> }` — the same scheme as the
annotation and relay-list signatures. When present, the registry requires the
server's well-known to publish its identity key (`"key"`, P-256 SPKI PEM) and
checks that the **signing key equals the published key** (via
`issuer_id_from_pem`), proving the announcer controls that key. The well-known
fetch stays mandatory as corroboration.

**Backward compatible:** an announce with no `signature` keeps the legacy
well-known-only behavior; a server that publishes no `key` simply cannot be
announced with a signature. The response reports `"signed": true|false`. The
feedback server opts in with `AppState::with_server_key_pem`.

### 3. Cross-registry relay-list gossip

`AppState::gossip_relays_to(peer)` pushes every stored relay list to a peer's
existing `POST /relays`. Ingestion (`ingest_relay_list`, shared by the endpoint
and gossip) **verifies the signature and issuer/key binding before storing**, so
a registry can relay lists it received from an untrusted peer without becoming a
trust amplifier — the same verify-before-store discipline announce already uses.
Replaceable semantics carry over: a not-newer list is a no-op, so re-gossip is
idempotent.

## Why these specifics

- **Injected clock, explicit sweep.** Deterministic tests on ephemeral ports
  (the `TestCluster` discipline) cannot wait on wall time; a `Clock` trait plus a
  manual `sweep` make liveness fully testable.
- **Grace window, not immediate eviction.** A single failed fetch is more often a
  blip than a death; tying eviction to the TTL avoids flapping.
- **Announce signs the URL only.** The smallest stable claim ("this key vouches
  for this URL") that still binds key to server, keeping the signed-bytes
  definition (`sign_announce`) a single source of truth.
- **Gossip reuses `POST /relays`.** No new trust path: the receiving registry
  runs exactly the same verification it runs for a first-party publish.

## Consequences

- `/servers` self-heals; dead servers no longer accumulate or get polled.
- A new `signed-announce` and `relay-gossip` capability are advertised in the
  registry's well-known.
- A relay list published to one registry can be made discoverable on another
  while remaining safe to relay untrusted.
- Limits / follow-ups: gossip is push-on-demand (no pull or periodic peer
  schedule yet); the registry still does not verify that an issuer's declared
  `write` servers actually carry its feedback. Neither blocks issue #25.
