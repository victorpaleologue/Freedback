//! Stable IRIs, namespaces, and the pinned JSON-LD `@context` for Freedback.
//!
//! INVARIANT (see `CLAUDE.md`): these IRIs MUST be served as stable URLs and
//! MUST NOT change once published. The `@context` value embedded in every
//! native annotation is pinned via `dcterms:conformsTo` so that consumers can
//! validate against the exact profile that produced the data.

/// Web Annotation namespace.
pub const OA: &str = "http://www.w3.org/ns/oa#";
/// schema.org namespace (note: `http`, not `https`, is the canonical `@context` form).
pub const SCHEMA: &str = "http://schema.org/";
/// SKOS namespace (used for motivation `skos:broader` relations).
pub const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
/// Dublin Core terms (used for `dcterms:conformsTo`).
pub const DCTERMS: &str = "http://purl.org/dc/terms/";
/// RDF Schema namespace.
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

/// The Freedback vocabulary namespace. MUST resolve to the served ontology.
pub const FREEDBACK: &str = "https://freedback.net/ns#";

/// Stable URL of the pinned JSON-LD `@context` document (`ontology/context.jsonld`).
pub const CONTEXT_URL: &str = "https://freedback.net/ns/context.jsonld";
/// Stable URL of the profile this build conforms to (pinned via `dcterms:conformsTo`).
pub const PROFILE_URL: &str = "https://freedback.net/profile/1";
/// Protocol identifier advertised in `/.well-known/freedback`.
pub const PROTOCOL_ID: &str = "freedback/1";

/// The single canonical media type for the native wire format.
pub const MEDIA_TYPE: &str = "application/ld+json";

/// The `@context` array embedded in every native annotation.
///
/// We pin the W3C Web Annotation context plus our own served context document.
/// Widgets and WASM clients SHOULD ship pre-compacted payloads using exactly
/// this `@context` (see `docs/adr/0004`).
pub fn context_value() -> serde_json::Value {
    serde_json::json!(["http://www.w3.org/ns/anno.jsonld", CONTEXT_URL])
}
