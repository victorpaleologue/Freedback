//! JWT **export profile** (INVARIANT 1: the JWT is an export/transport profile,
//! never the native format).
//!
//! Mirrors Mangrove's `submitReview`: an annotation is carried as a compact ES256
//! JWS (JWT), `header.payload.signature`, where the header `kid` is the issuer's
//! SPKI PEM public key and the payload is the annotation. The JWT signature is
//! the issuer proof, so a server can accept `PUT /submit/{jwt}` and trust it
//! exactly like a self-signed annotation. On ingest the payload is normalized
//! through [`crate::jsonld::from_jsonld`], so any conformant serialization works.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::identity::{issuer_id_from_pem, verify_es256, Identity};
use crate::jsonld::from_jsonld;
use crate::model::{Annotation, Creator};

/// Encode an annotation as an ES256 JWT signed by `identity`.
///
/// The inner detached `signature` and server-assigned `id` are dropped from the
/// payload — the JWT signature replaces the former and the latter is server-set.
pub fn to_jwt(ann: &Annotation, identity: &Identity) -> Result<String> {
    let mut payload = serde_json::to_value(ann)?;
    if let Some(obj) = payload.as_object_mut() {
        obj.remove("signature");
        obj.remove("id");
    }
    let header = json!({ "alg": "ES256", "typ": "JWT", "kid": identity.public_key_pem()? });

    let h = B64.encode(serde_json::to_vec(&header)?);
    let p = B64.encode(serde_json::to_vec(&payload)?);
    let signing_input = format!("{h}.{p}");
    let sig = identity.sign_es256(signing_input.as_bytes());
    Ok(format!("{signing_input}.{sig}"))
}

/// Decode and verify an ES256 JWT, returning the normalized annotation.
///
/// The issuer (`kid`) is stamped as the `creator` when the payload omits one.
pub fn from_jwt(jwt: &str) -> Result<Annotation> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Crypto("malformed JWT (expected 3 parts)".into()));
    }
    let (h, p, s) = (parts[0], parts[1], parts[2]);

    let header: Value = serde_json::from_slice(
        &B64.decode(h)
            .map_err(|e| Error::Crypto(format!("bad header b64: {e}")))?,
    )?;
    if header.get("alg").and_then(Value::as_str) != Some("ES256") {
        return Err(Error::Crypto("unsupported JWT alg (expected ES256)".into()));
    }
    let kid = header
        .get("kid")
        .and_then(Value::as_str)
        .ok_or(Error::MissingField("kid"))?;

    // Verify the signature over `header.payload`.
    verify_es256(kid, format!("{h}.{p}").as_bytes(), s)?;

    let payload: Value = serde_json::from_slice(
        &B64.decode(p)
            .map_err(|e| Error::Crypto(format!("bad payload b64: {e}")))?,
    )?;
    let mut ann = from_jsonld(&payload)?;
    if ann.creator.is_none() {
        ann.creator = Some(Creator::new(issuer_id_from_pem(kid)?));
    }
    Ok(ann)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, Motivation, Target};

    fn sample() -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(4.0)],
        )
        .with_created("2026-06-21T10:00:00Z")
    }

    #[test]
    fn jwt_round_trips() {
        let id = Identity::generate();
        let jwt = to_jwt(&sample(), &id).unwrap();
        assert_eq!(jwt.split('.').count(), 3);
        let back = from_jwt(&jwt).unwrap();
        assert_eq!(back.target.source(), "https://example.com/item/1");
        // Creator stamped from the JWT issuer.
        assert_eq!(back.creator.unwrap().id, id.issuer_id().unwrap());
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let id = Identity::generate();
        let jwt = to_jwt(&sample(), &id).unwrap();
        let mut parts: Vec<&str> = jwt.split('.').collect();
        // Replace the payload with a different one (re-encoded), keep the sig.
        let forged = B64.encode(br#"{"@context":"x","type":"Annotation","motivation":"assessing","target":"https://evil/","body":[{"type":["freedback:StarRating","schema:Rating"],"schema:ratingValue":1}]}"#);
        parts[1] = &forged;
        let tampered = parts.join(".");
        assert!(from_jwt(&tampered).is_err(), "tampered payload must fail");
    }

    #[test]
    fn malformed_is_rejected() {
        assert!(from_jwt("not.a.jwt.really").is_err());
        assert!(from_jwt("only-one-part").is_err());
    }
}
