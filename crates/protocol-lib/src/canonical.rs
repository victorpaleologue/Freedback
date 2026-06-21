//! Deterministic canonicalization and the content-addressed dedup id.
//!
//! The dedup id is the Freedback analogue of a Nostr NIP-01 event id, but it
//! resolves NIP-01's JSON-serialization ambiguity (flagged in nostr-protocol
//! issue #354) by using RFC 8785 JSON Canonicalization Scheme (JCS).
//!
//!   dedup_id = lowercase_hex( SHA-256( JCS( annotation \ {id, signature} ) ) )
//!
//! `id` (server-assigned) and `signature` (the proof itself) are removed so the
//! id is purely content-derived and identical re-POSTs are idempotent. The
//! `creator` IS included, so two different issuers asserting the same feedback
//! do NOT collapse.

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::model::Annotation;

/// Strip the non-content fields (`id`, `signature`) from an annotation's JSON.
fn content_value(ann: &Annotation) -> Result<serde_json::Value> {
    let mut v = serde_json::to_value(ann)?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("id");
        obj.remove("signature");
    }
    Ok(v)
}

/// The RFC 8785 canonical bytes that are hashed and signed.
///
/// This is the single source of truth for both the dedup id and the detached
/// signature, guaranteeing that a verifier hashes exactly what the signer did.
pub fn canonical_bytes(ann: &Annotation) -> Result<Vec<u8>> {
    let v = content_value(ann)?;
    serde_json_canonicalizer::to_vec(&v).map_err(|e| Error::Canonicalization(e.to_string()))
}

/// Compute the content-addressed dedup id (lowercase hex SHA-256).
pub fn dedup_id(ann: &Annotation) -> Result<String> {
    let bytes = canonical_bytes(ann)?;
    let digest = Sha256::digest(&bytes);
    Ok(hex_lower(&digest))
}

/// Lowercase hex encoding (no external dep).
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Body, Motivation, Target};

    fn sample() -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(4.0)],
        )
        .with_created("2026-06-21T10:00:00Z")
    }

    #[test]
    fn dedup_id_is_stable() {
        let a = sample();
        let b = sample();
        assert_eq!(dedup_id(&a).unwrap(), dedup_id(&b).unwrap());
    }

    #[test]
    fn dedup_id_ignores_server_id_and_signature() {
        let mut a = sample();
        let base = dedup_id(&a).unwrap();
        a.id = Some("https://server/annotations/abc".into());
        a.signature = Some(crate::model::Signature {
            alg: "ES256".into(),
            kid: "k".into(),
            sig: "s".into(),
        });
        assert_eq!(base, dedup_id(&a).unwrap());
    }

    #[test]
    fn dedup_id_changes_with_content() {
        let a = sample();
        let b = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(5.0)],
        )
        .with_created("2026-06-21T10:00:00Z");
        assert_ne!(dedup_id(&a).unwrap(), dedup_id(&b).unwrap());
    }
}
