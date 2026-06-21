//! HTTP handlers for the feedback server.

use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, ALLOW, LOCATION};
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

/// Methods the `/annotations/` container accepts — the `Allow` header value and
/// the body of an `OPTIONS` response (W3C WAP / LDP §4.2.8).
pub const CONTAINER_ALLOW: &str = "GET, HEAD, POST, OPTIONS";

/// Whether an `Accept` header is satisfiable by our JSON-LD representation.
///
/// We serve `application/ld+json` (a JSON subtype). A request is acceptable if
/// it has no `Accept`, or accepts `*/*`, `application/*`, `application/json`,
/// `application/ld+json`, or `application/activity+json`. Anything else (e.g.
/// `text/html` only) earns a `406`. Media-type parameters and the `q=` weight
/// are ignored for this coarse check.
fn accepts_json_ld(headers: &HeaderMap) -> bool {
    let Some(accept) = headers.get(ACCEPT).and_then(|v| v.to_str().ok()) else {
        return true; // no Accept ⇒ no constraint
    };
    accept.split(',').any(|part| {
        let media = part.split(';').next().unwrap_or("").trim();
        matches!(
            media,
            "" | "*/*"
                | "application/*"
                | "application/json"
                | "application/ld+json"
                | "application/activity+json"
        )
    })
}

/// `406 Not Acceptable` when the caller cannot accept our JSON-LD media type.
fn check_acceptable(headers: &HeaderMap) -> Result<(), ApiError> {
    if accepts_json_ld(headers) {
        Ok(())
    } else {
        Err(ApiError::not_acceptable(
            "this container serves application/ld+json",
        ))
    }
}

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

/// Normalize one JSON value into the canonical annotation model.
///
/// JSON-LD ingest is primary: accept any conformant W3C Web Annotation
/// serialization (not just our exact serde shape) and normalize it, so dedup
/// ids / signatures are serialization-independent (see protocol-lib::jsonld +
/// ADR 0007). Fast path: the alias normalizer resolves any serialization over
/// the pinned Freedback/anno vocabulary. Fallback: a document whose terms come
/// from a third party's own inline `@context` is compacted against our pinned
/// context first (ADR 0011), so foreign vocabularies content-address
/// identically. The fallback's error is only surfaced if it too fails.
fn parse_one(v: &Value) -> Result<Annotation, ApiError> {
    match freedback_protocol::from_jsonld(v) {
        Ok(ann) => Ok(ann),
        Err(fast_err) => freedback_protocol::jsonld_full::normalize_full(v).map_err(|full_err| {
            ApiError::bad_request(format!(
                "invalid annotation: {fast_err} (full compaction also failed: {full_err})"
            ))
        }),
    }
}

fn parse_annotations(value: &Value) -> Result<Vec<Annotation>, ApiError> {
    match value {
        Value::Array(arr) => arr.iter().map(parse_one).collect(),
        other => Ok(vec![parse_one(other)?]),
    }
}

/// Validate + persist one already-authorized annotation, stamping the OAuth
/// creator (when present) and the server-assigned id. Errors are returned
/// rather than converted to a response, so a batch can report them per item.
async fn ingest_one(
    state: &AppState,
    authz: &crate::auth::Authz,
    ann: &mut Annotation,
) -> Result<String, ApiError> {
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
    let full_id = format!("{}/annotations/{}", state.base_url, id);
    ann.id = Some(full_id.clone());
    state.store.put(ann).await?;
    Ok(full_id)
}

/// Decide whether a conditional GET may answer `304 Not Modified`, given the
/// request headers and the freshly built page's validators.
///
/// `If-None-Match` wins when present (RFC 7232 §3.3): a matching ETag → 304.
/// Otherwise `If-Modified-Since` is honored — `304` when the page's
/// `Last-Modified` is not newer than the client's timestamp.
fn not_modified(req: &HeaderMap, page: &HeaderMap) -> bool {
    if let Some(inm) = req.get(axum::http::header::IF_NONE_MATCH) {
        return page
            .get(axum::http::header::ETAG)
            .map(|etag| inm == etag)
            .unwrap_or(false);
    }
    if let (Some(ims), Some(lm)) = (
        req.get(axum::http::header::IF_MODIFIED_SINCE),
        page.get(axum::http::header::LAST_MODIFIED),
    ) {
        if let (Some(ims), Some(lm)) = (
            ims.to_str().ok().and_then(crate::httpdate::parse),
            lm.to_str().ok().and_then(crate::httpdate::parse),
        ) {
            return lm <= ims;
        }
    }
    false
}

