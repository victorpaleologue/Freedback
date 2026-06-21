//! Error types for the Freedback protocol library.

use thiserror::Error;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can arise when handling Freedback annotations.
#[derive(Debug, Error)]
pub enum Error {
    /// A field required by the protocol or by SHACL is missing.
    #[error("missing required field: {0}")]
    MissingField(&'static str),

    /// A value was outside the bounds declared by the rating type / SHACL shape.
    #[error("value out of bounds: {0}")]
    OutOfBounds(String),

    /// JSON (de)serialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// RFC 8785 canonicalization failure.
    #[error("canonicalization error: {0}")]
    Canonicalization(String),

    /// Cryptographic / signature failure.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// Key encoding / decoding (PEM, PKCS#8, SEC1) failure.
    #[error("key encoding error: {0}")]
    KeyEncoding(String),

    /// Signature verification failed (tampered payload or wrong key).
    #[error("signature verification failed")]
    SignatureInvalid,

    /// SHACL validation reported one or more violations.
    #[error("validation failed: {0}")]
    Validation(String),

    /// A capability requested at runtime was not compiled in (feature gate).
    #[error("feature not enabled: {0}")]
    FeatureDisabled(&'static str),
}
