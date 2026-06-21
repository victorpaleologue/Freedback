//! W3C Web Annotation Protocol / LDP container conformance suite (issue #28,
//! part 1) and batch partial-failure semantics (part 2).
//!
//! These exercise the protocol surface *beyond* the happy path in
//! `integration.rs`: paging `Link` rels (`first`/`last`/`next`/`prev`),
//! content negotiation (`Accept` → 406, `Content-Type` of the page), the
//! `Allow` header / `OPTIONS` probe, `HEAD`, container edge cases (empty
//! collection, out-of-range page), and the multi-status batch response.
//!
//! Everything runs the real router in-process over the in-memory store with
//! deterministic fixed timestamps, per the testing rules.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use freedback_feedback_server::{build_app, AppState};
use freedback_protocol::{Annotation, Body as FbBody, Identity, Motivation, Target};
use freedback_storage::MemoryStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const BASE: &str = "http://test.local";
const TARGET: &str = "https://example.com/item/conf";

fn app_with_page_size(page_size: usize) -> Router {
    let store = Arc::new(MemoryStore::new());
    let mut state = AppState::new(store, BASE);
    state.page_size = page_size;
    build_app(state)
}

fn app() -> Router {
    app_with_page_size(50)
}

fn app_with_oauth(token: &str, app_id: &str, user: &str) -> Router {
    let store = Arc::new(MemoryStore::new());
    let mut tokens = HashMap::new();
    tokens.insert(token.to_string(), (app_id.to_string(), user.to_string()));
    build_app(AppState::new(store, BASE).with_oauth(tokens))
}

/// Send a request, optionally with extra headers, returning status/headers/body.
async fn send_with(
    app: &Router,
    method: &str,
    uri: &str,
    extra: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in extra {
        req = req.header(*k, *v);
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

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    send_with(app, method, uri, &[], body).await
}

/// A signed star annotation on `TARGET`. A unique `(value, created)` gives a
/// distinct dedup id, so several can coexist under the same target.
fn signed_star_at(value: f64, created: &str) -> Value {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(TARGET.into()),
        vec![FbBody::star(value)],
    )
    .with_created(created);
    id.sign_annotation(&mut ann).unwrap();
    serde_json::to_value(ann).unwrap()
}

