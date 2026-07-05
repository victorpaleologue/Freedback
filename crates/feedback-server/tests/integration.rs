//! In-process integration tests for the feedback server (the `TestCluster`
//! pattern, single node). Drives the real router via `tower::oneshot`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use freedback_feedback_server::{build_app, AppState};
use freedback_protocol::{Annotation, Body as FbBody, Identity, Motivation, Target};
use freedback_storage::MemoryStore;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

const BASE: &str = "http://test.local";

fn app() -> Router {
    let store = Arc::new(MemoryStore::new());
    build_app(AppState::new(store, BASE))
}

fn app_with_oauth(token: &str, app_id: &str, user: &str) -> Router {
    let store = Arc::new(MemoryStore::new());
    let mut tokens = HashMap::new();
    tokens.insert(token.to_string(), (app_id.to_string(), user.to_string()));
    build_app(AppState::new(store, BASE).with_oauth(tokens))
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&v).unwrap()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, json)
}

fn signed_star(value: f64) -> (Identity, Value) {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri("https://example.com/item/1".into()),
        vec![FbBody::star(value)],
    )
    .with_created("2026-06-21T10:00:00Z");
    id.sign_annotation(&mut ann).unwrap();
    (id, serde_json::to_value(ann).unwrap())
}

#[tokio::test]
async fn post_signed_then_read_back() {
    let app = app();
    let (_id, ann) = signed_star(4.0);

    let (status, headers, body) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.contains_key("location"), "Location header set");
    let posted_id = body["id"].as_str().unwrap().to_string();
    assert!(posted_id.starts_with(BASE));

    // GET single by dedup id (the last path segment).
    let dedup = posted_id.rsplit('/').next().unwrap();
    let (status, _h, one) = send(&app, "GET", &format!("/annotations/{dedup}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["id"], posted_id);

    // Collection read returns it with paging headers.
    let (status, headers, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["type"], "AnnotationPage");
    assert_eq!(page["partOf"]["total"], 1);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let link = headers.get("link").unwrap().to_str().unwrap();
    assert!(link.contains("rel=\"canonical\""));
    assert!(link.contains("ldp#Page"));
    assert!(headers.contains_key("etag"));

    // Sync returns it.
    let (status, _h, items) = send(
        &app,
        "GET",
        "/sync?target=https://example.com/item/1&gt_iat=0",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_body_is_422_with_report() {
    let app = app();
    let (_id, ann) = signed_star(7.0); // out of [1,5]
    let (status, _h, body) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["report"]["conforms"], false);
    assert!(!body["report"]["violations"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let app = app();
    let (_id, mut ann) = signed_star(4.0);
    // Tamper after signing: change the rating value in the JSON.
    ann["body"][0]["schema:ratingValue"] = serde_json::json!(2.0);
    let (status, _h, _b) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unsigned_without_token_is_rejected() {
    let app = app();
    let ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri("https://example.com/item/1".into()),
        vec![FbBody::star(4.0)],
    )
    .with_created("2026-06-21T10:00:00Z");
    let (status, _h, _b) = send(
        &app,
        "POST",
        "/annotations/",
        None,
        Some(serde_json::to_value(ann).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oauth_path_stamps_creator() {
    let app = app_with_oauth("secret-token", "app-1", "user-9");
    let ann = Annotation::new(
        Motivation::Commenting,
        Target::Iri("https://example.com/item/2".into()),
        vec![FbBody::Comment {
            value: "nice".into(),
        }],
    )
    .with_created("2026-06-21T10:00:00Z");

    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        Some("secret-token"),
        Some(serde_json::to_value(ann).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["creator"]["id"], "urn:freedback:oauth:app-1:user-9");

    // Wrong token is rejected.
    let ann2 = serde_json::json!({
        "@context": "x", "type": "Annotation", "motivation": "commenting",
        "target": "https://example.com/item/2",
        "body": [{ "type": "TextualBody", "value": "x", "purpose": "commenting" }]
    });
    let (status, _h, _b) = send(&app, "POST", "/annotations/", Some("wrong"), Some(ann2)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn issue_report_ingests_and_round_trips() {
    // The issue / problem-report feedback type (ADR 0023): an oa:TextualBody
    // under the standard oa:editing motivation. Both the exact serde shape
    // (signed path) and an aliased JSON-LD form (bearer path) must ingest,
    // validate, and read back.
    let app = app_with_oauth("tok", "app", "u");

    // 1) Signed, canonical serde shape.
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Editing,
        Target::Iri("https://example.com/item/7".into()),
        vec![FbBody::issue("the checkout button does nothing")],
    )
    .with_created("2026-06-21T10:00:00Z");
    ann.creator = Some(freedback_protocol::Creator::new(id.issuer_id().unwrap()));
    id.sign_annotation(&mut ann).unwrap();
    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        None,
        Some(serde_json::to_value(&ann).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "signed issue accepted: {body}");
    assert_eq!(body["motivation"], "editing");
    assert_eq!(body["body"][0]["type"], "TextualBody");
    assert_eq!(body["body"][0]["purpose"], "editing");

    // 2) Aliased JSON-LD form (prefixed motivation/purpose, single body object)
    // over the bearer path normalizes to the same wire shape.
    let variant = serde_json::json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "type": "Annotation",
        "motivation": "oa:editing",
        "created": "2026-06-21T10:00:01Z",
        "target": "https://example.com/item/7",
        "body": { "type": "oa:TextualBody", "value": "images 404", "purpose": "oa:editing" }
    });
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some("tok"), Some(variant)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "aliased issue accepted: {body}"
    );
    assert_eq!(body["motivation"], "editing");
    assert_eq!(body["body"][0]["value"], "images 404");
    assert_eq!(body["body"][0]["purpose"], "editing");

    // 3) Both read back under the target.
    let (status, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/7",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["partOf"]["total"], 2);

    // 4) An empty issue text is rejected by SHACL (422), like empty comments.
    let empty = Annotation::new(
        Motivation::Editing,
        Target::Iri("https://example.com/item/7".into()),
        vec![FbBody::issue("")],
    )
    .with_created("2026-06-21T10:00:02Z");
    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        Some("tok"),
        Some(serde_json::to_value(&empty).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

#[tokio::test]
async fn accepts_varied_jsonld_serialization() {
    // JSON-LD is primary: a conformant-but-differently-serialized annotation
    // (single body object, target as an object, prefixed property names,
    // @context as a string, "oa:" motivation) must be accepted and normalized.
    let app = app_with_oauth("tok", "app", "u");
    let variant = serde_json::json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "type": "Annotation",
        "motivation": "oa:assessing",
        "created": "2026-06-21T10:00:00Z",
        "target": { "id": "https://example.com/item/9" },
        "body": {
            "type": ["freedback:StarRating", "schema:Rating"],
            "schema:ratingValue": 5,
            "schema:worstRating": 1,
            "schema:bestRating": 5
        }
    });
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some("tok"), Some(variant)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "varied serialization must be accepted"
    );
    // It was normalized to the canonical model (array body, stamped creator).
    assert!(body["body"].is_array());
    assert_eq!(body["body"][0]["schema:ratingValue"], 5.0);

    // And it is queryable under its target.
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/9",
        None,
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 1);
}

#[tokio::test]
async fn accepts_foreign_context_via_full_compaction() {
    // A third party names the same feedback with its OWN terms, bound to the
    // canonical IRIs by an inline @context the alias normalizer cannot read.
    // The server falls back to full JSON-LD compaction (ADR 0011) and accepts
    // it, normalizing to the canonical model.
    let app = app_with_oauth("tok", "app", "u");
    let foreign = serde_json::json!({
        "@context": {
            "Rating": "http://www.w3.org/ns/oa#Annotation",
            "about":  { "@id": "http://www.w3.org/ns/oa#hasTarget", "@type": "@id" },
            "why":    { "@id": "http://www.w3.org/ns/oa#motivatedBy", "@type": "@id" },
            "on":     { "@id": "http://purl.org/dc/terms/created", "@type": "http://www.w3.org/2001/XMLSchema#dateTime" },
            "scores": { "@id": "http://www.w3.org/ns/oa#hasBody", "@type": "@id" },
            "Stars":  "https://freedback.net/ns#StarRating",
            "stars":  { "@id": "http://schema.org/ratingValue", "@type": "http://www.w3.org/2001/XMLSchema#double" },
            "low":    { "@id": "http://schema.org/worstRating", "@type": "http://www.w3.org/2001/XMLSchema#double" },
            "high":   { "@id": "http://schema.org/bestRating", "@type": "http://www.w3.org/2001/XMLSchema#double" },
            "assessing": "http://www.w3.org/ns/oa#assessing"
        },
        "@type": "Rating",
        "why": "assessing",
        "on": "2026-06-21T10:00:00Z",
        "about": "https://example.com/item/foreign",
        "scores": { "@type": "Stars", "stars": 4, "low": 1, "high": 5 }
    });
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some("tok"), Some(foreign)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a third-party @context must be accepted via full compaction"
    );
    assert_eq!(body["body"][0]["schema:ratingValue"], 4.0);

    // Queryable under the normalized target.
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/foreign",
        None,
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 1);
}

#[tokio::test]
async fn submit_jwt_export_profile() {
    use freedback_protocol::to_jwt;
    let app = app();
    // No bearer / no detached signature — the JWT itself is the issuer proof.
    let id = Identity::generate();
    let ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri("https://example.com/item/7".into()),
        vec![FbBody::star(4.0)],
    )
    .with_created("2026-06-21T10:00:00Z");
    let jwt = to_jwt(&ann, &id).unwrap();

    let (status, headers, body) = send(&app, "PUT", &format!("/submit/{jwt}"), None, None).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid JWT submit must be accepted"
    );
    assert!(headers.contains_key("location"));
    assert_eq!(body["creator"]["id"], id.issuer_id().unwrap());

    // Readable back.
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/7",
        None,
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 1);

    // A garbage JWT is rejected.
    let (status, _h, _b) = send(&app, "PUT", "/submit/not.a.jwt", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn repost_is_idempotent() {
    let app = app();
    let (_id, ann) = signed_star(5.0);
    for _ in 0..3 {
        let (status, _h, _b) = send(&app, "POST", "/annotations/", None, Some(ann.clone())).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    assert_eq!(
        page["partOf"]["total"], 1,
        "re-POST must be idempotent by dedup id"
    );
}

#[tokio::test]
async fn accepts_browser_signed_annotation_end_to_end() {
    // The same fixture the widget produces (WebCrypto ES256 over the JCS bytes,
    // ADR 0013): no bearer token — the self-signature is the authorization. This
    // exercises the full path: from_jsonld → verify_annotation → SHACL → store.
    let app = app();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../protocol-lib/tests/fixtures/widget-signed.json"
    ))
    .unwrap();

    let (status, headers, body) = send(&app, "POST", "/annotations/", None, Some(fixture)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a browser self-signed annotation must be accepted without a token"
    );
    assert!(headers.contains_key("location"));
    assert!(body["creator"]["id"]
        .as_str()
        .unwrap()
        .starts_with("urn:freedback:key:"));

    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/widget",
        None,
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 1);
}

