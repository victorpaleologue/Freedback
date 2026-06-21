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
            "Stars":  "https://freedback.org/ns#StarRating",
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

    let (status, headers, body) =
        send(&app, "POST", "/annotations/", None, Some(fixture)).await;
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
    assert!(doc["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "wap-container"));
    assert_eq!(doc["conformsTo"], "https://freedback.org/profile/1");
}
