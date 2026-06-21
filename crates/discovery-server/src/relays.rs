//! NIP-65-style **relay list** for the outbox discovery model.
//!
//! Instead of (only) polling every announced server for a target, an *issuer*
//! declares — in a self-signed, replaceable record — which feedback servers it
//! **writes** its feedback to and **reads** aggregates from. To find a given
//! key's feedback you consult the servers it declared it writes to (Nostr's
//! "outbox model"), rather than fanning out across the whole registry.
//!
//! The record federates because it is signed by the issuer's P-256 key
//! (INVARIANT 4a): the signature is ES256 over the RFC 8785 canonical bytes of
//! the record minus its `signature`, and the declared `issuer` must equal the id
//! derived from the signing key. It is *replaceable*: a newer `updated` wins.

use freedback_protocol::identity::{issuer_id_from_pem, verify_es256};
use freedback_protocol::{canonical_json, Identity, Signature};
use serde::{Deserialize, Serialize};

/// A self-signed declaration of where an issuer publishes / reads feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayList {
    /// The issuer id (`urn:freedback:key:<sha256(SPKI DER)>`).
    pub issuer: String,
    /// Servers the issuer reads aggregates from.
    #[serde(default)]
    pub read: Vec<String>,
    /// Servers the issuer publishes its feedback to (the outbox set).
    #[serde(default)]
    pub write: Vec<String>,
    /// `xsd:dateTime` (ISO 8601, UTC). Newer replaces older for the same issuer.
    pub updated: String,
    /// Detached ES256 self-signature over the canonical bytes (without itself).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

impl RelayList {
    /// Build an unsigned relay list.
    pub fn new(
        issuer: impl Into<String>,
        read: Vec<String>,
        write: Vec<String>,
        updated: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            read,
            write,
            updated: updated.into(),
            signature: None,
        }
    }

    /// RFC 8785 canonical bytes of the record with `signature` removed.
    fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(o) = v.as_object_mut() {
            o.remove("signature");
        }
        canonical_json(&v).map_err(|e| e.to_string())
    }

    /// Sign in place with the issuer's identity (sets `signature`).
    pub fn sign(&mut self, id: &Identity) -> Result<(), String> {
        self.signature = None;
        let bytes = self.canonical_bytes()?;
        self.signature = Some(Signature {
            alg: "ES256".to_string(),
            kid: id.public_key_pem().map_err(|e| e.to_string())?,
            sig: id.sign_es256(&bytes),
        });
        Ok(())
    }

    /// Verify the self-signature **and** that `issuer` matches the signing key.
    ///
    /// Both checks matter: the signature proves authenticity, and the
    /// issuer/key binding stops a valid signature over key A from claiming to be
    /// issuer B's relay list.
    pub fn verify(&self) -> Result<(), String> {
        let sig = self.signature.as_ref().ok_or("relay list is unsigned")?;
        if sig.alg != "ES256" {
            return Err(format!("unsupported alg {:?}", sig.alg));
        }
        let derived = issuer_id_from_pem(&sig.kid).map_err(|e| e.to_string())?;
        if derived != self.issuer {
            return Err("issuer does not match signing key".to_string());
        }
        let bytes = self.canonical_bytes()?;
        verify_es256(&sig.kid, &bytes, &sig.sig).map_err(|_| "signature invalid".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_roundtrips() {
        let id = Identity::generate();
        let mut rl = RelayList::new(
            id.issuer_id().unwrap(),
            vec!["http://read.example".into()],
            vec!["http://write.example".into()],
            "2026-06-21T10:00:00Z",
        );
        rl.sign(&id).unwrap();
        rl.verify().unwrap();
    }

    #[test]
    fn tampering_breaks_verification() {
        let id = Identity::generate();
        let mut rl = RelayList::new(
            id.issuer_id().unwrap(),
            vec![],
            vec!["http://write.example".into()],
            "2026-06-21T10:00:00Z",
        );
        rl.sign(&id).unwrap();
        rl.write.push("http://evil.example".into()); // after signing
        assert!(rl.verify().is_err());
    }

    #[test]
    fn wrong_issuer_claim_is_rejected() {
        let id = Identity::generate();
        let other = Identity::generate();
        // Claim to be `other` but sign with `id`.
        let mut rl = RelayList::new(
            other.issuer_id().unwrap(),
            vec![],
            vec!["http://write.example".into()],
            "2026-06-21T10:00:00Z",
        );
        rl.sign(&id).unwrap();
        assert!(rl.verify().is_err(), "issuer/key mismatch must be rejected");
    }
}
