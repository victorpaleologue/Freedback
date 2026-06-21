//! Freedback collection / aggregation server (component 7).
//!
//! Aggregates feedback for a URI across registered feedback servers — politely:
//! it caches per `(server, uri)`, honors the upstream `Cache-Control: max-age`
//! (reusing a fresh page with **no** request at all), and otherwise revalidates
//! with `If-None-Match` / `If-Modified-Since` (cheap 304s). It rate-limits per
//! upstream host with a token bucket, dedups across servers by content-addressed
//! id, and unifies results across **equivalent** URIs.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use freedback_protocol::{dedup_id, Annotation};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

pub mod equivalence;
pub mod token_bucket;

use equivalence::EquivalenceSet;
use token_bucket::TokenBucket;

/// Per-`(server, uri)` cache entry for conditional revalidation.
///
/// Holds both validators (`ETag`, `Last-Modified`) for cheap 304s and a
/// freshness deadline derived from the upstream `Cache-Control: max-age`, within
/// which the entry is reused with **no** upstream request at all.
#[derive(Default, Clone)]
struct CacheEntry {
    etag: Option<String>,
    last_modified: Option<String>,
    /// `Instant` until which the entry is fresh (from `Cache-Control: max-age`).
    fresh_until: Option<Instant>,
    items: Vec<Annotation>,
}

impl CacheEntry {
    /// Still within the upstream-granted freshness lifetime?
    fn is_fresh(&self) -> bool {
        self.fresh_until.is_some_and(|t| Instant::now() < t)
    }
}

/// Rate-limit configuration.
#[derive(Clone, Copy)]
pub struct RateLimit {
    pub capacity: f64,
    pub refill_per_sec: f64,
}

impl Default for RateLimit {
    fn default() -> Self {
        // Generous default; tests tighten it.
        Self {
            capacity: 30.0,
            refill_per_sec: 10.0,
        }
    }
}

/// Shared collection-server state.
#[derive(Clone)]
pub struct AppState {
    servers: Arc<Mutex<BTreeSet<String>>>,
    cache: Arc<Mutex<HashMap<(String, String), CacheEntry>>>,
    eq: Arc<Mutex<EquivalenceSet>>,
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    rate: RateLimit,
    http: reqwest::Client,
    base_url: String,
    upstream_calls: Arc<AtomicU64>,
    upstream_304: Arc<AtomicU64>,
    cache_hits: Arc<AtomicU64>,
    /// Wrap the router in a permissive CORS layer so a cross-origin browser
    /// widget can read `/index`. Off by default; set by the binary via
    /// `FREEDBACK_CORS_PERMISSIVE` and by the widgets E2E harness.
    cors_permissive: bool,
}

impl AppState {
    /// Build state with the default rate limit.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_rate(base_url, RateLimit::default())
    }

    /// Build state with a specific rate limit.
    pub fn with_rate(base_url: impl Into<String>, rate: RateLimit) -> Self {
        Self {
            servers: Arc::new(Mutex::new(BTreeSet::new())),
            cache: Arc::new(Mutex::new(HashMap::new())),
            eq: Arc::new(Mutex::new(EquivalenceSet::new())),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            rate,
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            upstream_calls: Arc::new(AtomicU64::new(0)),
            upstream_304: Arc::new(AtomicU64::new(0)),
            cache_hits: Arc::new(AtomicU64::new(0)),
            cors_permissive: false,
        }
    }

    /// Enable a permissive CORS layer (cross-origin browser widgets).
    pub fn with_cors_permissive(mut self, on: bool) -> Self {
        self.cors_permissive = on;
        self
    }

    /// Register an upstream feedback server (used in tests/bootstrap).
    pub fn add_server(&self, url: &str) {
        self.servers.lock().unwrap().insert(normalize(url));
    }

    /// Total upstream GETs actually issued (after rate limiting).
    pub fn upstream_calls(&self) -> u64 {
        self.upstream_calls.load(Ordering::Relaxed)
    }
    /// Of those, how many returned `304 Not Modified`.
    pub fn upstream_304(&self) -> u64 {
        self.upstream_304.load(Ordering::Relaxed)
    }
    /// Reads served from a still-fresh cache entry with **no** upstream request
    /// (the `Cache-Control: max-age` freshness win).
    pub fn cache_hits(&self) -> u64 {
        self.cache_hits.load(Ordering::Relaxed)
    }

    /// Try to spend one token for `host`.
    fn allow(&self, host: &str) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        buckets
            .entry(host.to_string())
            .or_insert_with(|| TokenBucket::new(self.rate.capacity, self.rate.refill_per_sec))
            .try_acquire()
    }
}

