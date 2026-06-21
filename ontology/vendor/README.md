# Vendored well-known `@context` documents

These are bundled into `protocol-lib` at compile time (`include_str!`) and served
**offline** to the JSON-LD compaction path
(`crates/protocol-lib/src/jsonld_full.rs`). They let a document that references a
**remote** `@context` URL normalize without any network I/O — the loader is a
fixed allowlist; every other URL is refused (no arbitrary fetch → no SSRF). See
ADR 0011 (and the loader section added for issue #24).

| File | Served at (loader keys) | Provenance |
|---|---|---|
| `anno.jsonld` | `http(s)://www.w3.org/ns/anno.jsonld` | Verbatim copy of the W3C Web Annotation context. Source: `w3c/web-annotation` `gh-pages` `jsonld/anno.jsonld`. SHA-256 `48c6e5e80f86bb1bf812233d13e70fab8b19516ac4e57e1e4576473f0bbd062e`. |
| `schema-rating.jsonld` | `http(s)://schema.org/`, `http(s)://schema.org`, `https://schema.org/docs/jsonldcontext.json` | **Curated subset** of the schema.org context — only the rating vocabulary Freedback's typed bodies use (`ratingValue`, `bestRating`, `worstRating`, `ratingCount`, `reviewCount`, `Rating`, `AggregateRating`). The full schema.org context (~205 KB, with a global `@vocab` catch-all that would mis-expand foreign terms) is intentionally **not** bundled. Rating terms are typed `xsd:double` to match the pinned Freedback context so values coerce cleanly on compaction. |

## Updating

`anno.jsonld` MUST stay a verbatim copy of the canonical W3C document; re-fetch
from the source above and update the SHA-256 here if it ever changes. The
`schema-rating.jsonld` subset is ours to extend as Freedback's rating vocabulary
grows, but it must keep the schema.org IRIs intact.
