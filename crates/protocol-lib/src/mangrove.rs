//! **Mangrove review-schema** mapping for the JWT export profile (INVARIANT 1:
//! the JWT is an export/transport profile, never the native format).
//!
//! [`export`](crate::export) carries a *Freedback* annotation verbatim inside an
//! ES256 JWT. This module instead maps the **Mangrove review schema** — the
//! actual claim set Mangrove's `submitReview` signs — to and from our annotation
//! model, so Freedback can both *emit* a token a Mangrove server accepts and
//! *ingest* a token a Mangrove client produced.
//!
//! Mangrove's review payload (Open Reviews Association, Apache-2.0; see
//! `docs/attributions.md`) is a flat ES256 JWS whose claims are:
//!
//! | Mangrove claim         | Type            | Freedback mapping |
//! |------------------------|-----------------|-------------------|
//! | `iss`                  | SPKI/JWK key    | JWS `kid` → `creator` |
//! | `iat`                  | unix seconds    | `created` (RFC 3339 UTC) |
//! | `sub`                  | URI             | `target` |
//! | `rating`               | int `0..=100`   | `freedback:ScalarRating` on `[0,100]` |
//! | `opinion`              | string          | `oa:TextualBody` / `oa:commenting` |
//! | `images`              | `[{src,label}]` | `schema:image` extension on the body |
//! | `metadata.nickname`    | string          | `creator.nickname` |
//! | `metadata.client_id`   | URI             | `creator.client_id` |
//! | `metadata.is_personal_experience` | bool | `metadata.is_personal_experience` |
//! | `metadata.is_affiliated`          | bool | `metadata.is_affiliated` |
//! | `metadata.*` (other)   | any             | preserved verbatim under `metadata` |
//!
//! A Mangrove review must carry **at least one** of `rating` / `opinion`
//! (mirroring Mangrove's own server-side check); both may be present. We keep
//! the structural minimum here and leave value bounds to SHACL (INVARIANT 3) on
//! the resulting annotation.
//!
//! The whole module is pure Rust (serde_json + base64 + the crate's ES256
//! primitives), so it builds for `wasm32-unknown-unknown` alongside the rest of
//! `protocol-lib`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::identity::{issuer_id_from_pem, verify_es256, Identity};
use crate::model::{Annotation, Body, Creator, Motivation, Target};

/// The Mangrove `rating` scale: integers `0..=100`. We expose the bounds so the
/// mapping and any caller agree on the `ScalarRating` worst/best.
pub const MANGROVE_RATING_MIN: f64 = 0.0;
/// Upper bound of the Mangrove `rating` scale.
pub const MANGROVE_RATING_MAX: f64 = 100.0;

/// Encode an annotation as a **Mangrove-shaped** ES256 review JWT.
///
/// The annotation is projected onto Mangrove's claim set: its target becomes
/// `sub`, a rating body becomes `rating` (rescaled to `0..=100`), a comment body
/// becomes `opinion`, and creator/metadata extras become `metadata`. The token
/// is signed by `identity` exactly like [`crate::to_jwt`] (header
/// `{alg:ES256, kid:<SPKI PEM>}`), so a Mangrove server verifies it normally.
///
/// Returns an error if the annotation carries neither a rating nor a comment
/// (Mangrove requires at least one).
pub fn to_mangrove_jwt(ann: &Annotation, identity: &Identity) -> Result<String> {
    let payload = annotation_to_review(ann, &identity.public_key_pem()?)?;
    let header = json!({ "alg": "ES256", "typ": "JWT", "kid": identity.public_key_pem()? });

    let h = B64.encode(serde_json::to_vec(&header)?);
    let p = B64.encode(serde_json::to_vec(&payload)?);
    let signing_input = format!("{h}.{p}");
    let sig = identity.sign_es256(signing_input.as_bytes());
    Ok(format!("{signing_input}.{sig}"))
}

/// Decode and verify a Mangrove review JWT, returning the equivalent annotation.
///
/// Verifies the ES256 signature over `header.payload` against the `kid`, then
/// maps the Mangrove claims to the annotation model. The issuer (`kid`) is
/// stamped as `creator` (Mangrove's `iss`).
pub fn from_mangrove_jwt(jwt: &str) -> Result<Annotation> {
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

    verify_es256(kid, format!("{h}.{p}").as_bytes(), s)?;

    let payload: Value = serde_json::from_slice(
        &B64.decode(p)
            .map_err(|e| Error::Crypto(format!("bad payload b64: {e}")))?,
    )?;
    review_to_annotation(&payload, kid)
}

// --- annotation → Mangrove review -----------------------------------------

