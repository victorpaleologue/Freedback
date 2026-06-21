//! Freedback collection / aggregation server (component 7).
//!
//! Aggregates feedback for a URI across registered feedback servers — politely:
//! it caches per `(server, uri)`, revalidates with `If-None-Match` (cheap 304s),
//! rate-limits per upstream host with a token bucket, dedups across servers by
//! content-addressed id, and unifies results across **equivalent** URIs.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
#[derive(Default, Clone)]
struct CacheEntry {
    etag: Option<String>,
    items: Vec<Annotation>,
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
        }
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
    Router::new()
        .route("/servers", post(register_server).get(list_servers))
        .route("/index", get(index))
        .route("/equivalence", post(post_equivalence).get(get_equivalence))
        .route("/debug/metrics", get(metrics))
        .route("/.well-known/freedback", get(well_known))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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

        // Rate limit per host: if no token, serve whatever we have cached.
        if !self.allow(&host_of(server)) {
            return cached.items;
        }

        let url = format!("{server}/annotations/?target={}", urlencode(uri));
        let mut req = self.http.get(&url);
        if let Some(etag) = &cached.etag {
            req = req.header(axum::http::header::IF_NONE_MATCH, etag);
        }

        self.upstream_calls.fetch_add(1, Ordering::Relaxed);
        let Ok(resp) = req.send().await else {
            return cached.items;
        };

        if resp.status() == axum::http::StatusCode::NOT_MODIFIED {
            self.upstream_304.fetch_add(1, Ordering::Relaxed);
            return cached.items;
        }
        if !resp.status().is_success() {
            return cached.items;
        }

        let etag = resp
            .headers()
            .get(axum::http::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let Ok(doc) = resp.json::<Value>().await else {
            return cached.items;
        };
        let items = parse_items(&doc);

        self.cache.lock().unwrap().insert(
            key,
            CacheEntry {
                etag,
                items: items.clone(),
            },
        );
        items
    }
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