#[tokio::test]
async fn collection_emits_freshness_and_validator_headers() {
    let app = app();
    let (_id, ann) = signed_star(4.0);
    let _ = send(&app, "POST", "/annotations/", None, Some(ann)).await;

    let (status, headers, _body) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cc.contains("max-age="), "Cache-Control present: {cc}");
    let lm = headers.get("last-modified").unwrap().to_str().unwrap();
    assert!(lm.ends_with(" GMT"), "Last-Modified is an HTTP-date: {lm}");
}

#[tokio::test]
async fn if_modified_since_earns_a_304() {
    let app = app();
    let (_id, ann) = signed_star(4.0);
    let _ = send(&app, "POST", "/annotations/", None, Some(ann)).await;

    // Read once to learn the page's Last-Modified.
    let (_s, headers, _b) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    let last_modified = headers.get("last-modified").unwrap().to_str().unwrap();

    // A conditional GET with that exact date (no ETag) → 304.
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/annotations/?target=https://example.com/item/1")
        .header("if-modified-since", last_modified)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_MODIFIED,
        "If-Modified-Since at the page mtime must 304"
    );

    // An older If-Modified-Since (the epoch) → full 200 body.
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/annotations/?target=https://example.com/item/1")
        .header("if-modified-since", "Thu, 01 Jan 1970 00:00:00 GMT")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a stale If-Modified-Since must return the fresh representation"
    );
}