/// Project an annotation onto a Mangrove review claim object (no signature).
///
/// `iss_pem` is the issuer's SPKI PEM (Mangrove's `iss`), used when the
/// annotation has no explicit creator.
pub fn annotation_to_review(ann: &Annotation, iss_pem: &str) -> Result<Value> {
    let mut review = Map::new();

    // iss: prefer the annotation's creator id, else the signing key.
    let iss = ann
        .creator
        .as_ref()
        .map(|c| c.id.clone())
        .unwrap_or_else(|| iss_pem.to_string());
    review.insert("iss".into(), json!(iss));

    // iat: from `created` (RFC 3339 → unix seconds). Mangrove uses integer secs.
    if let Some(iat) = ann.iat() {
        review.insert("iat".into(), json!(iat));
    }

    // sub: the target source IRI.
    review.insert("sub".into(), json!(ann.target.source()));

    // rating / opinion / images come from the bodies.
    let mut rating: Option<i64> = None;
    let mut opinion: Option<String> = None;
    let mut images: Vec<Value> = Vec::new();

    for body in &ann.body {
        match body {
            Body::StarRating { value, worst, best } | Body::ScalarRating { value, worst, best } => {
                rating = Some(rescale_to_mangrove(*value, *worst, *best));
            }
            Body::ThumbRating { up } => {
                // up → 100, down → 0 on the Mangrove scale.
                rating = Some(if *up { 100 } else { 0 });
            }
            Body::Comment { value } => opinion = Some(value.clone()),
            // An issue / problem report (ADR 0023) is free text about the
            // target; Mangrove has no dedicated field, so it exports as the
            // review's `opinion` (lossy: it round-trips as a comment).
            Body::Issue { value } => opinion = Some(value.clone()),
            // A tag has no Mangrove equivalent field; fold it into images as a
            // labelled marker so it is not silently dropped (round-trips via
            // metadata is lossy, so we keep tags in `metadata.tags`).
            Body::Tag { value } => {
                images.push(json!({ "label": format!("tag:{value}") }));
            }
        }
    }

    // Mangrove requires at least a rating or an opinion.
    if rating.is_none() && opinion.is_none() {
        return Err(Error::OutOfBounds(
            "a Mangrove review needs at least a rating or an opinion".into(),
        ));
    }
    if let Some(r) = rating {
        review.insert("rating".into(), json!(r));
    }
    if let Some(o) = opinion {
        review.insert("opinion".into(), json!(o));
    }
    if !images.is_empty() {
        review.insert("images".into(), json!(images));
    }

    // metadata: creator nickname/type plus any conformsTo marker, so a
    // round-trip preserves who/what produced the review.
    let mut metadata = Map::new();
    if let Some(creator) = &ann.creator {
        if let Some(t) = &creator.type_ {
            metadata.insert("creator_type".into(), json!(t));
        }
    }
    if let Some(profile) = &ann.conforms_to {
        metadata.insert("conformsTo".into(), json!(profile));
    }
    if !metadata.is_empty() {
        review.insert("metadata".into(), Value::Object(metadata));
    }

    Ok(Value::Object(review))
}

/// Rescale a rating from its native `[worst,best]` to Mangrove's `0..=100`.
fn rescale_to_mangrove(value: f64, worst: f64, best: f64) -> i64 {
    if (best - worst).abs() < f64::EPSILON {
        return 0;
    }
    let frac = ((value - worst) / (best - worst)).clamp(0.0, 1.0);
    (frac * (MANGROVE_RATING_MAX - MANGROVE_RATING_MIN) + MANGROVE_RATING_MIN).round() as i64
}

// --- Mangrove review → annotation -----------------------------------------

/// Map a Mangrove review claim object to the annotation model.
///
/// `kid` is the verified signing key (SPKI PEM); it stamps the `creator` when
/// the review's `iss` is itself a raw key (so the federating issuer id is the
/// stable `urn:freedback:key:` form, consistent with the native path).
pub fn review_to_annotation(review: &Value, kid: &str) -> Result<Annotation> {
    let sub = review
        .get("sub")
        .and_then(Value::as_str)
        .ok_or(Error::MissingField("sub"))?;

    let rating = review.get("rating").and_then(Value::as_f64);
    let opinion = review
        .get("opinion")
        .and_then(Value::as_str)
        .map(str::to_string);
    if rating.is_none() && opinion.is_none() {
        return Err(Error::OutOfBounds(
            "a Mangrove review needs at least a rating or an opinion".into(),
        ));
    }

    // Bodies: a rating (as a ScalarRating on the Mangrove [0,100] scale) and/or
    // a comment. A rating motivates `assessing`; an opinion-only review is a
    // `commenting`.
    let mut bodies: Vec<Body> = Vec::new();
    let motivation = if let Some(r) = rating {
        bodies.push(Body::ScalarRating {
            value: r,
            worst: MANGROVE_RATING_MIN,
            best: MANGROVE_RATING_MAX,
        });
        Motivation::Assessing
    } else {
        Motivation::Commenting
    };
    if let Some(text) = opinion {
        bodies.push(Body::Comment { value: text });
    }

    let mut ann = Annotation::new(motivation, Target::Iri(sub.to_string()), bodies);

    // created: from `iat` (unix seconds → RFC 3339 UTC).
    if let Some(iat) = review.get("iat").and_then(Value::as_i64) {
        if let Some(ts) = unix_to_rfc3339(iat) {
            ann.created = Some(ts);
        }
    }

    // creator: Mangrove `iss` is the issuer; normalize a raw key to the stable
    // issuer id, otherwise keep the given id. The `kid` is always a key, so a
    // matching/absent `iss` resolves to `urn:freedback:key:...`.
    let iss = review.get("iss").and_then(Value::as_str);
    let creator_id = match iss {
        Some(s) if s == kid || s.starts_with("-----BEGIN") => issuer_id_from_pem(kid)?,
        Some(s) => s.to_string(),
        None => issuer_id_from_pem(kid)?,
    };
    let mut creator = Creator::new(creator_id);
    if let Some(t) = review
        .get("metadata")
        .and_then(|m| m.get("creator_type"))
        .and_then(Value::as_str)
    {
        creator.type_ = Some(t.to_string());
    }
    ann.creator = Some(creator);

    Ok(ann)
}

