//! Right-to-erasure delete documents (ADR 0021).
//!
//! An author erases their own annotation by presenting a **delete document**
//! signed with the *same* P-256 key that signed the annotation. The document's
//! canonical JSON shape is:
//!
//! ```json
//! {"type":"Delete","annotation":"<dedup_id>","created":"<RFC3339>"}
//! ```
//!
//! plus an optional detached [`Signature`] (`{alg, kid, sig}`) — the exact
//! machinery annotations use (ADR 0002/0003): the ES256 signature is computed
//! over the RFC 8785 (JCS) canonical bytes of the document **without** the
//! `signature` field, the `kid` is the signer's SPKI PEM public key, and the
//! `sig` is base64url (no pad). No new cryptography.
//!
//! Pure serde + p256, so this module is available on both native and `wasm32`.

use serde::{Deserialize, Serialize};

use crate::canonical::canonical_json;
use crate::error::{Error, Result};
use crate::identity::{issuer_id_from_pem, verify_es256, Identity};
use crate::model::Signature;

/// The `type` value of a delete document.
pub const DELETE_TYPE: &str = "Delete";

/// A right-to-erasure delete request (ADR 0021).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRequest {
    /// Always `"Delete"`.
    #[serde(rename = "type")]
    pub type_: String,
    /// The content-addressed dedup id of the annotation to erase.
    pub annotation: String,
    /// When the delete was issued (`xsd:dateTime`, RFC 3339 UTC).
    pub created: String,
    /// Detached ES256 signature over the canonical bytes (the document without
    /// this field). Absent on the OAuth-authorized path, where the bearer
    /// token — not a key — proves ownership.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

impl DeleteRequest {
    /// Build an unsigned delete request for `dedup_id`, issued at `created`.
    pub fn new(dedup_id: impl Into<String>, created: impl Into<String>) -> Self {
        Self {
            type_: DELETE_TYPE.to_string(),
            annotation: dedup_id.into(),
            created: created.into(),
            signature: None,
        }
    }

    /// The RFC 8785 (JCS) canonical bytes that are signed: the document with
    /// the `signature` field removed. The single source of truth for both the
    /// signer and the verifier, mirroring `canonical_bytes` for annotations.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut v = serde_json::to_value(self)?;
        if let Some(obj) = v.as_object_mut() {
            obj.remove("signature");
        }
        canonical_json(&v)
    }
}

impl Identity {
    /// Sign a delete request in place, populating its `signature` field —
    /// the erasure counterpart of [`Identity::sign_annotation`].
    pub fn sign_delete(&self, doc: &mut DeleteRequest) -> Result<()> {
        doc.signature = None; // ensure the signature is not part of signed bytes
        let bytes = doc.canonical_bytes()?;
        let sig = self.sign_es256(&bytes);
        doc.signature = Some(Signature {
            alg: "ES256".to_string(),
            kid: self.public_key_pem()?,
            sig,
        });
        Ok(())
    }
}

/// Verify the detached signature carried by a delete request.
///
/// Checks the document shape (`type == "Delete"`, `alg == "ES256"`) and the
/// ECDSA signature over the canonical bytes against the public key embedded in
/// `signature.kid`. On success returns the **issuer id** derived from that key
/// (the same `urn:freedback:key:<sha256-hex-of-SPKI-DER>` form annotations use
/// for `creator.id`), so the caller can check it matches the annotation's
/// creator. The raw `kid` PEM stays available on the document itself.
pub fn verify_delete(doc: &DeleteRequest) -> Result<String> {
    if doc.type_ != DELETE_TYPE {
        return Err(Error::OutOfBounds(format!(
            "type must be \"Delete\", got {:?}",
            doc.type_
        )));
    }
    let sig = doc
        .signature
        .as_ref()
        .ok_or(Error::MissingField("signature"))?;
    if sig.alg != "ES256" {
        return Err(Error::Crypto(format!("unsupported alg {:?}", sig.alg)));
    }

    // Verify over the same bytes the signer used: signature field removed.
    let mut unsigned = doc.clone();
    unsigned.signature = None;
    let bytes = unsigned.canonical_bytes()?;
    verify_es256(&sig.kid, &bytes, &sig.sig)?;

    issuer_id_from_pem(&sig.kid)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic fixed keypairs + timestamps (repo testing rules).
    const DEDUP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const CREATED: &str = "2026-07-05T12:00:00Z";

    /// A deterministic identity derived from a fixed scalar (not from PEM, to
    /// avoid embedding a real key blob in the source; the scalar is fixed so
    /// signatures are reproducible across runs).
    fn fixed_identity(byte: u8) -> Identity {
        // p256 scalars from a fixed 32-byte seed are always valid for small
        // nonzero seeds like these.
        let pem = {
            use p256::ecdsa::SigningKey;
            use p256::pkcs8::{EncodePrivateKey, LineEnding};
            let sk = SigningKey::from_slice(&[byte; 32]).unwrap();
            sk.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
        };
        Identity::from_pkcs8_pem(&pem).unwrap()
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let id = fixed_identity(7);
        let mut doc = DeleteRequest::new(DEDUP, CREATED);
        id.sign_delete(&mut doc).unwrap();
        assert!(doc.signature.is_some());
        let issuer = verify_delete(&doc).unwrap();
        assert_eq!(issuer, id.issuer_id().unwrap());
        assert!(issuer.starts_with("urn:freedback:key:"));
    }

    #[test]
    fn wrong_key_is_rejected() {
        let signer = fixed_identity(7);
        let other = fixed_identity(9);
        let mut doc = DeleteRequest::new(DEDUP, CREATED);
        signer.sign_delete(&mut doc).unwrap();
        // Swap in another identity's kid: the signature no longer verifies.
        doc.signature.as_mut().unwrap().kid = other.public_key_pem().unwrap();
        assert!(matches!(verify_delete(&doc), Err(Error::SignatureInvalid)));
    }

    #[test]
    fn tamper_is_rejected() {
        let id = fixed_identity(7);
        let mut doc = DeleteRequest::new(DEDUP, CREATED);
        id.sign_delete(&mut doc).unwrap();
        doc.annotation = "f".repeat(64); // point the delete at another record
        assert!(matches!(verify_delete(&doc), Err(Error::SignatureInvalid)));
    }

    #[test]
    fn canonical_bytes_are_stable_and_exclude_signature() {
        let id = fixed_identity(7);
        let mut doc = DeleteRequest::new(DEDUP, CREATED);
        let before = doc.canonical_bytes().unwrap();
        id.sign_delete(&mut doc).unwrap();
        let after = doc.canonical_bytes().unwrap();
        assert_eq!(before, after, "signature must not change the signed bytes");
        // The canonical JCS form is exactly the ADR 0021 shape (keys sorted).
        assert_eq!(
            String::from_utf8(before).unwrap(),
            format!(r#"{{"annotation":"{DEDUP}","created":"{CREATED}","type":"Delete"}}"#)
        );
    }

    #[test]
    fn missing_signature_is_rejected() {
        let doc = DeleteRequest::new(DEDUP, CREATED);
        assert!(matches!(
            verify_delete(&doc),
            Err(Error::MissingField("signature"))
        ));
    }

    #[test]
    fn wrong_type_is_rejected() {
        let id = fixed_identity(7);
        let mut doc = DeleteRequest::new(DEDUP, CREATED);
        doc.type_ = "Annotation".into();
        id.sign_delete(&mut doc).unwrap();
        assert!(verify_delete(&doc).is_err());
    }
}
