//! Full JSON-LD normalization (native, `jsonld` feature) for **arbitrary
//! third-party `@context`s** — the conformance path beyond the alias-based
//! [`crate::jsonld`].
//!
//! The alias normalizer ([`crate::jsonld::from_jsonld`]) resolves any
//! serialization expressed over the *pinned* Freedback/anno vocabulary (compact
//! terms, prefixed IRIs, single-or-array shapes). It cannot resolve a document
//! that names the same concepts with **its own** terms, e.g.
//!
//! ```json
//! { "@context": { "note": "http://www.w3.org/ns/oa#hasBody", … }, "note": … }
//! ```
//!
//! For that we run the real JSON-LD processor: **compact the incoming document
//! against our pinned `@context`**. Compaction expands the document with
//! whatever `@context` it declares (resolving the third party's terms to full
//! IRIs) and then re-serializes it using our term definitions — producing the
//! exact shape [`from_jsonld`](crate::jsonld::from_jsonld) already understands.
//! The model it yields feeds the same dedup id / signature path, so two
//! vocabularies for the same feedback content-address identically (ADR 0011).
//!
//! Native only: the `json-ld` crate pulls a large async stack and is not part of
//! the wasm core. The processor is driven by a **preloaded allowlist loader**
//! ([`preloaded_loader`]) seeded with the well-known remote contexts bundled at
//! compile time (the W3C Web Annotation context and a curated schema.org rating
//! subset). So both **inline** `@context`s *and* references to those well-known
//! **remote** `@context` URLs resolve offline; every other remote URL is refused
//! with no network call (no arbitrary fetch → no SSRF — issue #24, ADR 0011).

use std::collections::HashMap;

use iref::IriBuf;
use json_ld::RemoteDocument;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::Annotation;

/// The pinned `@context` document (the one served at [`crate::context::CONTEXT_URL`]),
/// embedded so compaction never depends on the network.
const PINNED_CONTEXT: &str = include_str!("../../../ontology/context.jsonld");

/// The canonical W3C Web Annotation context (verbatim copy; see
/// `ontology/vendor/README.md`), bundled so documents that reference it by URL
/// resolve offline.
const ANNO_CONTEXT: &str = include_str!("../../../ontology/vendor/anno.jsonld");

/// A curated subset of the schema.org context — the rating vocabulary Freedback's
/// typed bodies use. The full context is intentionally not bundled (see
/// `ontology/vendor/README.md`).
const SCHEMA_RATING_CONTEXT: &str = include_str!("../../../ontology/vendor/schema-rating.jsonld");

/// Normalize an arbitrary conformant JSON-LD annotation — *including ones using
/// a third-party `@context`* — into the canonical [`Annotation`] model.
///
/// Compacts the document against the pinned Freedback `@context`, then hands the
/// predictable result to [`from_jsonld`](crate::jsonld::from_jsonld).
pub fn normalize_full(doc: &Value) -> Result<Annotation> {
    let compacted = compact_to_pinned(doc)?;
    crate::jsonld::from_jsonld(&compacted)
}

/// Compact a JSON-LD document against the pinned Freedback `@context`.
///
/// Exposed for tests and callers that want the canonical serialization rather
/// than the parsed model.
pub fn compact_to_pinned(doc: &Value) -> Result<Value> {
    use json_ld::syntax::TryFromJson;
    use json_ld::JsonLdProcessor;
    use static_iref::iri;

    // The input document, tagged with a base IRI so relative references resolve.
    let input = json_ld::RemoteDocument::new(
        Some(iri!("https://freedback.net/.well-known/in").to_owned()),
        Some("application/ld+json".parse().expect("static media type")),
        json_syntax::Value::from_serde_json(doc.clone()),
    );

    // Our pinned `@context`, parsed once into the processor's context type.
    let ctx_doc: Value = serde_json::from_str(PINNED_CONTEXT)
        .map_err(|e| Error::Validation(format!("pinned context parse: {e}")))?;
    let ctx_inner = ctx_doc
        .get("@context")
        .cloned()
        .ok_or_else(|| Error::Validation("pinned context missing @context".into()))?;
    let context =
        json_ld::syntax::Context::try_from_json(json_syntax::Value::from_serde_json(ctx_inner))
            .map_err(|e| Error::Validation(format!("pinned context invalid: {e:?}")))?;
    let context_ref = json_ld::RemoteContextReference::Loaded(json_ld::RemoteDocument::new(
        Some(iri!("https://freedback.net/ns/context.jsonld").to_owned()),
        Some("application/ld+json".parse().expect("static media type")),
        context,
    ));

    let loader = preloaded_loader()?;
    let compacted = futures::executor::block_on(input.compact(context_ref, &loader))
        .map_err(|e| Error::Validation(format!("json-ld compact: {e}")))?;

    Ok(compacted.into_serde_json())
}