/// `POST /annotations/` — POST-to-container.
///
/// Two shapes, chosen by the request body:
///
/// * **Single object** → the legacy contract: `201 Created` with the stored
///   annotation and a `Location`, or a `4xx`/`422` error for the whole request.
/// * **JSON array (batch)** → **partial-failure** semantics. Each item is
///   authorized, validated, and persisted *independently*; the response is
///   `207 Multi-Status` whose body lists every item's outcome in submission
///   order. Valid items are persisted even when siblings fail
///   (**persist-valid-items** policy — see ADR 0018); an invalid item never
///   blocks a valid one. The batch as a whole is `207` whenever it parses,
///   including the all-success and all-failure cases, so a client always reads
///   outcomes from the same place.
///
/// Authorization is still evaluated per item: an OAuth bearer authorizes the
/// whole batch, otherwise each item must carry its own valid self-signature, so
/// one unsigned/forged item fails only itself.
pub async fn post_annotations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Result<axum::response::Response, ApiError> {
    let is_batch = value.is_array();
    let mut anns = parse_annotations(&value)?;
    if anns.is_empty() {
        return Err(ApiError::bad_request("empty submission"));
    }

    if !is_batch {
        // --- Single-item path: all-or-nothing, unchanged contract. ---
        let authz = authorize(&state.oauth, &headers, &anns)?;
        let ann = &mut anns[0];
        let full_id = ingest_one(&state, &authz, ann).await?;
        let mut out_headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&full_id) {
            out_headers.insert(LOCATION, v);
        }
        return Ok((StatusCode::CREATED, out_headers, Json(json!(ann))).into_response());
    }

    // --- Batch path: per-item outcomes (207 Multi-Status). ---
    // OAuth authorizes the whole batch up front; a bad token fails the request.
    // For self-signed batches we authorize each item individually below so a
    // single bad signature is reported, not fatal.
    let batch_authz = match crate::auth::oauth_authz(&state.oauth, &headers) {
        Ok(Some(authz)) => Some(authz),
        Ok(None) => None,        // no bearer ⇒ per-item self-signed
        Err(e) => return Err(e), // a *present* but invalid bearer is fatal
    };

    let mut results = Vec::with_capacity(anns.len());
    for (index, ann) in anns.iter_mut().enumerate() {
        // Authorize this item.
        let authz = match &batch_authz {
            Some(a) => a.clone(),
            None => match crate::auth::authorize_one_self_signed(ann) {
                Ok(a) => a,
                Err(e) => {
                    results.push(item_failure(index, &e));
                    continue;
                }
            },
        };
        match ingest_one(&state, &authz, ann).await {
            Ok(full_id) => results.push(json!({
                "index": index,
                "status": 201,
                "id": full_id,
            })),
            Err(e) => results.push(item_failure(index, &e)),
        }
    }

    let succeeded = results.iter().filter(|r| r["status"] == 201).count();
    let body = json!({
        "@context": "https://freedback.org/ns/batch/1",
        "type": "BatchResult",
        "total": results.len(),
        "succeeded": succeeded,
        "failed": results.len() - succeeded,
        "results": results,
    });
    Ok((StatusCode::MULTI_STATUS, Json(body)).into_response())
}

/// Render one failed batch item as a JSON result object (mirrors the standalone
/// error bodies: a SHACL failure carries its `report`, others a flat message).
fn item_failure(index: usize, err: &ApiError) -> Value {
    let (status, mut obj) = err.as_item();
    obj["index"] = json!(index);
    obj["status"] = json!(status.as_u16());
    obj
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
    // Content negotiation: reject a caller that cannot accept JSON-LD (406).
    check_acceptable(&headers)?;
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
        state.cache_max_age,
    );

    // Conditional GET. Per RFC 7232, `If-None-Match` takes precedence over
    // `If-Modified-Since`; we evaluate the ETag first, then fall back to the
    // `Last-Modified` validator so an aggregator that only kept the date still
    // earns a cheap 304.
    if not_modified(&headers, &view.headers) {
        let mut resp = axum::response::Response::new(axum::body::Body::empty());
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        // Echo the validators a 304 is allowed to carry.
        for h in [
            axum::http::header::ETAG,
            axum::http::header::CACHE_CONTROL,
            axum::http::header::LAST_MODIFIED,
        ] {
            if let Some(v) = view.headers.get(&h) {
                resp.headers_mut().insert(h, v.clone());
            }
        }
        resp.headers_mut()
            .insert(ALLOW, HeaderValue::from_static(CONTAINER_ALLOW));
        return Ok(resp);
    }

    let mut out_headers = view.headers;
    out_headers.insert(ALLOW, HeaderValue::from_static(CONTAINER_ALLOW));
    Ok((out_headers, Json(view.body)).into_response())
}

/// `OPTIONS /annotations/` — advertise the methods the container supports.
///
/// Returns `204 No Content` with an `Allow` header (W3C WAP / LDP §4.2.8); a
/// preflight-style probe can learn what the container accepts without a body.
pub async fn options_container() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(ALLOW, HeaderValue::from_static(CONTAINER_ALLOW));
    // `Accept-Post` is an LDP header (no http crate constant); name it directly.
    headers.insert(
        axum::http::HeaderName::from_static("accept-post"),
        HeaderValue::from_static("application/ld+json, application/json"),
    );
    (StatusCode::NO_CONTENT, headers)
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

/// `PUT /submit/mangrove/{jwt}` — Mangrove **review-schema** export-profile
/// ingest.
///
/// Unlike [`submit`] (which carries a Freedback annotation verbatim), the token
/// here is a *Mangrove review* (`sub`/`rating`/`opinion`/`metadata`); it is
/// mapped to our annotation model (protocol-lib::mangrove), then validated and
/// stored on the normal path. The JWT signature is the issuer proof.
pub async fn submit_mangrove(
    State(state): State<AppState>,
    Path(jwt): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut ann = freedback_protocol::from_mangrove_jwt(&jwt)
        .map_err(|e| ApiError::unauthorized(format!("invalid Mangrove review JWT: {e}")))?;

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
    let mut doc = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["wap-container", "sync-cursor", "jws-identity", "oauth-identity", "jwt-export", "mangrove-review", "batch-multistatus"],
        "conformsTo": "https://freedback.org/profile/1",
        "links": [
            { "rel": "self", "href": format!("{}/.well-known/freedback", state.base_url) },
            { "rel": "http://www.w3.org/ns/oa#annotationService",
              "href": format!("{}/annotations/", state.base_url) }
        ]
    });
    // Optionally advertise the server-identity public key so a discovery
    // registry can corroborate a signed announce against it.
    if let Some(key) = &state.server_key_pem {
        doc["key"] = json!(key);
    }
    Json(doc)
}
