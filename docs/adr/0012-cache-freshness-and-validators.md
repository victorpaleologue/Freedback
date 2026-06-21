# ADR 0012 — HTTP cache freshness + validators between collection and feedback servers

- **Status:** accepted
- **Date:** 2026-06-21
- **Closes:** the M6 "`Cache-Control`/`Last-Modified` honoring" deferral (#7).
- **Builds on:** the conditional-GET (`ETag`/`If-None-Match`) revalidation already
  in M6.

## Context

The collection server aggregates a URI's feedback by fanning out to every
registered feedback server on each `/index`. The first cut was already polite —
it cached per `(server, uri)` and revalidated with `If-None-Match`, so a repeat
query cost a cheap `304`. But a `304` is still a **round-trip per upstream per
query**: under steady polling that is a lot of requests that only ever say
"nothing changed".

HTTP already solves this with two distinct mechanisms we were not using:

- **Freshness** (`Cache-Control: max-age`) — a cache may reuse a response with
  *no network request at all* until it goes stale.
- **The `Last-Modified` validator** (`If-Modified-Since`) — a second way to earn
  a `304`, for upstreams (or proxies) that key on modification time rather than
  an opaque `ETag`.

## Decision

Implement both, end to end.

**Feedback server (origin).** `build_page` now emits:

- `Cache-Control: max-age=<N>` — `N` is `AppState::cache_max_age` (default 30s,
  `with_cache_max_age` to override, `0` to force revalidation).
- `Last-Modified: <IMF-fixdate>` — the newest `created` time among the items on
  the page (the representation's modification time). Empty page ⇒ omitted.

It also **honors** conditional requests per RFC 7232 precedence: `If-None-Match`
first (ETag), then `If-Modified-Since` (`304` when the page's `Last-Modified` is
not newer than the client's date). HTTP-date handling is a tiny local
`httpdate` module (IMF-fixdate only — the form a sender must produce).

**Collection server (cache).** The per-`(server, uri)` entry gains
`last_modified` and a `fresh_until: Instant` deadline parsed from the upstream
`max-age`. On fetch:

1. **Fresh** ⇒ reuse the cached items with **no upstream request and no rate
   budget spent** (counted as a `cacheHits` metric).
2. **Stale** ⇒ revalidate, sending *both* validators we hold (`If-None-Match` +
   `If-Modified-Since`); a `304` refreshes `fresh_until` from the response.
3. A `200` restocks items + validators + freshness; `Cache-Control: no-store`
   suppresses caching.

## Why these specifics

- **Freshness beats revalidation under polling.** The common case (content
  unchanged within `max-age`) now costs *zero* upstream traffic, not one `304`
  each. The `fresh_cache_serves_without_any_upstream_call` test asserts exactly
  this: four queries, one upstream call, zero `304`s, ≥3 cache hits.
- **Both validators, not just ETag.** Sending `If-Modified-Since` alongside
  `If-None-Match` costs nothing and lets the aggregator interoperate with origins
  or intermediaries that only support modification-time validation.
- **IMF-fixdate only.** We never generate the obsolete RFC 850 / asctime forms,
  so parsing them would be dead code; an unrecognized date is treated as "no
  condition" (safe: a full `200`).
- **`max-age` is configurable, default short (30s).** Feedback is append-mostly
  and latency-tolerant; a short freshness window bounds staleness while still
  collapsing bursts. Tests pin `0` (force revalidation) or a large value (prove
  the freshness short-circuit) to isolate each behavior.

## Consequences

- Steady-state aggregation traffic drops from one conditional request per query
  to roughly one per `max-age` window per `(server, uri)`.
- Staleness is bounded by `max-age` (default 30s) — an explicit, tunable
  trade-off, surfaced in `/.well-known` as the existing `polite-cache`
  capability.
- New observability: `cacheHits` in `/debug/metrics` next to `upstreamCalls` /
  `upstream304`.
- Still in-memory: persistent index/cache across restarts remains future work.