/// Build the collection-server router.
pub fn build_app(state: AppState) -> Router {
    let cors_permissive = state.cors_permissive;
    let router = Router::new()
        .route("/servers", post(register_server).get(list_servers))
        .route("/index", get(index))
        .route("/equivalence", post(post_equivalence).get(get_equivalence))
        .route("/debug/metrics", get(metrics))
        .route("/.well-known/freedback", get(well_known))
        .layer(TraceLayer::new_for_http());

    let router = if cors_permissive {
        use axum::http::{header, Method};
        router.layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::ACCEPT]),
        )
    } else {
        router
    };

    router.with_state(state)
}

fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn host_of(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    no_scheme.split('/').next().unwrap_or(no_scheme).to_string()
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// --- handlers --------------------------------------------------------------

#[derive(Deserialize)]
struct ServerBody {
    url: String,
}

async fn register_server(
    State(state): State<AppState>,
    Json(body): Json<ServerBody>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let base = normalize(&body.url);
    let well_known = format!("{base}/.well-known/freedback");
    let resp = state.http.get(&well_known).send().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("cannot reach server: {e}") })),
        )
    })?;
    let ok = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|d| {
            d.get("protocol")
                .and_then(Value::as_str)
                .map(|s| s == "freedback/1")
        })
        .unwrap_or(false);
    if !ok {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": "not a freedback/1 server" })),
        ));
    }
    state.add_server(&base);
    Ok(Json(json!({ "ok": true, "server": base })))
}

