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
//! - `wasm`: browser build marker (enables the `js` RNG backend). WASM
//!   consumers use `default-features = false`.
//!
//! Validation is a shapes-driven SHACL-Core-subset interpreter ([`validation`])
//! built on pure-Rust RDF parsers, so it runs on **both** native and `wasm32`
//! (widgets can pre-validate). Full JSON-LD interop (expanding/compacting
//! *external* annotations via the `json-ld` crate) is a later milestone; our own
//! pipeline emits compact JSON-LD directly and converts to RDF via [`rdf`].

pub mod canonical;
pub mod context;
pub mod error;
pub mod identity;
pub mod model;
pub mod rdf;
pub mod validation;

pub use canonical::{canonical_bytes, dedup_id};
pub use error::{Error, Result};
pub use identity::{verify_annotation, Identity};
pub use model::{Annotation, Body, Creator, Motivation, Selector, Signature, Target};
pub use validation::{validate_annotation, ValidationOutcome, Validator};
