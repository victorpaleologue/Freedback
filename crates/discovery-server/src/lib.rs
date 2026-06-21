//! Freedback discovery server (component 2): a registry that is "just another
//! conformant server".
//!
//! A server announces its URL; the registry **verifies by fetching that
//! server's `/.well-known/freedback`** (RFC 8615) — it never trusts the POSTed
//! URL on its own. `GET /resolve?target=` returns the announced servers that
//! actually hold feedback for a URI (the flat-list model).
//!
//! On top of that flat model sits a NIP-65-style **outbox** resolver
//! ([`relays`], ADR 0014): an issuer publishes a self-signed [`relays::RelayList`]
//! declaring which servers it writes to / reads from, so `GET /resolve?issuer=`
//! returns where that key's feedback lives without fanning out across the
//! registry.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

pub mod relays;
use relays::RelayList;

/// Shared registry state.
#[derive(Clone)]
pub struct AppState {
    servers: Arc<Mutex<BTreeSet<String>>>,
    /// NIP-65-style relay lists, keyed by issuer id (replaceable, newest wins).
    relays: Arc<Mutex<HashMap<String, RelayList>>>,
    http: reqwest::Client,
    base_url: String,
}

impl AppState {
    /// Build registry state with its own public base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            servers: Arc::new(Mutex::new(BTreeSet::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// The currently announced servers.
    pub fn servers(&self) -> Vec<String> {
        self.servers.lock().unwrap().iter().cloned().collect()
    }
}

/// Build the discovery-server router.
pub fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/announce", post(announce))
        .route("/servers", get(servers))
        .route("/relays", post(post_relays).get(get_relays))
        .route("/resolve", get(resolve))
        .route("/.well-known/freedback", get(well_known))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

#[derive(Deserialize)]
struct AnnounceBody {
    url: String,
}

/// `POST /announce {url}` — verify the announced server then record it.
async fn announce(
    State(state): State<AppState>,
    Json(body): Json<AnnounceBody>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let base = normalize(&body.url);
    let well_known = format!("{base}/.well-known/freedback");

    let reject = |msg: &str| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": msg })),
        )
    };

    let resp = state
        .http
        .get(&well_known)
        .send()
        .await
        .map_err(|e| reject(&format!("could not fetch well-known: {e}")))?;
    if !resp.status().is_success() {
        return Err(reject("well-known returned a non-success status"));
    }
    let doc: Value = resp
        .json()
        .await
        .map_err(|_| reject("well-known is not valid JSON"))?;
    if doc.get("protocol").and_then(Value::as_str) != Some("freedback/1") {
        return Err(reject("well-known does not advertise protocol freedback/1"));
    }

    state.servers.lock().unwrap().insert(base.clone());
    Ok(Json(json!({ "ok": true, "server": base })))
}

/// `GET /servers` — the flat list of announced servers.
async fn servers(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "servers": state.servers() }))
}

/// `POST /relays` — publish a self-signed NIP-65-style relay list.
///
/// The registry verifies the issuer's signature (and the issuer/key binding)
/// before storing; it never takes an issuer's claimed servers on faith. The
/// record is replaceable: a newer `updated` supersedes an older one.
async fn post_relays(
    State(state): State<AppState>,
    Json(list): Json<RelayList>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let reject = |msg: String| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": msg })),
        )
    };

    list.verify().map_err(reject)?;

    let mut relays = state.relays.lock().unwrap();
    if let Some(existing) = relays.get(&list.issuer) {
        // Replaceable: keep the newer record (ignore stale re-publishes).
        if existing.updated >= list.updated {
            return Ok(Json(
                json!({ "ok": true, "issuer": list.issuer, "stored": false, "reason": "not newer" }),
            ));
        }
    }
    let issuer = list.issuer.clone();
    relays.insert(issuer.clone(), list);
    Ok(Json(
        json!({ "ok": true, "issuer": issuer, "stored": true }),
    ))
}

#[derive(Deserialize)]
struct RelayQuery {
    issuer: String,
}

/// `GET /relays?issuer=` — the stored signed relay list for an issuer.
async fn get_relays(
    State(state): State<AppState>,
    Query(q): Query<RelayQuery>,
) -> Result<Json<RelayList>, (axum::http::StatusCode, Json<Value>)> {
    state
        .relays
        .lock()
        .unwrap()
        .get(&q.issuer)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({ "error": "no relay list for issuer" })),
            )
        })
}

#[derive(Deserialize)]
struct ResolveParams {
    target: Option<String>,
    issuer: Option<String>,
}

/// `GET /resolve?target=` — announced servers that actually hold feedback for
/// the target (verified live against each server's collection).
///
/// `GET /resolve?issuer=` — the **outbox** resolution: the servers the issuer
/// declared it writes to (from its signed relay list), with no fan-out polling.
async fn resolve(State(state): State<AppState>, Query(p): Query<ResolveParams>) -> Json<Value> {
    if let Some(issuer) = p.issuer {
        let write = state
            .relays
            .lock()
            .unwrap()
            .get(&issuer)
            .map(|rl| rl.write.clone())
            .unwrap_or_default();
        return Json(json!({ "issuer": issuer, "servers": write }));
    }

    let Some(target) = p.target else {
        return Json(json!({ "error": "provide target= or issuer=" }));
    };
    let mut holders = Vec::new();
    for server in state.servers() {
        if server_has_target(&state.http, &server, &target).await {
            holders.push(server);
        }
    }
    Json(json!({ "target": target, "servers": holders }))
}

async fn server_has_target(http: &reqwest::Client, server: &str, target: &str) -> bool {
    let url = format!("{server}/annotations/?target={}", urlencode(target));
    let Ok(resp) = http.get(&url).send().await else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    let Ok(doc) = resp.json::<Value>().await else {
        return false;
    };
    doc.get("partOf")
        .and_then(|p| p.get("total"))
        .and_then(Value::as_u64)
        .map(|t| t > 0)
        .unwrap_or(false)
}

/// `GET /.well-known/freedback` — the registry's own self-description.
async fn well_known(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["discovery-registry", "relay-list"],
        "conformsTo": "https://freedback.org/profile/1",
        "links": [
            { "rel": "self", "href": format!("{}/.well-known/freedback", state.base_url) },
            { "rel": "servers", "href": format!("{}/servers", state.base_url) },
            { "rel": "relays", "href": format!("{}/relays", state.base_url) }
        ]
    }))
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