async fn list_servers(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "servers": state.servers.lock().unwrap().iter().cloned().collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct IndexParams {
    target: String,
}

/// `GET /index?target=` — merged, deduped aggregate across servers + equivalents.
async fn index(State(state): State<AppState>, Query(p): Query<IndexParams>) -> Json<Value> {
    let class = state.eq.lock().unwrap().class(&p.target);
    let servers: Vec<String> = state.servers.lock().unwrap().iter().cloned().collect();

    // Collect annotations from every (server, uri) pair, deduped by content id.
    let mut merged: HashMap<String, Annotation> = HashMap::new();
    for uri in &class {
        for server in &servers {
            let items = state.fetch(server, uri).await;
            for ann in items {
                if let Ok(id) = dedup_id(&ann) {
                    merged.entry(id).or_insert(ann);
                }
            }
        }
    }

    let mut items: Vec<Annotation> = merged.into_values().collect();
    items.sort_by_key(|a| a.iat().unwrap_or(0));

    Json(json!({
        "target": p.target,
        "equivalents": class,
        "servers": servers,
        "total": items.len(),
        "items": items,
    }))
}

impl AppState {
    /// Fetch one `(server, uri)` collection with caching + conditional GET +
    /// rate limiting. Returns the freshest items known (possibly from cache).
    async fn fetch(&self, server: &str, uri: &str) -> Vec<Annotation> {
        let key = (server.to_string(), uri.to_string());
        let cached = self
            .cache
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_default();

        // Freshness (RFC 7234): while the upstream `Cache-Control: max-age` has
        // not elapsed, reuse the entry with no upstream request — not even a
        // conditional one. This is the cheapest path and spends no rate budget.
        if cached.is_fresh() {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return cached.items;
        }

        // Rate limit per host: if no token, serve whatever we have cached.
        if !self.allow(&host_of(server)) {
            return cached.items;
        }

        let url = format!("{server}/annotations/?target={}", urlencode(uri));
        let mut req = self.http.get(&url);
        // Send both validators we hold so the upstream can answer 304 by ETag or
        // by modification date.
        if let Some(etag) = &cached.etag {
            req = req.header(axum::http::header::IF_NONE_MATCH, etag);
        }
        if let Some(lm) = &cached.last_modified {
            req = req.header(axum::http::header::IF_MODIFIED_SINCE, lm);
        }

        self.upstream_calls.fetch_add(1, Ordering::Relaxed);
        let Ok(resp) = req.send().await else {
            return cached.items;
        };

        if resp.status() == axum::http::StatusCode::NOT_MODIFIED {
            self.upstream_304.fetch_add(1, Ordering::Relaxed);
            // A 304 may refresh freshness (and the validators) without a body.
            let fresh_until = max_age(resp.headers()).map(|d| Instant::now() + d);
            let mut entry = cached.clone();
            entry.fresh_until = fresh_until;
            if let Some(lm) = last_modified_of(resp.headers()) {
                entry.last_modified = Some(lm);
            }
            self.cache.lock().unwrap().insert(key, entry);
            return cached.items;
        }
        if !resp.status().is_success() {
            return cached.items;
        }

        let headers = resp.headers().clone();
        let etag = headers
            .get(axum::http::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let last_modified = last_modified_of(&headers);
        let fresh_until = max_age(&headers).map(|d| Instant::now() + d);
        let no_store = no_store(&headers);

        let Ok(doc) = resp.json::<Value>().await else {
            return cached.items;
        };
        let items = parse_items(&doc);

        if !no_store {
            self.cache.lock().unwrap().insert(
                key,
                CacheEntry {
                    etag,
                    last_modified,
                    fresh_until,
                    items: items.clone(),
                },
            );
        }
        items
    }
}

/// Parse `Cache-Control: max-age=<n>` (seconds) from response headers.
fn max_age(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let cc = headers
        .get(axum::http::header::CACHE_CONTROL)?
        .to_str()
        .ok()?;
    for directive in cc.split(',') {
        let directive = directive.trim();
        if let Some(v) = directive.strip_prefix("max-age=") {
            return v.trim().parse::<u64>().ok().map(Duration::from_secs);
        }
    }
    None
}

/// Does `Cache-Control` forbid storing the response?
fn no_store(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(axum::http::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(|cc| cc.split(',').any(|d| d.trim() == "no-store"))
        .unwrap_or(false)
}

/// Extract the `Last-Modified` validator string, if any.
fn last_modified_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn parse_items(doc: &Value) -> Vec<Annotation> {
    let arr = match doc {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    arr.into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

#[derive(Deserialize)]
struct EquivalenceBody {
    a: String,
    b: String,
    proof: Option<String>,
}

async fn post_equivalence(
    State(state): State<AppState>,
    Json(body): Json<EquivalenceBody>,
) -> Json<Value> {
    let proof = body.proof.unwrap_or_else(|| "manual".to_string());
    state.eq.lock().unwrap().union(&body.a, &body.b, proof);
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct EqQuery {
    uri: String,
}

async fn get_equivalence(State(state): State<AppState>, Query(q): Query<EqQuery>) -> Json<Value> {
    Json(json!({ "uri": q.uri, "class": state.eq.lock().unwrap().class(&q.uri) }))
}

async fn metrics(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "upstreamCalls": state.upstream_calls(),
        "upstream304": state.upstream_304(),
        "cacheHits": state.cache_hits(),
    }))
}

async fn well_known(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["collection-index", "equivalence", "polite-cache"],
        "conformsTo": "https://freedback.org/profile/1",
        "links": [
            { "rel": "self", "href": format!("{}/.well-known/freedback", state.base_url) },
            { "rel": "index", "href": format!("{}/index", state.base_url) }
        ]
    }))
}
