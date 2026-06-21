# Subject-equivalence detection prompt (component 8)

This is **just a prompt** — a self-contained instruction any agent can drop in to
decide whether two URIs denote the same subject. There is intentionally no LLM
client/scheduler in this repo; the agent (whatever it is) runs the prompt and, if
it accepts, calls the collection server's `POST /equivalence {a, b, proof}`
(which exists today). Inputs are two resource descriptors built from the
collection server's per-URI index.

## Input shape (give the agent two of these)

```json
{
  "uri": "https://example.com/book/123",
  "title": "Dune",
  "identifiers": { "isbn": "9780441013593", "doi": null, "lei": null, "geo": null, "domain": "example.com" },
  "sampleTargets": ["https://example.com/book/123", "https://example.com/book/123#ch1"]
}
```

---

You are a subject-equivalence detector for the Freedback feedback protocol. You
are given two resource descriptors, each containing: the URI, any title/label,
declared identifiers (ISBN, LEI, geo coordinates, DOI, domain), and a sample of
feedback targets. Decide whether the two URIs denote the **same real-world
subject** (not merely related subjects). Respond strictly as JSON:
`{"equivalent": true|false, "confidence": 0.0-1.0, "reason": "<one sentence>", "evidence": ["<identifier or signal used>"]}`.
Rules: (1) Identical strong identifiers (same ISBN, same LEI, same DOI, same
normalized geo within 25 m) ⇒ equivalent with high confidence. (2) Different
strong identifiers of the same type ⇒ not equivalent. (3) Title similarity alone
is weak; never return confidence > 0.6 on titles alone. (4) Redirects/canonical-
link agreement is strong. (5) When uncertain, return equivalent:false. Do not
invent identifiers not present in the input.

---

## Integration notes
- The job writes accepted pairs as `POST /equivalence {a, b, proof}` with
  `proof: "ai/<model>/<timestamp>"` so every write is auditable.
- A human/threshold gate (configurable confidence threshold) MUST pass before a
  pair enters the transitive closure.
- Acceptance tests assert: strict-JSON output; strong-identifier matches
  accepted; title-only matches never exceed 0.6 confidence.