/// Seed `n` distinct annotations under `TARGET` (deterministic timestamps).
async fn seed(app: &Router, n: usize) {
    for i in 0..n {
        let created = format!("2026-06-21T10:{:02}:00Z", i);
        let value = 1.0 + (i % 5) as f64; // 1..=5, valid
        let (status, _h, _b) = send(
            app,
            "POST",
            "/annotations/",
            Some(signed_star_at(value, &created)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "seed item {i}");
    }
}

fn link_header(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("link")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

// --- Part 1: container conformance ----------------------------------------

#[tokio::test]
async fn paging_emits_first_last_next_prev_rels() {
    let app = app_with_page_size(2);
    seed(&app, 5).await; // 5 items / 2 per page ⇒ pages 0,1,2 (last = 2)

    // First page (0): first, last, next; NO prev.
    let url = format!("/annotations/?target={TARGET}&page=0");
    let (status, headers, page) = send(&app, "GET", &url, None).await;
    assert_eq!(status, StatusCode::OK);
    let link = link_header(&headers);
    assert!(link.contains("rel=\"first\""), "first rel: {link}");
    assert!(link.contains("rel=\"last\""), "last rel: {link}");
    assert!(link.contains("rel=\"next\""), "next rel: {link}");
    assert!(!link.contains("rel=\"prev\""), "no prev on page 0: {link}");
    // Body mirrors the navigation in partOf + next. Targets are percent-encoded
    // in the minted URLs, so assert on the (encoded) page suffix, not the raw IRI.
    assert_eq!(page["partOf"]["total"], 5);
    assert!(page["partOf"]["first"]
        .as_str()
        .unwrap()
        .ends_with("&page=0"));
    assert!(page["partOf"]["last"]
        .as_str()
        .unwrap()
        .ends_with("&page=2"));
    assert!(page.get("next").is_some());
    assert!(page.get("prev").is_none());

    // Middle page (1): all four rels.
    let url = format!("/annotations/?target={TARGET}&page=1");
    let (_s, headers, _b) = send(&app, "GET", &url, None).await;
    let link = link_header(&headers);
    for rel in ["first", "last", "next", "prev"] {
        assert!(
            link.contains(&format!("rel=\"{rel}\"")),
            "{rel} on middle page: {link}"
        );
    }

    // Last page (2): first, last, prev; NO next.
    let url = format!("/annotations/?target={TARGET}&page=2");
    let (_s, headers, page) = send(&app, "GET", &url, None).await;
    let link = link_header(&headers);
    assert!(link.contains("rel=\"prev\""), "prev on last page: {link}");
    assert!(
        !link.contains("rel=\"next\""),
        "no next on last page: {link}"
    );
    assert!(page.get("next").is_none());
    assert!(page.get("prev").is_some());
}

#[tokio::test]
async fn single_page_collection_has_first_equal_last() {
    let app = app();
    seed(&app, 1).await;
    let url = format!("/annotations/?target={TARGET}");
    let (_s, headers, page) = send(&app, "GET", &url, None).await;
    let link = link_header(&headers);
    // first and last both point at page 0; no next/prev.
    assert!(link.contains("rel=\"first\""));
    assert!(link.contains("rel=\"last\""));
    assert!(!link.contains("rel=\"next\""));
    assert!(!link.contains("rel=\"prev\""));
    assert_eq!(page["partOf"]["first"], page["partOf"]["last"]);
}

#[tokio::test]
async fn empty_collection_is_a_valid_single_page() {
    let app = app();
    // No items under this target.
    let (status, headers, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/none",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["type"], "AnnotationPage");
    assert_eq!(page["partOf"]["total"], 0);
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    let link = link_header(&headers);
    // An empty collection still advertises first/last (== page 0) and no next/prev.
    assert!(link.contains("rel=\"first\""));
    assert!(link.contains("rel=\"last\""));
    assert!(!link.contains("rel=\"next\""));
    assert!(!link.contains("rel=\"prev\""));
}

#[tokio::test]
async fn out_of_range_page_returns_empty_items_no_next() {
    let app = app_with_page_size(2);
    seed(&app, 3).await; // pages 0,1 (last=1)
    let url = format!("/annotations/?target={TARGET}&page=9");
    let (status, headers, page) = send(&app, "GET", &url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    let link = link_header(&headers);
    assert!(
        !link.contains("rel=\"next\""),
        "no next past the end: {link}"
    );
    // first/last still describe the real bounds of the collection.
    assert!(link.contains("rel=\"first\""));
    assert!(link.contains("rel=\"last\""));
}

#[tokio::test]
async fn collection_advertises_jsonld_content_type() {
    let app = app();
    seed(&app, 1).await;
    let url = format!("/annotations/?target={TARGET}");
    let (status, headers, _b) = send(&app, "GET", &url, None).await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("application/ld+json"), "Content-Type: {ct}");
    assert!(ct.contains("profile="), "JSON-LD profile advertised: {ct}");
}

#[tokio::test]
async fn unacceptable_accept_is_406() {
    let app = app();
    seed(&app, 1).await;
    let url = format!("/annotations/?target={TARGET}");
    // A client that only accepts HTML cannot be served our JSON-LD.
    let (status, _h, _b) = send_with(&app, "GET", &url, &[("accept", "text/html")], None).await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);

    // But application/ld+json, application/json, and */* are all acceptable.
    for accept in [
        "application/ld+json",
        "application/json",
        "*/*",
        "application/*",
    ] {
        let (status, _h, _b) = send_with(&app, "GET", &url, &[("accept", accept)], None).await;
        assert_eq!(status, StatusCode::OK, "Accept {accept} must be OK");
    }
}

#[tokio::test]
async fn options_advertises_allow_and_accept_post() {
    let app = app();
    let (status, headers, _b) = send(&app, "OPTIONS", "/annotations/", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let allow = headers.get("allow").unwrap().to_str().unwrap();
    for m in ["GET", "HEAD", "POST", "OPTIONS"] {
        assert!(allow.contains(m), "Allow lists {m}: {allow}");
    }
    let accept_post = headers.get("accept-post").unwrap().to_str().unwrap();
    assert!(
        accept_post.contains("application/ld+json"),
        "Accept-Post: {accept_post}"
    );
}

#[tokio::test]
async fn get_response_carries_allow_header() {
    let app = app();
    seed(&app, 1).await;
    let url = format!("/annotations/?target={TARGET}");
    let (status, headers, _b) = send(&app, "GET", &url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("allow"), "GET advertises Allow");
}

#[tokio::test]
async fn head_returns_headers_without_a_body() {
    let app = app();
    seed(&app, 1).await;
    let url = format!("/annotations/?target={TARGET}");
    // axum derives HEAD from the GET handler and strips the body.
    let req = Request::builder()
        .method("HEAD")
        .uri(&url)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().contains_key("etag"),
        "HEAD carries the ETag validator"
    );
    assert!(
        resp.headers().contains_key("link"),
        "HEAD carries paging Link rels"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty(), "HEAD has no body");
}

// --- Part 2: batch partial-failure semantics ------------------------------

#[tokio::test]
async fn batch_with_one_invalid_item_reports_per_item_outcomes() {
    // persist-valid-items policy: a batch of [valid, invalid, valid] persists
    // the two valid items and reports the invalid one, all as 207 Multi-Status.
    let app = app();
    let good1 = signed_star_at(4.0, "2026-06-21T10:00:00Z");
    let bad = signed_star_at(7.0, "2026-06-21T10:01:00Z"); // 7 out of [1,5] ⇒ SHACL reject
    let good2 = signed_star_at(2.0, "2026-06-21T10:02:00Z");

    let batch = json!([good1, bad, good2]);
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some(batch)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "a batch is 207");
    assert_eq!(body["type"], "BatchResult");
    assert_eq!(body["total"], 3);
    assert_eq!(body["succeeded"], 2);
    assert_eq!(body["failed"], 1);

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    // Order preserved; the middle item failed with a SHACL report.
    assert_eq!(results[0]["status"], 201);
    assert_eq!(results[0]["index"], 0);
    assert!(results[0]["id"].as_str().unwrap().starts_with(BASE));
    assert_eq!(results[1]["status"], 422);
    assert_eq!(results[1]["index"], 1);
    assert_eq!(results[1]["report"]["conforms"], false);
    assert_eq!(results[2]["status"], 201);

    // The two valid items are actually persisted (the invalid one is not).
    let url = format!("/annotations/?target={TARGET}");
    let (_s, _h, page) = send(&app, "GET", &url, None).await;
    assert_eq!(
        page["partOf"]["total"], 2,
        "valid items persisted, invalid skipped"
    );
}

#[tokio::test]
async fn all_valid_batch_is_207_all_success() {
    let app = app();
    let batch = json!([
        signed_star_at(4.0, "2026-06-21T10:00:00Z"),
        signed_star_at(5.0, "2026-06-21T10:01:00Z"),
    ]);
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some(batch)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(body["succeeded"], 2);
    assert_eq!(body["failed"], 0);
    assert!(body["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["status"] == 201));
}

#[tokio::test]
async fn batch_unsigned_item_fails_only_itself() {
    // One item is unsigned; the others are self-signed. The unsigned item is
    // reported 401 but does not block the signed ones (per-item authorization).
    let app = app();
    let signed = signed_star_at(4.0, "2026-06-21T10:00:00Z");
    let unsigned = {
        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri(TARGET.into()),
            vec![FbBody::star(3.0)],
        )
        .with_created("2026-06-21T10:01:00Z");
        serde_json::to_value(ann).unwrap()
    };
    let batch = json!([signed, unsigned]);
    let (status, _h, body) = send(&app, "POST", "/annotations/", Some(batch)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(body["results"][0]["status"], 201);
    assert_eq!(body["results"][1]["status"], 401);
    assert_eq!(body["succeeded"], 1);
}

#[tokio::test]
async fn batch_under_oauth_authorizes_whole_batch() {
    // With a valid bearer, the batch is authorized once; each item is still
    // validated independently (the invalid one is reported, not fatal).
    let app = app_with_oauth("tok", "app", "u");
    let good = {
        let ann = Annotation::new(
            Motivation::Commenting,
            Target::Iri(TARGET.into()),
            vec![FbBody::Comment { value: "ok".into() }],
        )
        .with_created("2026-06-21T10:00:00Z");
        serde_json::to_value(ann).unwrap()
    };
    let bad = {
        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri(TARGET.into()),
            vec![FbBody::star(9.0)], // out of bounds
        )
        .with_created("2026-06-21T10:01:00Z");
        serde_json::to_value(ann).unwrap()
    };
    let batch = json!([good, bad]);
    let req = Request::builder()
        .method("POST")
        .uri("/annotations/")
        .header("authorization", "Bearer tok")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&batch).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["succeeded"], 1);
    assert_eq!(body["failed"], 1);
    // The persisted comment got the app-scoped creator.
    assert_eq!(body["results"][0]["status"], 201);
}

