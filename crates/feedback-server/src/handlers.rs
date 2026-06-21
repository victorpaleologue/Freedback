//! HTTP handlers for the feedback server.

use axum::extract::{Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use freedback_protocol::{dedup_id, Annotation};
use freedback_storage::Query as StoreQuery;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::authorize;
use crate::collection::build_page;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CollectionParams {
    pub target: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SyncParams {
    pub target: String,
    pub gt_iat: Option<i64>,
    pub latest_edits_only: Option<bool>,
}

fn parse_annotations(value: Value) -> Result<Vec<Annotation>, ApiError> {
    // JSON-LD ingest is primary: accept any conformant W3C Web Annotation
    // serialization (not just our exact serde shape) and normalize it to the
    // canonical model, so dedup ids / signatures are serialization-independent
    // (see protocol-lib::jsonld + ADR 0007).
    // Fast path: the alias normalizer resolves any serialization over the
    // pinned Freedback/anno vocabulary. Fallback: a document whose terms come
    // from a third party's own inline `@context` is compacted against our
    // pinned context first (ADR 0011), so foreign vocabularies content-address
    // identically. The fallback's error is only surfaced if it too fails.
    let parse_one = |v: &Value| match freedback_protocol::from_jsonld(v) {
        Ok(ann) => Ok(ann),
        Err(fast_err) => freedback_protocol::jsonld_full::normalize_full(v).map_err(|full_err| {
            ApiError::bad_request(format!(
                "invalid annotation: {fast_err} (full compaction also failed: {full_err})"
            ))
        }),
    };
    match value {
        Value::Array(arr) => arr.iter().map(parse_one).collect(),
        other => Ok(vec![parse_one(&other)?]),
    }
}

/// `POST /annotations/` — POST-to-container (single annotation or batch).
pub async fn post_annotations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    let mut anns = parse_annotations(value)?;
    if anns.is_empty() {
        return Err(ApiError::bad_request("empty submission"));
    }

    // Authorize the whole batch (OAuth bearer OR every annotation self-signed).
    let authz = authorize(&state.oauth, &headers, &anns)?;

    // Validate everything before persisting anything.
    for ann in &mut anns {
        if let Some(creator) = authz.oauth_creator() {
            if ann.creator.is_none() {
                ann.creator = Some(creator);
            }
        }
        ann.structural_check()?;
        let outcome = state.validator.validate(ann)?;
        if !outcome.conforms {
            return Err(ApiError::Validation(outcome.violations));
        }
        let id = dedup_id(ann)?;
        ann.id = Some(format!("{}/annotations/{}", state.base_url, id));
    }

    // Persist (idempotent by dedup id).
    for ann in &anns {
        state.store.put(ann).await?;
    }

    let mut out_headers = HeaderMap::new();
    let body = if anns.len() == 1 {
        if let Some(id) = anns[0].id.as_deref() {
            if let Ok(v) = HeaderValue::from_str(id) {
                out_headers.insert(LOCATION, v);
            }
        }
        serde_json::to_value(&anns[0])?
    } else {
        json!(anns)
    };

    Ok((StatusCode::CREATED, out_headers, Json(body)))
}

/// `GET /annotations/?target=&page=&page_size=` — paginated collection read.
///
/// Honors `If-None-Match` (conditional GET): when the caller's ETag matches the
/// freshly computed page ETag, returns `304 Not Modified` with no body, so a
/// polite aggregator (the collection server) costs one cheap 304.
pub async fn get_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<CollectionParams>,
) -> Result<axum::response::Response, ApiError> {
    let page = p.page.unwrap_or(0);
    let page_size = p.page_size.unwrap_or(state.page_size);
    let result = state
        .store
        .query(&StoreQuery {
            target: p.target.clone(),
            page,
            page_size,
        })
        .await?;

    let view = build_page(
        &state.base_url,
        p.target.as_deref(),
        page,
        page_size,
        result.total,
        &result.items,
    );

    // Conditional GET: 304 when the client's ETag matches.
    if let (Some(inm), Some(etag)) = (
        headers.get(axum::http::header::IF_NONE_MATCH),
        view.headers.get(axum::http::header::ETAG),
    ) {
        if inm == etag {
            let mut not_modified = axum::response::Response::new(axum::body::Body::empty());
            *not_modified.status_mut() = StatusCode::NOT_MODIFIED;
            not_modified
                .headers_mut()
                .insert(axum::http::header::ETAG, etag.clone());
            return Ok(not_modified);
        }
    }

    Ok((view.headers, Json(view.body)).into_response())
}

/// `PUT /submit/{jwt}` — Mangrove-style export-profile ingest.
///
/// The annotation is carried as an ES256 JWT; its signature is the issuer proof,
/// so this path needs no bearer/self-signature. The payload is normalized,
/// SHACL-validated, and stored like any other write.
pub async fn submit(
    State(state): State<AppState>,
    Path(jwt): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut ann = freedback_protocol::from_jwt(&jwt)
        .map_err(|e| ApiError::unauthorized(format!("invalid JWT: {e}")))?;

    ann.structural_check()?;
    let outcome = state.validator.validate(&ann)?;
    if !outcome.conforms {
        return Err(ApiError::Validation(outcome.violations));
    }
    let id = dedup_id(&ann)?;
    ann.id = Some(format!("{}/annotations/{}", state.base_url, id));
    state.store.put(&ann).await?;

    let mut headers = HeaderMap::new();
    if let Some(id) = ann.id.as_deref() {
        if let Ok(v) = HeaderValue::from_str(id) {
            headers.insert(LOCATION, v);
        }
    }
    Ok((
        StatusCode::CREATED,
        headers,
        Json(serde_json::to_value(&ann)?),
    ))
}

/// `GET /annotations/{id}` — single annotation by dedup id.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get(&id).await? {
        Some(mut ann) => {
            ann.id = Some(format!("{}/annotations/{}", state.base_url, id));
            Ok(Json(ann))
        }
        None => Err(ApiError::not_found("annotation not found")),
    }
}

/// `GET /sync?target=&gt_iat=&latest_edits_only=` — incremental cursor.
pub async fn get_sync(
    State(state): State<AppState>,
    Query(p): Query<SyncParams>,
) -> Result<Json<Value>, ApiError> {
    let items = state
        .store
        .sync(
            &p.target,
            p.gt_iat.unwrap_or(0),
            p.latest_edits_only.unwrap_or(true),
        )
        .await?;
    Ok(Json(json!(items)))
}

/// `GET /.well-known/freedback` — capabilities self-description.
pub async fn well_known(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["wap-container", "sync-cursor", "jws-identity", "oauth-identity", "jwt-export"],
        "conformsTo": "https://freedback.org/profile/1",
        "links": [
            { "rel": "self", "href": format!("{}/.well-known/freedback", state.base_url) },
            { "rel": "http://www.w3.org/ns/oa#annotationService",
              "href": format!("{}/annotations/", state.base_url) }
        ]
    }))
}
