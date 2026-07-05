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

/// Methods a single `/annotations/{id}` item accepts. `DELETE` is the WAP verb
/// for the author's right to erasure (ADR 0021).
pub const ITEM_ALLOW: &str = "GET, HEAD, DELETE, OPTIONS";

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
        "@context": "https://freedback.net/ns/batch/1",
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
///
/// An erased annotation answers `410 Gone` (a tombstone exists — ADR 0021),
/// distinguishing "deleted by its author" from a plain `404` never-seen id.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get(&id).await? {
        Some(mut ann) => {
            ann.id = Some(format!("{}/annotations/{}", state.base_url, id));
            let mut headers = HeaderMap::new();
            headers.insert(ALLOW, HeaderValue::from_static(ITEM_ALLOW));
            Ok((headers, Json(ann)))
        }
        None if state.store.is_tombstoned(&id).await? => {
            Err(ApiError::gone("annotation was deleted"))
        }
        None => Err(ApiError::not_found("annotation not found")),
    }
}

/// `OPTIONS /annotations/{id}` — advertise the methods an item supports
/// (including `DELETE`, the right-to-erasure verb).
pub async fn options_item() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(ALLOW, HeaderValue::from_static(ITEM_ALLOW));
    (StatusCode::NO_CONTENT, headers)
}

/// `DELETE /annotations/{dedup_id}` — the author's right to erasure (ADR 0021).
///
/// The body is a delete document `{"type":"Delete","annotation":"<dedup_id>",
/// "created":"<RFC3339>"}`; authorization matches the identity that created the
/// annotation:
///
/// * **Self-signed annotations** — the document carries a detached ES256
///   signature over its JCS canonical bytes; it must verify AND its key must be
///   the same identity that signed the annotation (derived issuer id equals the
///   annotation's `creator.id`, or the `kid`s are equal). `403` otherwise.
/// * **OAuth annotations** — a valid bearer resolving to the same
///   `(app_id, user_id)` creator; the document may omit the signature. `403`
///   on a creator mismatch (including a bearer trying to erase a self-signed
///   annotation, and vice versa).
///
/// On success the content is erased and a content-free tombstone retained;
/// the response is `204 No Content`. Deleting an **already-erased** id is
/// idempotent: `204` again. An id never seen at all is `404`.
pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Result<axum::response::Response, ApiError> {
    let doc: freedback_protocol::DeleteRequest = serde_json::from_value(value)
        .map_err(|e| ApiError::bad_request(format!("invalid delete document: {e}")))?;
    if doc.type_ != freedback_protocol::erasure::DELETE_TYPE {
        return Err(ApiError::bad_request(format!(
            "delete document type must be \"Delete\", got {:?}",
            doc.type_
        )));
    }
    if doc.annotation != id {
        return Err(ApiError::bad_request(
            "the document's `annotation` must equal the dedup id in the path",
        ));
    }

    let Some(ann) = state.store.get(&id).await? else {
        if state.store.is_tombstoned(&id).await? {
            // Idempotent: already erased. Re-affirm with 204, not an error.
            return Ok(no_content_with_allow());
        }
        return Err(ApiError::not_found("annotation not found"));
    };

    match crate::auth::oauth_authz(&state.oauth, &headers)? {
        // OAuth path: the bearer's app-scoped creator must be the annotation's.
        Some(authz) => {
            let creator = authz
                .oauth_creator()
                .expect("oauth_authz only yields OAuth identities");
            if ann.creator.as_ref().map(|c| c.id.as_str()) != Some(creator.id.as_str()) {
                return Err(ApiError::forbidden(
                    "bearer identity is not the annotation's creator",
                ));
            }
        }
        // Self-signed path: same canonicalization, same scheme, same key as
        // the annotation itself (protocol-lib::erasure).
        None => {
            if doc.signature.is_none() {
                return Err(ApiError::unauthorized(
                    "no bearer token and the delete document is unsigned",
                ));
            }
            let issuer = freedback_protocol::verify_delete(&doc)
                .map_err(|_| ApiError::forbidden("delete signature verification failed"))?;
            let creator_matches =
                ann.creator.as_ref().map(|c| c.id.as_str()) == Some(issuer.as_str());
            let kid_matches = match (&ann.signature, &doc.signature) {
                (Some(a), Some(d)) => a.kid == d.kid,
                _ => false,
            };
            if !(creator_matches || kid_matches) {
                return Err(ApiError::forbidden(
                    "the delete key is not the annotation's creator",
                ));
            }
        }
    }

    let deleted_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let proof = serde_json::to_value(&doc)?;
    state.store.delete(&id, deleted_at, proof).await?;
    Ok(no_content_with_allow())
}

/// A `204 No Content` carrying the item `Allow` header.
fn no_content_with_allow() -> axum::response::Response {
    let mut headers = HeaderMap::new();
    headers.insert(ALLOW, HeaderValue::from_static(ITEM_ALLOW));
    (StatusCode::NO_CONTENT, headers).into_response()
}

/// Query parameters of `GET /tombstones`.
#[derive(Debug, Deserialize)]
pub struct TombstoneParams {
    /// Exclusive cursor: only tombstones with `deleted_at > gt_deleted_at`.
    pub gt_deleted_at: Option<i64>,
}

/// `GET /tombstones?gt_deleted_at=` — the erasure propagation feed (ADR 0021).
///
/// Returns the content-free tombstones (`{dedup_id, deleted_at, proof}`)
/// ordered by `deleted_at` ascending, so sync consumers (collection servers,
/// advanced clients) can evict erased annotations using `deleted_at` as their
/// cursor. Additive endpoint: the `/sync` item shape is unchanged.
pub async fn get_tombstones(
    State(state): State<AppState>,
    Query(p): Query<TombstoneParams>,
) -> Result<Json<Value>, ApiError> {
    let tombs = state
        .store
        .tombstones(p.gt_deleted_at.unwrap_or(i64::MIN))
        .await?;
    Ok(Json(json!(tombs)))
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
        "capabilities": ["wap-container", "sync-cursor", "jws-identity", "oauth-identity", "jwt-export", "mangrove-review", "batch-multistatus", "negentropy", "erasure"],
        "conformsTo": "https://freedback.net/profile/1",
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