#[tokio::test]
async fn well_known_advertises_capabilities() {
    let app = app();
    let (status, _h, doc) = send(&app, "GET", "/.well-known/freedback", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["protocol"], "freedback/1");
    let caps = doc["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "wap-container"));
    assert!(caps.iter().any(|c| c == "erasure"), "ADR 0021 capability");
    assert_eq!(doc["conformsTo"], "https://freedback.net/profile/1");
}

// --- data licensing (ADR 0022) ------------------------------------------------

const LICENSE: &str = "https://creativecommons.org/licenses/by/4.0/";

/// A signed star rating carrying an explicit `rights` license IRI.
fn signed_licensed_star(value: f64, license: &str) -> (Identity, Value) {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri("https://example.com/item/1".into()),
        vec![FbBody::star(value)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_rights(license);
    id.sign_annotation(&mut ann).unwrap();
    (id, serde_json::to_value(ann).unwrap())
}

#[tokio::test]
async fn rights_survives_post_and_read_back() {
    let app = app();
    let (_id, ann) = signed_licensed_star(4.0, LICENSE);

    let (status, _h, body) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["rights"], LICENSE, "POST echoes the license");
    let dedup = body["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    // Single read returns it with `rights` intact.
    let (status, _h, one) = send(&app, "GET", &format!("/annotations/{dedup}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["rights"], LICENSE);

    // Collection read too.
    let (status, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["rights"], LICENSE);
}

#[tokio::test]
async fn rights_survives_an_aliased_jsonld_serialization() {
    // The same annotation with `rights` spelled as the prefixed JSON-LD term
    // `dcterms:rights` must normalize (alias ingest, ADR 0007) to the same
    // model — so the self-signature still verifies and reads return `rights`.
    let app = app();
    let (_id, mut ann) = signed_licensed_star(5.0, LICENSE);
    let obj = ann.as_object_mut().unwrap();
    let rights = obj.remove("rights").unwrap();
    obj.insert("dcterms:rights".into(), rights);

    let (status, _h, body) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "aliased rights ingests: {body}"
    );
    let dedup = body["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    let (status, _h, one) = send(&app, "GET", &format!("/annotations/{dedup}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["rights"], LICENSE, "read back in canonical form");
}

#[tokio::test]
async fn non_iri_rights_is_rejected_by_shacl() {
    let app = app();
    let (_id, ann) = signed_licensed_star(4.0, "not an iri");

    let (status, _h, body) = send(&app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "SHACL must reject a non-IRI rights: {body}"
    );
    assert_eq!(body["report"]["conforms"], false);
    let violations = body["report"]["violations"].as_array().unwrap();
    assert!(
        violations
            .iter()
            .any(|v| v.as_str().unwrap_or_default().contains("license IRI")),
        "violation names the rights constraint: {violations:?}"
    );
}

#[tokio::test]
async fn well_known_advertises_the_default_license_when_configured() {
    // Configured → `"license"` is surfaced (annotations without `rights` are
    // distributed under it, ADR 0022).
    let store = Arc::new(MemoryStore::new());
    let licensed = build_app(AppState::new(store, BASE).with_default_license(LICENSE));
    let (status, _h, doc) = send(&licensed, "GET", "/.well-known/freedback", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["license"], LICENSE);

    // Unconfigured → the key is absent entirely (not null / empty).
    let (status, _h, doc) = send(&app(), "GET", "/.well-known/freedback", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        doc.get("license").is_none(),
        "no default license advertised"
    );
}

// --- right to erasure (ADR 0021) --------------------------------------------

/// Build a signed delete document for `dedup` with `id`'s key.
fn signed_delete(id: &Identity, dedup: &str) -> Value {
    let mut doc = freedback_protocol::DeleteRequest::new(dedup, "2026-07-05T12:00:00Z");
    id.sign_delete(&mut doc).unwrap();
    serde_json::to_value(doc).unwrap()
}

/// POST a signed annotation and return its dedup id (last path segment).
async fn publish(app: &Router, ann: Value) -> String {
    let (status, _h, body) = send(app, "POST", "/annotations/", None, Some(ann)).await;
    assert_eq!(status, StatusCode::CREATED);
    body["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn erasure_lifecycle_self_signed() {
    let app = app();
    let (id, ann) = signed_star(4.0);
    let dedup = publish(&app, ann.clone()).await;
    let item = format!("/annotations/{dedup}");

    // Present before the delete.
    let (status, _h, _b) = send(&app, "GET", &item, None, None).await;
    assert_eq!(status, StatusCode::OK);

    // DELETE with the author's key → 204 No Content.
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(signed_delete(&id, &dedup)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // GET now answers 410 Gone (tombstoned, not merely unknown).
    let (status, _h, _b) = send(&app, "GET", &item, None, None).await;
    assert_eq!(status, StatusCode::GONE);

    // The collection no longer returns it.
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/item/1",
        None,
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 0, "erased from the container read");

    // Re-POSTing the same content is 410: erased stays erased.
    let (status, _h, _b) = send(&app, "POST", "/annotations/", None, Some(ann.clone())).await;
    assert_eq!(status, StatusCode::GONE);

    // …including through the batch path (per-item 410).
    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        None,
        Some(Value::Array(vec![ann])),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(body["results"][0]["status"], 410);

    // The tombstone feed lists it (content-free: just id, time, proof).
    let (status, _h, tombs) = send(&app, "GET", "/tombstones?gt_deleted_at=0", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let tombs = tombs.as_array().unwrap();
    assert_eq!(tombs.len(), 1);
    assert_eq!(tombs[0]["dedup_id"], dedup.as_str());
    assert_eq!(tombs[0]["proof"]["type"], "Delete");
    assert!(tombs[0].get("body").is_none() && tombs[0].get("target").is_none());

    // Deleting again is idempotent → 204.
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(signed_delete(&id, &dedup)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_with_wrong_key_is_403_and_annotation_survives() {
    let app = app();
    let (_author, ann) = signed_star(4.0);
    let dedup = publish(&app, ann).await;
    let item = format!("/annotations/{dedup}");

    let stranger = Identity::generate();
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(signed_delete(&stranger, &dedup)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not the author's key");

    // The annotation survives.
    let (status, _h, _b) = send(&app, "GET", &item, None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn delete_unknown_id_is_404_and_unsigned_is_401() {
    let app = app();
    let ghost = "0".repeat(64);
    let doc = serde_json::json!({
        "type": "Delete", "annotation": ghost, "created": "2026-07-05T12:00:00Z",
    });
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &format!("/annotations/{ghost}"),
        None,
        Some(doc),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "never-seen id is a plain 404"
    );

    // An existing annotation + an unsigned document without a bearer → 401.
    let (_id, ann) = signed_star(4.0);
    let dedup = publish(&app, ann).await;
    let doc = serde_json::json!({
        "type": "Delete", "annotation": dedup, "created": "2026-07-05T12:00:00Z",
    });
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &format!("/annotations/{dedup}"),
        None,
        Some(doc),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_delete_documents_are_400() {
    let app = app();
    let (id, ann) = signed_star(4.0);
    let dedup = publish(&app, ann).await;
    let item = format!("/annotations/{dedup}");

    // Not a delete document at all.
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(serde_json::json!({"x": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Wrong type value.
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(serde_json::json!({
            "type": "Annotation", "annotation": dedup, "created": "2026-07-05T12:00:00Z",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The document's `annotation` must equal the path id.
    let other = "f".repeat(64);
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(signed_delete(&id, &other)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing was erased by any of those.
    let (status, _h, _b) = send(&app, "GET", &item, None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn oauth_delete_requires_the_same_identity() {
    // Two bearers → two distinct (app, user) identities on one server.
    let store = Arc::new(MemoryStore::new());
    let mut tokens = HashMap::new();
    tokens.insert("tok-alice".to_string(), ("app-1".into(), "alice".into()));
    tokens.insert("tok-bob".to_string(), ("app-1".into(), "bob".into()));
    let app = build_app(AppState::new(store, BASE).with_oauth(tokens));

    let ann = Annotation::new(
        Motivation::Commenting,
        Target::Iri("https://example.com/item/2".into()),
        vec![FbBody::Comment {
            value: "nice".into(),
        }],
    )
    .with_created("2026-06-21T10:00:00Z");
    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        Some("tok-alice"),
        Some(serde_json::to_value(&ann).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let dedup = body["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let item = format!("/annotations/{dedup}");
    let doc = serde_json::json!({
        "type": "Delete", "annotation": dedup, "created": "2026-07-05T12:00:00Z",
    });

    // A different OAuth user is not the creator → 403.
    let (status, _h, _b) = send(&app, "DELETE", &item, Some("tok-bob"), Some(doc.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A self-signed delete cannot erase an OAuth-authored annotation → 403.
    let stranger = Identity::generate();
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &item,
        None,
        Some(signed_delete(&stranger, &dedup)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The creator's own bearer erases it (no signature needed) → 204.
    let (status, _h, _b) = send(&app, "DELETE", &item, Some("tok-alice"), Some(doc.clone())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _h, _b) = send(&app, "GET", &item, None, None).await;
    assert_eq!(status, StatusCode::GONE);

    // And a bearer cannot erase a self-signed annotation (vice versa).
    let (_id, signed) = signed_star(3.0);
    let dedup2 = publish(&app, signed).await;
    let doc2 = serde_json::json!({
        "type": "Delete", "annotation": dedup2, "created": "2026-07-05T12:00:00Z",
    });
    let (status, _h, _b) = send(
        &app,
        "DELETE",
        &format!("/annotations/{dedup2}"),
        Some("tok-alice"),
        Some(doc2),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn item_options_and_cors_advertise_delete() {
    let app = app();
    let (_id, ann) = signed_star(4.0);
    let dedup = publish(&app, ann).await;

    let (status, headers, _b) = send(
        &app,
        "OPTIONS",
        &format!("/annotations/{dedup}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let allow = headers.get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("DELETE"), "item Allow gains DELETE: {allow}");
}
