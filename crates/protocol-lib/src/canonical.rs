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

/// The RFC 8785 (JCS) canonical bytes of an **arbitrary** JSON value.
///
/// The generic counterpart to [`canonical_bytes`], for signing other Freedback
/// records over the same canonicalization (e.g. the discovery relay list).
pub fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>> {
    serde_json_canonicalizer::to_vec(value).map_err(|e| Error::Canonicalization(e.to_string()))
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

    /// The exact RFC 8785 canonical bytes of [`sample`] BEFORE the optional
    /// `rights` field existed (data licensing, ADR 0022). `rights` is
    /// `skip_serializing_if = Option::is_none`, so an annotation without a
    /// license must keep producing these exact bytes — i.e. every existing
    /// fixture, signature, and dedup id remains valid.
    const SAMPLE_CANONICAL_WITHOUT_RIGHTS: &str = concat!(
        r#"{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.net/ns/context.jsonld"],"#,
        r#""body":[{"schema:bestRating":5,"schema:ratingValue":4,"schema:worstRating":1,"#,
        r#""type":["freedback:StarRating","schema:Rating"]}],"#,
        r#""conformsTo":"https://freedback.net/profile/1","created":"2026-06-21T10:00:00Z","#,
        r#""motivation":"assessing","target":"https://example.com/item/1","type":"Annotation"}"#
    );

    #[test]
    fn absent_rights_keeps_pre_licensing_canonical_bytes() {
        let bytes = canonical_bytes(&sample()).unwrap();
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            SAMPLE_CANONICAL_WITHOUT_RIGHTS,
            "an annotation without `rights` must canonicalize byte-identically \
             to the pre-ADR-0022 form (existing dedup ids / signatures stay valid)"
        );
    }

    #[test]
    fn present_rights_is_content_and_changes_the_dedup_id() {
        let plain = sample();
        let licensed = sample().with_rights("https://creativecommons.org/licenses/by/4.0/");
        let bytes = canonical_bytes(&licensed).unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains(r#""rights":"https://creativecommons.org/licenses/by/4.0/""#),
            "rights participates in the canonical bytes"
        );
        assert_ne!(
            dedup_id(&plain).unwrap(),
            dedup_id(&licensed).unwrap(),
            "the same feedback under an explicit license is a different statement"
        );
        // And two different licenses are two different statements.
        let other = sample().with_rights("https://creativecommons.org/publicdomain/zero/1.0/");
        assert_ne!(dedup_id(&licensed).unwrap(), dedup_id(&other).unwrap());
    }

    /// Cross-language pin (ADR 0013 + ADR 0022): this exact string is also
    /// asserted by the widgets' JS canonicalizer over the same licensed content
    /// (`widgets/test.cjs`, `EXPECTED_CANONICAL_LICENSED`). If the two diverge,
    /// a license set in the browser would break signature verification here.
    #[test]
    fn licensed_widget_content_canonical_bytes() {
        let ann = sample()
            .with_creator(crate::model::Creator::new("urn:freedback:key:abc"))
            .with_rights("https://creativecommons.org/licenses/by/4.0/");
        let expected = concat!(
            r#"{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.net/ns/context.jsonld"],"#,
            r#""body":[{"schema:bestRating":5,"schema:ratingValue":4,"schema:worstRating":1,"#,
            r#""type":["freedback:StarRating","schema:Rating"]}],"#,
            r#""conformsTo":"https://freedback.net/profile/1","created":"2026-06-21T10:00:00Z","#,
            r#""creator":{"id":"urn:freedback:key:abc"},"motivation":"assessing","#,
            r#""rights":"https://creativecommons.org/licenses/by/4.0/","#,
            r#""target":"https://example.com/item/1","type":"Annotation"}"#
        );
        let bytes = canonical_bytes(&ann).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
    }

    #[test]
    fn rights_round_trips_through_serde() {
        let licensed = sample().with_rights("https://creativecommons.org/licenses/by/4.0/");
        let json = serde_json::to_value(&licensed).unwrap();
        assert_eq!(
            json["rights"],
            serde_json::json!("https://creativecommons.org/licenses/by/4.0/")
        );
        let back: Annotation = serde_json::from_value(json).unwrap();
        assert_eq!(back, licensed);
        // …and absence stays absent (no `"rights": null` on the wire).
        let json = serde_json::to_value(sample()).unwrap();
        assert!(json.get("rights").is_none());
    }

    /// Cross-language pin for the ISSUE feedback type (ADR 0023): this exact
    /// string is also asserted by the widgets' JS canonicalizer over the same
    /// content (`widgets/test.cjs`, `EXPECTED_CANONICAL_ISSUE`). Fixed creator
    /// and timestamp so the bytes — and the derived dedup id — are stable
    /// across runs and languages.
    #[test]
    fn issue_canonical_bytes_and_dedup_id_are_pinned() {
        let ann = Annotation::new(
            Motivation::Editing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::issue("the checkout button does nothing")],
        )
        .with_created("2026-06-21T10:00:00Z")
        .with_creator(crate::model::Creator::new("urn:freedback:key:abc"));
        let expected = concat!(
            r#"{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.net/ns/context.jsonld"],"#,
            r#""body":[{"format":"text/plain","purpose":"editing","type":"TextualBody","#,
            r#""value":"the checkout button does nothing"}],"#,
            r#""conformsTo":"https://freedback.net/profile/1","created":"2026-06-21T10:00:00Z","#,
            r#""creator":{"id":"urn:freedback:key:abc"},"motivation":"editing","#,
            r#""target":"https://example.com/item/1","type":"Annotation"}"#
        );
        let bytes = canonical_bytes(&ann).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
        assert_eq!(
            dedup_id(&ann).unwrap(),
            "dba0077f4239ac204481d458fc284f3d039e1c4f30e7c8cba7aaeb74b8696539",
            "the issue dedup id is content-derived and stable"
        );
    }

    /// A signature over an issue annotation verifies, and the same fixed key +
    /// timestamp always signs the same canonical bytes (deterministic ES256,
    /// RFC 6979) — so signatures and dedup ids are stable across runs.
    #[test]
    fn issue_signs_and_verifies_deterministically() {
        let identity = crate::identity::Identity::generate();
        let build = |id: &crate::identity::Identity| {
            let mut ann = Annotation::new(
                Motivation::Editing,
                Target::Iri("https://example.com/item/1".into()),
                vec![Body::issue("the checkout button does nothing")],
            )
            .with_created("2026-06-21T10:00:00Z")
            .with_creator(crate::model::Creator::new(id.issuer_id().unwrap()));
            id.sign_annotation(&mut ann).unwrap();
            ann
        };
        let a = build(&identity);
        let b = build(&identity);
        crate::identity::verify_annotation(&a).expect("issue signature verifies");
        assert_eq!(
            a.signature, b.signature,
            "same key + same content => same signature"
        );
        assert_eq!(dedup_id(&a).unwrap(), dedup_id(&b).unwrap());
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
