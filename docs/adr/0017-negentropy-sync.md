# ADR 0017 — Negentropy (NIP-77) range-based sync for backdated reconciliation

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** the M7 deferral of negentropy (#26); `docs/roadmap.md` M7 note.
- **Builds on:** ADR 0002 (content-addressed dedup id over RFC 8785 JCS),
  the M7 advanced-client (#8: local redb copy + per-`(server, target)` resume
  cursor).

## Context

The advanced-client keeps a local copy of a target's feedback and pulls
incrementally with a resume cursor: `/sync?gt_iat=<cursor>` returns only items
strictly newer than the cursor. This is O(new) for **forward** arrivals, but it
is structurally blind to **backdated** items — an annotation whose `iat` is below
the cursor (a late federation hop, a clock-skewed peer, an import of old data) is
never re-seen. M7 patched the hole with `reconcile_full`: a from-scratch
`gt_iat = 0` pull that re-fetches the **entire** target set. Correct, but O(all)
on every reconcile — untenable as a set grows.

Nostr's [NIP-77 "negentropy"] solves exactly this: efficient range-based set
reconciliation that transfers data proportional to the size of the **difference**
between two sets, not their total size.

## Decision

Implement negentropy over the per-`(server, target)` set of content-addressed
**dedup ids**, and make it the reconcile path; keep `reconcile_full` as a
labeled fallback.

### The algorithm (`protocol-lib::negentropy`, pure Rust, native + wasm)

Both peers hold a set of `Item { timestamp, id }` (the annotation's `iat` and its
dedup id) and sort it by `(timestamp, id)` — the NIP-77 sort key — so both derive
the same canonical order and address the same ranges. Then:

1. The initiator sends a covering set of **ranges**. For each range it sends
   either a **fingerprint** (a cheap digest of the ids in that range) or, when
   the range already holds few ids, the **explicit id list** (NIP-77's IdList
   mode).
2. The responder compares each range against its own set. A **matching
   fingerprint settles** the range — nothing transfers. A **mismatch is split**
   into up to `BUCKETS` sub-ranges (at real item boundaries) that recurse; once a
   range is small (`≤ ID_LIST_THRESHOLD`) it answers with explicit ids.
3. The initiator diffs each settled id list into `have` (only it holds) and
   `need` (only the peer holds), and re-poses still-mismatching fingerprint
   ranges for the next round. Recursion depth is `log_BUCKETS(N)`.
4. The initiator fetches **only** the `need` ids in bulk.

### Framing — our choice (we do **not** match NIP-77's wire bytes)

NIP-77 is a binary, varint-packed, **stateful streaming** protocol built for a
relay's persistent connection. Freedback is **HTTP/1.1 batch, not real-time**
(INVARIANT 7), so we keep the negentropy *algorithm* but reframe each round as a
**stateless JSON request/response**:

- **Fingerprint** = lowercase-hex `SHA-256( count_le ‖ id_bytes… )` over the ids
  in the range. The issue allowed "XOR or secure hash of ids in a range"; we use
  SHA-256 because Freedback already depends on `sha2` everywhere (the dedup id
  itself is SHA-256), keeping the wasm bundle lean. The count prefix means a
  range and a strict superset never collide and the empty range has a fixed
  value. We deliberately forgo NIP-77's addition-mod-`2^256` (incrementally
  updatable) fingerprint — collision resistance, not incremental update, is what
  a per-round stateless exchange needs.
- A `Message` is a list of per-range claims (`Fingerprint{range, fp}` or
  `IdList{range, ids}`); a `Bound` is a half-open `(timestamp, id)` interval
  (inclusive lower, exclusive upper; `None` = unbounded). The server answers each
  round at `POST /negentropy {target, message}` with another `Message`, reading
  only its set — so each round is an independent, idempotent HTTP call. The
  client drives rounds to a fixpoint, then bulk-fetches the `need` ids at
  `POST /annotations/by-id {ids}`.

### Where each side lives

- **Core** (`protocol-lib::negentropy`): `sorted`, `initiate`, `respond`,
  `reconcile`, `fingerprint` — pure, dependency-light, compiles to
  `wasm32-unknown-unknown`. The same `respond` serves the server handler and the
  in-process tests.
- **Server** (`feedback-server`): `POST /negentropy` (one reconcile round over
  the **full** id set for a target — not collapsed to latest edits, since
  reconciliation diffs ids one-for-one) and `POST /annotations/by-id` (bulk
  fetch). Advertises a `negentropy` capability in `/.well-known/freedback`.
- **Client** (`cli-client::Client`): `negentropy_round` + `fetch_by_id`.
  `advanced-client::AdvancedClient::reconcile` drives the loop, fetches only the
  `need` ids, and falls back to `reconcile_full` (labeled `ReconcileVia::FullPull`)
  if the peer has no `/negentropy` endpoint.

## Why these specifics

- **Reconcile over the full id set, not latest-edits.** Edit-supersession is a
  *view* the local store computes after merging; reconciliation must compare
  every stored id so a backdated *edit* is not silently dropped as "already have
  the latest".
- **`have` is ignored by the advanced-client.** It is a read-only local copy and
  never pushes; the protocol still computes `have` (so a future bidirectional
  sync gets it for free), but the client only acts on `need`.
- **Additive + graceful degradation.** The cursor `/sync` path is untouched; a
  server that never deploys `/negentropy` still reconciles via the full-pull
  fallback. No invariant moves, no wire format changes.

## Consequences

- A second reconcile after a handful of backdated inserts transfers **O(diff)**:
  the acceptance test seeds 500 items, syncs them, inserts 5 backdated items, and
  asserts the second reconcile transfers exactly 5 (not 500) in `< 10` rounds,
  with a third reconcile transferring 0. The protocol core is unit-tested for
  identical sets (zero transfer), one-sided differences, both-directional
  differences, and logarithmic convergence on a 4096-item set.
- Limits / follow-ups: the `/negentropy` round currently rebuilds and re-sorts
  the server's id set per request (fine for the in-memory store; a production
  Oxigraph backend would want a cached/indexed ordering); reconciliation is
  one-directional (pull-only) by the advanced-client's design; and the bound
  encoding sends full `(timestamp, id)` keys rather than NIP-77's
  prefix-compressed bounds — a wire-size optimization, not a correctness gap.

[NIP-77 "negentropy"]: https://github.com/nostr-protocol/nips/blob/master/77.md
