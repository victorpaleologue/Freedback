//! # Freedback protocol library
//!
//! The shared core for every Freedback component. Native + `wasm32`.
//!
//! The native wire format is a W3C Web Annotation (JSON-LD): `target` + `body` +
//! `motivation` (+ optional `Selector`). Typed feedback lives in the body as
//! subclasses of `schema:Rating`. See `CLAUDE.md` for the fixed invariants this
//! crate must never violate, and `docs/` for the design rationale.
//!
//! ## Feature flags
//! - `validation` (default, native): shapes-driven SHACL-Core-subset validation
//!   ([`validation`]) built on the pure-Rust `oxrdf`/`oxttl` parsers.
//! - `wasm`: browser build marker (enables the `js` RNG backend). WASM consumers
//!   use `default-features = false` (+ `wasm`), which omits `validation` and the
//!   RDF dependency chain; they rely on the server to validate on write (see
//!   `docs/adr/0004`). The model, canonicalization, dedup id, and P-256 signing
//!   all remain available in the browser.
//!
//! JSON-LD ingest is **primary**: [`jsonld::from_jsonld`] normalizes any
//! conformant W3C Web Annotation serialization into the canonical model before
//! the dedup id / signature are computed (ADR 0007), and the model → RDF mapping
//! in [`rdf`] feeds SHACL. Both are pure Rust (native + `wasm32`).
//!
//! Documents that name the same concepts with a **third party's own
//! `@context`** are handled by [`jsonld_full::normalize_full`] (the `jsonld`
//! feature, native): it compacts the document against the pinned Freedback
//! context via the real `json-ld` processor, so a foreign vocabulary
//! content-addresses identically (ADR 0011). The server tries the fast alias
//! normalizer first and falls back to full compaction.

pub mod canonical;
pub mod context;
pub mod error;
pub mod export;
pub mod identity;
pub mod jsonld;
pub mod mangrove;
pub mod model;
pub mod rdf;

#[cfg(feature = "jsonld")]
pub mod jsonld_full;

#[cfg(feature = "validation")]
pub mod validation;

pub use canonical::{canonical_bytes, canonical_json, dedup_id};
pub use error::{Error, Result};
pub use export::{from_jwt, to_jwt};
pub use identity::{verify_annotation, Identity};
pub use jsonld::from_jsonld;
pub use mangrove::{from_mangrove_jwt, to_mangrove_jwt};
pub use model::{Annotation, Body, Creator, Motivation, Selector, Signature, Target};

#[cfg(feature = "validation")]
pub use validation::{validate_annotation, ValidationOutcome, Validator};