#[tokio::test]
async fn batch_with_bad_bearer_is_fatal_401() {
    // A *present* but invalid bearer fails the whole batch request (we cannot
    // attribute any item), unlike a per-item content failure.
    let app = app_with_oauth("tok", "app", "u");
    let batch = json!([signed_star_at(4.0, "2026-06-21T10:00:00Z")]);
    let req = Request::builder()
        .method("POST")
        .uri("/annotations/")
        .header("authorization", "Bearer wrong")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&batch).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn single_item_post_keeps_legacy_201_contract() {
    // A bare object (not an array) is NOT a batch: it keeps the 201 + Location
    // contract, and an invalid one is a flat 422 (not a 207).
    let app = app();
    let (status, headers, body) = send(
        &app,
        "POST",
        "/annotations/",
        Some(signed_star_at(4.0, "2026-06-21T10:00:00Z")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.contains_key("location"));
    assert_eq!(body["type"], "Annotation");

    let (status, _h, body) = send(
        &app,
        "POST",
        "/annotations/",
        Some(signed_star_at(7.0, "2026-06-21T10:05:00Z")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["report"]["conforms"], false);
}

// --- Part 3: Mangrove review-schema export profile ------------------------

#[tokio::test]
async fn mangrove_review_jwt_is_ingested() {
    use freedback_protocol::to_mangrove_jwt;
    let app = app();
    let id = Identity::generate();
    let ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri("https://example.com/place/42".into()),
        vec![
            FbBody::star(4.0),
            FbBody::Comment {
                value: "good".into(),
            },
        ],
    )
    .with_created("2026-06-21T10:00:00Z");
    let jwt = to_mangrove_jwt(&ann, &id).unwrap();

    let (status, headers, body) = send(&app, "PUT", &format!("/submit/mangrove/{jwt}"), None).await;
    assert_eq!(status, StatusCode::CREATED, "Mangrove review JWT accepted");
    assert!(headers.contains_key("location"));
    // The rating round-tripped onto the Mangrove [0,100] scalar scale.
    assert_eq!(body["body"][0]["schema:ratingValue"], 75.0);
    assert_eq!(body["creator"]["id"], id.issuer_id().unwrap());

    // Queryable under the mapped target.
    let (_s, _h, page) = send(
        &app,
        "GET",
        "/annotations/?target=https://example.com/place/42",
        None,
    )
    .await;
    assert_eq!(page["partOf"]["total"], 1);

    // A garbage Mangrove JWT is rejected.
    let (status, _h, _b) = send(&app, "PUT", "/submit/mangrove/not.a.jwt", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn well_known_advertises_new_capabilities() {
    let app = app();
    let (status, _h, doc) = send(&app, "GET", "/.well-known/freedback", None).await;
    assert_eq!(status, StatusCode::OK);
    let caps = doc["capabilities"].as_array().unwrap();
    for cap in ["mangrove-review", "batch-multistatus"] {
        assert!(caps.iter().any(|c| c == cap), "advertises {cap}");
    }
}
