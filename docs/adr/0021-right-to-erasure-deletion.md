# ADR 0021 — Right to erasure: author-signed deletion replaces "append-only"

- **Status:** accepted (amends ADR 0002 and ADR 0003; rewrites invariant 4's
  edit/delete clause)
- **Date:** 2026-07-05

## Context

The original design carried an implicit Nostr-style assumption: the annotation
log is append-only, "deletes" are just newer annotations that supersede older
ones (collapsed per `(issuer, target)` by `/sync?latest_edits_only=true`), and
nothing is ever removed — the plain `GET /annotations/?target=` read returns
every annotation ever posted, forever.

That model is wrong for Freedback, for three reasons:

1. **Freedback is not a replication network.** Federation happens at **query
   time**: a collection server polls feedback servers and keeps a *cache*.
   Readers and caches are never owners of the data. There is no swarm of peer
   relays whose copies we must treat as canonical — the feedback server the
   author published to *is* the authoritative store.
2. **The author owns their feedback.** The self-signed P-256 key (ADR 0003) is
   not just a provenance mechanism — it is the **ownership credential**. The
   holder of the key that signed an annotation has the right to erase it
   (GDPR-style *right to be forgotten*), directly, without operator
   intervention — not merely the ability to publish a newer statement on top.
3. **Deployments are not necessarily public.** A Freedback server may be
   internal to a company. "Once published, always retrievable" is not an
   acceptable contract for personal data held in an identified storage the
   author can reach.

What deletion can and cannot promise: erasure is a **guarantee at the server
that executes it** and a **propagated instruction** to protocol-level caches.
Copies exported beyond protocol reach (screenshots, third-party scrapes) are
out of scope — as with any published data, takedown there is best-effort.

## Decision

1. **Real deletion, authorized by authorship.** The feedback server exposes
   `DELETE /annotations/{dedup_id}` (the WAP-conformant verb on the annotation
   resource). Authorization matches the identity that created the annotation:
   - **Self-signed annotations:** the request carries a detached ES256
     signature over the RFC 8785 (JCS) canonical bytes of a delete document
     `{"type": "Delete", "annotation": "<dedup_id>", "created": "<RFC3339>"}`,
     verified against the **same public key** (`kid`) that signed the
     annotation. Same canonicalization, same signature scheme, same key — no
     new cryptography (ADR 0002/0003 machinery reused).
   - **OAuth annotations:** a valid bearer resolving to the same composite
     `(app_id, user_id)` creator.
2. **Erase content, keep a content-free tombstone.** On deletion the server
   removes the annotation (body, target, timestamps — everything), retaining
   only `{dedup_id, deleted_at, proof}`. The tombstone contains no feedback
   content and no personal data beyond the issuer's already-public key. It
   exists so that the *erasure itself* can propagate:
   - tombstones are exposed to sync consumers (with `deleted_at` as their
     cursor position) so caches drop their copies on the next pull;
   - re-`POST` of a tombstoned `dedup_id` is rejected (`410 Gone`), so deleted
     content cannot resurrect through gossip, retries, or negentropy
     reconciliation.
3. **Re-statement is always possible.** Because `created` is part of the
   content address (ADR 0002), an author who genuinely wants to say the same
   thing again produces a new `created` → a new `dedup_id`, unaffected by the
   old tombstone. The tombstone retires one *record*, not an opinion.
4. **Caches must honor tombstones.** Collection servers and advanced-client
   local stores delete their cached copy when they see a tombstone. A cache
   that never syncs again simply ages out; politeness rules (ADR 0013) already
   bound how stale a cache may be.
5. **Edits are unchanged.** Supersession per `(issuer, target)`, newest wins,
   remains the edit model (`/sync?latest_edits_only=true`). Deletion is
   orthogonal: an edit says "this is my current opinion"; a delete says "remove
   my record".

## Why not the alternatives

- **Supersede-only (the status quo).** Fails the right to be forgotten: the
  original stays retrievable forever via the container read. Acceptable for a
  trust-minimized relay swarm; not for author-owned feedback in an identified
  store.
- **Nostr kind-5-style deletion *requests*.** Right shape for a replication
  network where no server is authoritative; wrong here — the publication
  server *is* authoritative, so deletion there is deletion, not a plea.
- **Admin-only purge.** The author is the owner; erasure must not require
  operator intervention. (Operators can of course still purge out-of-band.)
- **Full-history immutability for auditability.** We deliberately trade
  perfect auditability for data ownership. Aggregates recompute after erasure;
  a signed statement you can no longer retrieve is simply no longer part of
  the record.

## Consequences

- `FeedbackStore` gains `delete(dedup_id)` + tombstone listing; all backends
  (oxigraph, sqlite, memory) implement it, and the conformance suite covers
  delete → query-gone → re-put-rejected.
- The feedback server adds the `DELETE` route, advertises it in `Allow`/CORS,
  and answers `410 Gone` for tombstoned ids on GET and re-POST.
- Sync consumers (collection server, advanced client) learn deletions from the
  tombstone feed and evict their copies.
- The CLI gains `freedback delete` (pairs with `write --key-file`, which is
  what makes the author's key durable enough to exercise ownership).
- Widgets: the browser identity (IndexedDB, issue #27) already persists, so a
  "delete my feedback" affordance is now possible — follow-up work.
- ADR 0002's consequence "edits are modeled as new annotations …" still holds
  for edits; its implicit "nothing is ever removed" no longer does. CLAUDE.md
  invariant 4's "append-only re-signed edits/deletes" clause is rewritten.
