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
        return Ok(resp);
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

/// Body of `POST /negentropy`: the target whose set is reconciled plus the
/// client's range message for this round.
#[derive(Debug, Deserialize)]
pub struct NegentropyBody {
    /// The target URI whose annotation set is being reconciled.
    pub target: String,
    /// The client's per-range claims for this round.
    pub message: freedback_protocol::Message,
}

/// Build the server's sorted negentropy item set (`(iat, dedup_id)`) for a
/// target — the **full** id set, NOT collapsed to latest edits, since
/// reconciliation diffs content-addressed ids one-for-one.
async fn negentropy_items(
    state: &AppState,
    target: &str,
) -> Result<Vec<freedback_protocol::Item>, ApiError> {
    let page = state
        .store
        .query(&StoreQuery {
            target: Some(target.to_string()),
            page: 0,
            page_size: 0, // 0 = all
        })
        .await?;
    let mut items = Vec::with_capacity(page.items.len());
    for ann in &page.items {
        items.push(freedback_protocol::Item::new(
            ann.iat().unwrap_or(0),
            dedup_id(ann)?,
        ));
    }
    Ok(freedback_protocol::negentropy::sorted(items))
}

/// `POST /negentropy` — one round of NIP-77-style range reconciliation.
///
/// The client posts its per-range claims; the server answers each range over its
/// own set (matching fingerprints settle, mismatches split/recurse, small ranges
/// return explicit ids). Stateless: the server reads only its set, so each round
/// is an independent request (INVARIANT 7, HTTP batch not real-time).
pub async fn post_negentropy(
    State(state): State<AppState>,
    Json(body): Json<NegentropyBody>,
) -> Result<Json<freedback_protocol::Message>, ApiError> {
    let items = negentropy_items(&state, &body.target).await?;
    let reply = freedback_protocol::negentropy::respond(&items, &body.message);
    Ok(Json(reply))
}

/// Body of `POST /annotations/by-id`: the dedup ids to fetch in bulk.
#[derive(Debug, Deserialize)]
pub struct ByIdBody {
    /// The dedup ids to fetch.
    pub ids: Vec<String>,
}

/// `POST /annotations/by-id` — bulk fetch annotations by dedup id.
///
/// The reconcile path resolves a small set of `need` ids via negentropy, then
/// fetches exactly those (and no more), keeping the transfer O(diff). Unknown
/// ids are silently skipped.
pub async fn post_by_id(
    State(state): State<AppState>,
    Json(body): Json<ByIdBody>,
) -> Result<Json<Value>, ApiError> {
    let mut out = Vec::with_capacity(body.ids.len());
    for id in &body.ids {
        if let Some(mut ann) = state.store.get(id).await? {
            ann.id = Some(format!("{}/annotations/{}", state.base_url, id));
            out.push(ann);
        }
    }
    Ok(Json(json!(out)))
}

/// `GET /.well-known/freedback` — capabilities self-description.
pub async fn well_known(State(state): State<AppState>) -> Json<Value> {
    let mut doc = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["wap-container", "sync-cursor", "jws-identity", "oauth-identity", "jwt-export", "negentropy"],
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
