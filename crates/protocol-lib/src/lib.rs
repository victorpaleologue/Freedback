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
//! in [`rdf`] feeds SHACL. Both are pure Rust (native + `wasm32`). Full
//! expansion of arbitrary third-party `@context`s via the `json-ld` crate is the
//! documented extension.

pub mod canonical;
pub mod context;
pub mod error;
pub mod identity;
pub mod jsonld;
pub mod model;
pub mod rdf;

#[cfg(feature = "validation")]
pub mod validation;

pub use canonical::{canonical_bytes, dedup_id};
pub use error::{Error, Result};
pub use identity::{verify_annotation, Identity};
pub use jsonld::from_jsonld;
pub use model::{Annotation, Body, Creator, Motivation, Selector, Signature, Target};

#[cfg(feature = "validation")]
pub use validation::{validate_annotation, ValidationOutcome, Validator};