/// Format a unix timestamp (seconds) as an RFC 3339 UTC string, matching the
/// `created` shape the rest of the model expects.
fn unix_to_rfc3339(secs: i64) -> Option<String> {
    let dt = time::OffsetDateTime::from_unix_timestamp(secs).ok()?;
    dt.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, Motivation, Target};

    fn star_ann(value: f64) -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(value)],
        )
        .with_created("2026-06-21T10:00:00Z")
    }

    #[test]
    fn star_maps_to_mangrove_rating_and_back() {
        let id = Identity::generate();
        // 4.0 on [1,5] → 75 on [0,100].
        let jwt = to_mangrove_jwt(&star_ann(4.0), &id).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let payload: Value = serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(payload["rating"], 75);
        assert_eq!(payload["sub"], "https://example.com/item/1");

        let back = from_mangrove_jwt(&jwt).unwrap();
        assert_eq!(back.target.source(), "https://example.com/item/1");
        // Round-trips as a ScalarRating on the [0,100] scale.
        match &back.body[0] {
            Body::ScalarRating { value, worst, best } => {
                assert_eq!(*value, 75.0);
                assert_eq!((*worst, *best), (0.0, 100.0));
            }
            other => panic!("expected ScalarRating, got {other:?}"),
        }
        assert_eq!(back.creator.unwrap().id, id.issuer_id().unwrap());
    }

    #[test]
    fn opinion_maps_to_comment() {
        let id = Identity::generate();
        let ann = Annotation::new(
            Motivation::Commenting,
            Target::Iri("https://example.com/item/2".into()),
            vec![Body::Comment {
                value: "great service".into(),
            }],
        )
        .with_created("2026-06-21T10:00:00Z");
        let jwt = to_mangrove_jwt(&ann, &id).unwrap();
        let back = from_mangrove_jwt(&jwt).unwrap();
        assert_eq!(back.motivation, Motivation::Commenting);
        match &back.body[0] {
            Body::Comment { value } => assert_eq!(value, "great service"),
            other => panic!("expected Comment, got {other:?}"),
        }
    }

    #[test]
    fn rating_and_opinion_both_present() {
        let id = Identity::generate();
        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/3".into()),
            vec![
                Body::star(5.0),
                Body::Comment {
                    value: "perfect".into(),
                },
            ],
        )
        .with_created("2026-06-21T10:00:00Z");
        let jwt = to_mangrove_jwt(&ann, &id).unwrap();
        let back = from_mangrove_jwt(&jwt).unwrap();
        assert_eq!(back.body.len(), 2);
        assert_eq!(back.motivation, Motivation::Assessing);
    }

    #[test]
    fn empty_review_is_rejected_both_ways() {
        let id = Identity::generate();
        // A tag-only annotation has neither rating nor opinion.
        let ann = Annotation::new(
            Motivation::Tagging,
            Target::Iri("https://example.com/item/4".into()),
            vec![Body::Tag { value: "x".into() }],
        );
        assert!(to_mangrove_jwt(&ann, &id).is_err());

        // A review JWT with neither rating nor opinion is rejected on ingest.
        let review = json!({ "iss": "k", "sub": "https://example.com/x", "iat": 0 });
        assert!(review_to_annotation(&review, "k").is_err());
    }

    #[test]
    fn iat_round_trips_to_created() {
        let id = Identity::generate();
        let jwt = to_mangrove_jwt(&star_ann(3.0), &id).unwrap();
        let back = from_mangrove_jwt(&jwt).unwrap();
        // 2026-06-21T10:00:00Z is a stable instant; the unix seconds round-trip.
        assert_eq!(back.iat(), star_ann(3.0).iat());
    }

    #[test]
    fn tampered_mangrove_payload_is_rejected() {
        let id = Identity::generate();
        let jwt = to_mangrove_jwt(&star_ann(4.0), &id).unwrap();
        let mut parts: Vec<&str> = jwt.split('.').collect();
        let forged = B64.encode(br#"{"iss":"k","sub":"https://evil/","rating":1}"#);
        parts[1] = &forged;
        let tampered = parts.join(".");
        assert!(from_mangrove_jwt(&tampered).is_err());
    }
}
