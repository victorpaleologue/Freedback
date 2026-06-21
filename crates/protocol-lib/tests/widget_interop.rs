//! Cross-language interop: a W3C annotation **signed in the browser** by the
//! WebCrypto path of `widgets/freedback-widgets.js` must verify here.
//!
//! `tests/fixtures/widget-signed.json` is produced by the widget's
//! `buildSignedAnnotation` (ECDSA P-256, ES256 over the RFC 8785 JCS bytes) and
//! committed. This test normalizes it through the same ingest path the server
//! uses (`from_jsonld`) and checks the detached signature with
//! `verify_annotation` — proving the JS JCS + signing pipeline matches the Rust
//! canonicalizer/verifier byte-for-byte (ADR 0013).

use freedback_protocol::{from_jsonld, identity::issuer_id_from_pem, verify_annotation};

const FIXTURE: &str = include_str!("fixtures/widget-signed.json");

#[test]
fn browser_signed_annotation_verifies() {
    let doc: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");

    // Ingest exactly as the server would.
    let ann = from_jsonld(&doc).expect("fixture normalizes to the model");

    // The detached ES256 signature verifies over the reconstructed canonical
    // bytes — i.e. the browser hashed/signed exactly what Rust recomputes.
    verify_annotation(&ann).expect("browser-made signature must verify in Rust");
}

#[test]
fn browser_creator_matches_its_signing_key() {
    let doc: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let ann = from_jsonld(&doc).unwrap();

    // The widget derives `creator.id` as urn:freedback:key:<sha256(SPKI DER)>;
    // it must equal the id derived from the signature's public key (kid PEM),
    // so the self-signed provenance is internally consistent.
    let sig = ann.signature.as_ref().expect("signed");
    let from_key = issuer_id_from_pem(&sig.kid).expect("kid is an SPKI PEM");
    assert_eq!(
        ann.creator.as_ref().unwrap().id,
        from_key,
        "creator id must be derived from the signing key"
    );
}

#[test]
fn tampering_with_browser_annotation_is_rejected() {
    let mut doc: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    // Flip the rating after signing.
    doc["body"][0]["schema:ratingValue"] = serde_json::json!(0.1);
    let ann = from_jsonld(&doc).unwrap();
    assert!(
        verify_annotation(&ann).is_err(),
        "a modified body must fail signature verification"
    );
}
