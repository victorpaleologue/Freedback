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
//! the wasm core. The processor uses [`json_ld::NoLoader`], so **inline**
//! `@context`s resolve offline; documents that reference a *remote* `@context`
//! URL need a fetching/preloaded loader (future work — we do not perform network
//! I/O on the validation path by default).

use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::Annotation;

/// The pinned `@context` document (the one served at [`crate::context::CONTEXT_URL`]),
/// embedded so compaction never depends on the network.
const PINNED_CONTEXT: &str = include_str!("../../../ontology/context.jsonld");

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
        Some(iri!("https://freedback.org/.well-known/in").to_owned()),
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
        Some(iri!("https://freedback.org/ns/context.jsonld").to_owned()),
        Some("application/ld+json".parse().expect("static media type")),
        context,
    ));

    let loader = json_ld::NoLoader;
    let compacted = futures::executor::block_on(input.compact(context_ref, &loader))
        .map_err(|e| Error::Validation(format!("json-ld compact: {e}")))?;

    Ok(compacted.into_serde_json())
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
                "Stars":   "https://freedback.org/ns#StarRating",
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
}