/// Build the offline document loader: a fixed allowlist mapping the well-known
/// remote `@context` URLs to their bundled documents.
///
/// This is the whole SSRF/availability story. A [`HashMap`] is a [`Loader`] that
/// serves exactly its entries and returns "not found" for anything else, so a
/// document referencing a *known* context URL (`anno.jsonld`, schema.org) resolves
/// from the embedded copy, while any *unknown* URL is rejected — and either way the
/// loader never touches the network.
///
/// [`Loader`]: json_ld::Loader
fn preloaded_loader() -> Result<HashMap<IriBuf, RemoteDocument>> {
    // The well-known contexts and every URL spelling clients reference them by.
    // anno is served at www.w3.org over both schemes; schema.org documents cite a
    // mix of trailing-slash / scheme / the explicit jsonldcontext.json.
    let entries: &[(&[&str], &str)] = &[
        (
            &[
                "http://www.w3.org/ns/anno.jsonld",
                "https://www.w3.org/ns/anno.jsonld",
            ],
            ANNO_CONTEXT,
        ),
        (
            &[
                "http://schema.org/",
                "https://schema.org/",
                "http://schema.org",
                "https://schema.org",
                "https://schema.org/docs/jsonldcontext.json",
            ],
            SCHEMA_RATING_CONTEXT,
        ),
    ];

    let mut map = HashMap::new();
    for (urls, body) in entries {
        let parsed: Value = serde_json::from_str(body)
            .map_err(|e| Error::Validation(format!("bundled context parse: {e}")))?;
        for url in *urls {
            let iri = IriBuf::new(url.to_string())
                .map_err(|e| Error::Validation(format!("bundled context url {url}: {e}")))?;
            let doc = RemoteDocument::new(
                Some(iri.clone()),
                Some("application/ld+json".parse().expect("static media type")),
                json_syntax::Value::from_serde_json(parsed.clone()),
            );
            map.insert(iri, doc);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::dedup_id;
    use crate::model::{Annotation, Body, Creator, Motivation, Target};

    fn canonical() -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(4.0)],
        )
        .with_created("2026-06-21T10:00:00Z")
        .with_creator(Creator::new("did:key:k1"))
    }

    #[test]
    fn normalizes_a_foreign_vocabulary_to_the_same_dedup_id() {
        // A third party describes the *same* feedback with entirely different
        // term names, bound to the canonical IRIs by its own inline @context.
        // The alias normalizer cannot read this; full compaction can.
        let foreign = serde_json::json!({
            "@context": {
                "Rating": "http://www.w3.org/ns/oa#Annotation",
                "about":   { "@id": "http://www.w3.org/ns/oa#hasTarget", "@type": "@id" },
                "why":     { "@id": "http://www.w3.org/ns/oa#motivatedBy", "@type": "@id" },
                "by":      { "@id": "http://purl.org/dc/terms/creator", "@type": "@id" },
                "on":      { "@id": "http://purl.org/dc/terms/created", "@type": "http://www.w3.org/2001/XMLSchema#dateTime" },
                "scores":  { "@id": "http://www.w3.org/ns/oa#hasBody", "@type": "@id" },
                "Stars":   "https://freedback.net/ns#StarRating",
                "stars":   { "@id": "http://schema.org/ratingValue", "@type": "http://www.w3.org/2001/XMLSchema#double" },
                "low":     { "@id": "http://schema.org/worstRating", "@type": "http://www.w3.org/2001/XMLSchema#double" },
                "high":    { "@id": "http://schema.org/bestRating", "@type": "http://www.w3.org/2001/XMLSchema#double" },
                "assessing": "http://www.w3.org/ns/oa#assessing"
            },
            "@type": "Rating",
            "why": "assessing",
            "by": "did:key:k1",
            "on": "2026-06-21T10:00:00Z",
            "about": "https://example.com/item/1",
            "scores": {
                "@type": "Stars",
                "stars": 4,
                "low": 1,
                "high": 5
            }
        });

        let parsed = normalize_full(&foreign).expect("foreign vocab should normalize");
        assert_eq!(
            dedup_id(&parsed).unwrap(),
            dedup_id(&canonical()).unwrap(),
            "a foreign @context for the same feedback must content-address identically"
        );
    }

    #[test]
    fn compacts_inline_custom_context_to_pinned_terms() {
        let foreign = serde_json::json!({
            "@context": {
                "Rating": "http://www.w3.org/ns/oa#Annotation",
                "about":  { "@id": "http://www.w3.org/ns/oa#hasTarget", "@type": "@id" },
                "why":    { "@id": "http://www.w3.org/ns/oa#motivatedBy", "@type": "@id" },
                "assessing": "http://www.w3.org/ns/oa#assessing",
                "scores": { "@id": "http://www.w3.org/ns/oa#hasBody", "@type": "@id" },
                "Note":   "http://www.w3.org/ns/oa#TextualBody",
                "text":   "http://www.w3.org/1999/02/22-rdf-syntax-ns#value"
            },
            "@type": "Rating",
            "why": "assessing",
            "about": "https://example.com/x",
            "scores": { "@type": "Note", "text": "nice" }
        });

        let compacted = compact_to_pinned(&foreign).unwrap();
        // Pinned terms appear after compaction (e.g. our "target"/"body" aliases).
        let s = serde_json::to_string(&compacted).unwrap();
        assert!(
            s.contains("\"target\"") && s.contains("\"body\""),
            "expected pinned terms in {s}"
        );
    }

    #[test]
    fn resolves_a_remote_anno_context_url_offline() {
        // The envelope (type/motivation/target/creator/created/body) is named with
        // the standard anno terms, supplied **only** by a reference to the remote
        // W3C context URL — never inline. The rating body terms (Freedback/schema)
        // are inline since the anno context does not define them. The preloaded
        // loader resolves the remote URL from the bundled copy, with no network.
        let foreign = serde_json::json!({
            "@context": [
                "http://www.w3.org/ns/anno.jsonld",
                {
                    "assessing": "http://www.w3.org/ns/oa#assessing",
                    "StarRating": "https://freedback.net/ns#StarRating",
                    "ratingValue": { "@id": "http://schema.org/ratingValue", "@type": "http://www.w3.org/2001/XMLSchema#double" },
                    "worstRating": { "@id": "http://schema.org/worstRating", "@type": "http://www.w3.org/2001/XMLSchema#double" },
                    "bestRating":  { "@id": "http://schema.org/bestRating",  "@type": "http://www.w3.org/2001/XMLSchema#double" }
                }
            ],
            "type": "Annotation",
            "motivation": "assessing",
            "creator": "did:key:k1",
            "created": "2026-06-21T10:00:00Z",
            "target": "https://example.com/item/1",
            "body": { "type": "StarRating", "ratingValue": 4, "worstRating": 1, "bestRating": 5 }
        });

        let parsed = normalize_full(&foreign).expect("remote anno context should resolve offline");
        assert_eq!(
            dedup_id(&parsed).unwrap(),
            dedup_id(&canonical()).unwrap(),
            "a remote anno @context URL must content-address identically to the canonical form"
        );
    }

    #[test]
    fn resolves_remote_anno_and_schema_context_urls_together() {
        // The realistic full case: the envelope comes from the remote anno context
        // and the rating body terms from the remote schema.org context. Only the two
        // genuinely Freedback-specific terms (the StarRating class, the assessing
        // motivation) are inline. Both remote URLs resolve from the bundled allowlist.
        let foreign = serde_json::json!({
            "@context": [
                "http://www.w3.org/ns/anno.jsonld",
                "http://schema.org/",
                {
                    "assessing": "http://www.w3.org/ns/oa#assessing",
                    "StarRating": "https://freedback.net/ns#StarRating"
                }
            ],
            "type": "Annotation",
            "motivation": "assessing",
            "creator": "did:key:k1",
            "created": "2026-06-21T10:00:00Z",
            "target": "https://example.com/item/1",
            "body": { "type": "StarRating", "ratingValue": 4, "worstRating": 1, "bestRating": 5 }
        });

        let parsed =
            normalize_full(&foreign).expect("remote anno + schema contexts should resolve offline");
        assert_eq!(
            dedup_id(&parsed).unwrap(),
            dedup_id(&canonical()).unwrap(),
            "remote anno + schema @context URLs must content-address identically"
        );
    }

    #[test]
    fn rejects_an_unknown_remote_context_url_without_network() {
        // An unknown remote @context URL is not in the allowlist, so the loader
        // returns "not found" rather than fetching it — compaction fails and the
        // document is rejected. (The loader has no network backend at all, so this
        // can only ever be an offline rejection.)
        let foreign = serde_json::json!({
            "@context": "http://not-a-well-known-context.example/ctx.jsonld",
            "type": "Annotation",
            "motivation": "assessing",
            "target": "https://example.com/item/1"
        });

        let err = normalize_full(&foreign).expect_err("unknown remote @context must be rejected");
        assert!(
            matches!(err, Error::Validation(_)),
            "unknown remote @context should surface as a validation error, got {err:?}"
        );
    }

    #[test]
    fn preloaded_loader_keys_cover_both_url_schemes() {
        // Guard the allowlist: both well-known contexts are reachable under the
        // http and https spellings clients actually use.
        let loader = preloaded_loader().unwrap();
        for url in [
            "http://www.w3.org/ns/anno.jsonld",
            "https://www.w3.org/ns/anno.jsonld",
            "http://schema.org/",
            "https://schema.org/",
        ] {
            let iri = IriBuf::new(url.to_string()).unwrap();
            assert!(
                loader.contains_key(&iri),
                "missing allowlist entry for {url}"
            );
        }
    }
}
