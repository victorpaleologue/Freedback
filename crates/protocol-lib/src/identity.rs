//! Self-signed ECDSA P-256 identity (INVARIANT 4a — federates).
//!
//! Mirrors Mangrove's keypair/sign model but in pure Rust via the RustCrypto
//! `p256` crate, so it compiles to both native and `wasm32`. The public key
//! (PEM) is the portable issuer id; signatures are detached and computed over
//! the RFC 8785 canonical bytes (see `canonical.rs`).
//!
//! Browser widgets MAY instead sign via WebCrypto (`crypto.subtle`) so private
//! keys never enter WASM linear memory; both paths produce the same ES256
//! signature over the same canonical bytes (see `docs/adr/0003`).

use base64::Engine;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature as EcdsaSig, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use sha2::{Digest, Sha256};

use crate::canonical::canonical_bytes;
use crate::error::{Error, Result};
use crate::model::{Annotation, Signature};

const B64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A self-signed identity: a P-256 keypair.
#[derive(Clone)]
pub struct Identity {
    signing: SigningKey,
}

impl Identity {
    /// Generate a fresh random identity.
    pub fn generate() -> Self {
        // Sample a uniformly-random valid scalar without pulling in a separate
        // RNG crate: getrandom feeds 32 bytes, retry on the (vanishingly rare)
        // out-of-range value.
        loop {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).expect("getrandom failed");
            if let Ok(signing) = SigningKey::from_slice(&bytes) {
                return Self { signing };
            }
        }
    }

    /// Load an identity from a PKCS#8 PEM private key.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self> {
        let signing =
            SigningKey::from_pkcs8_pem(pem).map_err(|e| Error::KeyEncoding(e.to_string()))?;
        Ok(Self { signing })
    }

    /// Export the private key as a PKCS#8 PEM string.
    pub fn to_pkcs8_pem(&self) -> Result<String> {
        self.signing
            .to_pkcs8_pem(LineEnding::LF)
            .map(|s| s.to_string())
            .map_err(|e| Error::KeyEncoding(e.to_string()))
    }

    /// The verifying (public) key.
    pub fn verifying_key(&self) -> VerifyingKey {
        *self.signing.verifying_key()
    }

    /// The public key as SPKI PEM — used as the JWS `kid` (the portable issuer
    /// id, Mangrove-style).
    pub fn public_key_pem(&self) -> Result<String> {
        self.verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| Error::KeyEncoding(e.to_string()))
    }

    /// A stable, compact issuer id derived from the public key:
    /// `urn:freedback:key:<sha256-hex-of-SPKI-DER>`. Suitable for the
    /// annotation `creator.id`.
    pub fn issuer_id(&self) -> Result<String> {
        issuer_id_from_verifying_key(&self.verifying_key())
    }

    /// Sign an annotation in place, populating its `signature` field.
    pub fn sign_annotation(&self, ann: &mut Annotation) -> Result<()> {
        ann.signature = None; // ensure the signature is not part of signed bytes
        let bytes = canonical_bytes(ann)?;
        let sig: EcdsaSig = self.signing.sign(&bytes);
        ann.signature = Some(Signature {
            alg: "ES256".to_string(),
            kid: self.public_key_pem()?,
            sig: B64URL.encode(sig.to_bytes()),
        });
        Ok(())
    }
}

/// Derive the stable issuer id from a public key.
pub fn issuer_id_from_verifying_key(vk: &VerifyingKey) -> Result<String> {
    let der = vk
        .to_public_key_der()
        .map_err(|e| Error::KeyEncoding(e.to_string()))?;
    let digest = Sha256::digest(der.as_bytes());
    Ok(format!("urn:freedback:key:{}", hex_lower(&digest)))
}

/// Verify the detached signature carried by an annotation.
///
/// Recomputes the canonical bytes (with `id`/`signature` removed) and checks the
/// ECDSA signature against the public key embedded in `signature.kid`. Any
/// tampering with the content invalidates the signature.
pub fn verify_annotation(ann: &Annotation) -> Result<()> {
    let sig = ann
        .signature
        .as_ref()
        .ok_or(Error::MissingField("signature"))?;
    if sig.alg != "ES256" {
        return Err(Error::Crypto(format!("unsupported alg {:?}", sig.alg)));
    }
    let vk = VerifyingKey::from_public_key_pem(&sig.kid)
        .map_err(|e| Error::KeyEncoding(e.to_string()))?;

    // Verify over the same bytes the signer used: signature field removed.
    let mut unsigned = ann.clone();
    unsigned.signature = None;
    let bytes = canonical_bytes(&unsigned)?;

    let raw = B64URL
        .decode(sig.sig.as_bytes())
        .map_err(|e| Error::Crypto(format!("bad base64url signature: {e}")))?;
    let ecdsa = EcdsaSig::from_slice(&raw).map_err(|_| Error::SignatureInvalid)?;
    vk.verify(&bytes, &ecdsa)
        .map_err(|_| Error::SignatureInvalid)
}

use p256::pkcs8::DecodePublicKey;

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
    fn sign_and_verify_roundtrip() {
        let id = Identity::generate();
        let mut ann = sample();
        id.sign_annotation(&mut ann).unwrap();
        assert!(ann.signature.is_some());
        verify_annotation(&ann).unwrap();
    }

    #[test]
    fn tamper_is_rejected() {
        let id = Identity::generate();
        let mut ann = sample();
        id.sign_annotation(&mut ann).unwrap();
        // Tamper with the body after signing.
        ann.body = vec![Body::star(1.0)];
        assert!(matches!(
            verify_annotation(&ann),
            Err(Error::SignatureInvalid)
        ));
    }

    #[test]
    fn server_id_does_not_break_signature() {
        let id = Identity::generate();
        let mut ann = sample();
        id.sign_annotation(&mut ann).unwrap();
        // Server assigns an id after signing; signature must still verify.
        ann.id = Some("https://server/annotations/xyz".into());
        verify_annotation(&ann).unwrap();
    }

    #[test]
    fn pem_roundtrip() {
        let id = Identity::generate();
        let pem = id.to_pkcs8_pem().unwrap();
        let id2 = Identity::from_pkcs8_pem(&pem).unwrap();
        assert_eq!(id.public_key_pem().unwrap(), id2.public_key_pem().unwrap());
    }
}
