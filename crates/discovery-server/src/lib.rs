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
//!
//! ## Discovery hardening (issue #25)
//!
//! 1. **Liveness/expiry.** Each announced server records `last_verified`. A
//!    [`AppState::sweep`] re-fetches every server's well-known and drops the
//!    ones that fail or whose verification has aged past
//!    [`RegistryConfig::server_ttl_secs`]. `sweep` is an explicit entry point
//!    (driven by tests and by the binary's background task) and the clock is
//!    injectable ([`clock`]) so tests never sleep on wall-clock time.
//! 2. **Signed announces.** An announce may carry a detached ES256
//!    self-signature ([`Announce::signature`]). When present it is verified
//!    against the announcing server's published key (the well-known's `"key"`
//!    field), proving control of the key — not just reachability of the URL.
//!    Unsigned announces stay valid (backward compatible).
//! 3. **Cross-registry gossip.** Signed relay lists replicate between registries
//!    ([`AppState::gossip_relays_to`] / [`AppState::ingest_relay_list`]): a list
//!    published to registry A becomes discoverable on registry B, and B
//!    re-verifies the signature before storing — safe to relay untrusted.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

pub mod clock;
pub mod relays;

use clock::{Clock, SystemClock};
use freedback_protocol::identity::{issuer_id_from_pem, verify_es256};
use freedback_protocol::{canonical_json, Identity, Signature};
use relays::RelayList;

/// Tunables for liveness/expiry. All durations are in seconds.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// How long a server's last successful verification stays valid. A
    /// [`AppState::sweep`] drops servers whose `last_verified` is older than
    /// this (or that fail re-verification past this grace window).
    pub server_ttl_secs: u64,
    /// Suggested interval between background sweeps (used by the binary; tests
    /// call `sweep` directly). Does not affect the TTL.
    pub sweep_interval_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            // 1h validity, swept every 5min, by default.
            server_ttl_secs: 3600,
            sweep_interval_secs: 300,
        }
    }
}

/// A verified, announced server.
#[derive(Debug, Clone)]
struct ServerEntry {
    /// `now_unix` at the last successful well-known verification.
    last_verified: u64,
    /// The published server-identity key (P-256 SPKI PEM), if the server
    /// advertised one. Used to corroborate signed announces.
    #[allow(dead_code)]
    key_pem: Option<String>,
}

/// Shared registry state.
#[derive(Clone)]
pub struct AppState {
    /// Announced servers keyed by normalized URL (live entries only after a
    /// sweep removes stale ones).
    servers: Arc<Mutex<BTreeMap<String, ServerEntry>>>,
    /// NIP-65-style relay lists, keyed by issuer id (replaceable, newest wins).
    relays: Arc<Mutex<HashMap<String, RelayList>>>,
    http: reqwest::Client,
    base_url: String,
    config: RegistryConfig,
    clock: Arc<dyn Clock>,
}

impl AppState {
    /// Build registry state with its own public base URL and default config /
    /// system clock.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            servers: Arc::new(Mutex::new(BTreeMap::new())),
            relays: Arc::new(Mutex::new(HashMap::new())),
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            config: RegistryConfig::default(),
            clock: Arc::new(SystemClock),
        }
    }

    /// Override the liveness/expiry config (builder style).
    pub fn with_config(mut self, config: RegistryConfig) -> Self {
        self.config = config;
        self
    }

    /// Inject a clock — tests pass a [`clock::TestClock`] they advance manually.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The suggested background sweep interval (seconds).
    pub fn sweep_interval_secs(&self) -> u64 {
        self.config.sweep_interval_secs
    }

    /// The currently announced servers (URLs only).
    pub fn servers(&self) -> Vec<String> {
        self.servers.lock().unwrap().keys().cloned().collect()
    }

    /// Number of stored relay lists (test/observability helper).
    pub fn relay_count(&self) -> usize {
        self.relays.lock().unwrap().len()
    }

    /// Fetch a server's well-known and, if it is a conformant Freedback server,
    /// return its (optional) published key. `Err` means "not verifiable".
    async fn fetch_well_known(&self, base: &str) -> Result<Option<String>, String> {
        let well_known = format!("{base}/.well-known/freedback");
        let resp = self
            .http
            .get(&well_known)
            .send()
            .await
            .map_err(|e| format!("could not fetch well-known: {e}"))?;
        if !resp.status().is_success() {
            return Err("well-known returned a non-success status".to_string());
        }
        let doc: Value = resp
            .json()
            .await
            .map_err(|_| "well-known is not valid JSON".to_string())?;
        if doc.get("protocol").and_then(Value::as_str) != Some("freedback/1") {
            return Err("well-known does not advertise protocol freedback/1".to_string());
        }
        Ok(doc
            .get("key")
            .and_then(Value::as_str)
            .map(|s| s.to_string()))
    }

    /// Re-verify every announced server and drop the ones that fail or whose
    /// last verification has aged past the TTL. Returns the URLs removed.
    ///
    /// This is the explicit liveness entry point (issue #25 part 1): the binary
    /// calls it on an interval; tests call it directly after advancing the
    /// injected clock, so there is no wall-clock dependency.
    pub async fn sweep(&self) -> Vec<String> {
        let now = self.clock.now_unix();
        let ttl = self.config.server_ttl_secs;

        // Snapshot the current URLs so we don't hold the lock across awaits.
        let urls: Vec<String> = self.servers.lock().unwrap().keys().cloned().collect();

        let mut removed = Vec::new();
        for url in urls {
            match self.fetch_well_known(&url).await {
                Ok(key) => {
                    // Reachable and conformant: refresh its verification stamp.
                    let mut servers = self.servers.lock().unwrap();
                    if let Some(entry) = servers.get_mut(&url) {
                        entry.last_verified = now;
                        entry.key_pem = key;
                    }
                }
                Err(_) => {
                    // Unreachable / non-conformant: drop only once it is also
                    // past its TTL grace window, so a transient blip does not
                    // immediately evict a server verified moments ago.
                    let mut servers = self.servers.lock().unwrap();
                    let expired = servers
                        .get(&url)
                        .map(|e| now.saturating_sub(e.last_verified) >= ttl)
                        .unwrap_or(false);
                    if expired {
                        servers.remove(&url);
                        removed.push(url);
                    }
                }
            }
        }
        removed
    }

    /// Ingest a relay list: verify its signature + issuer/key binding, then
    /// store it if newer than any existing record for the issuer. Returns
    /// `Ok(true)` if stored, `Ok(false)` if a not-newer record was kept,
    /// `Err` if verification failed. Shared by `POST /relays` and gossip.
    pub fn ingest_relay_list(&self, list: RelayList) -> Result<bool, String> {
        list.verify()?;
        let mut relays = self.relays.lock().unwrap();
        if let Some(existing) = relays.get(&list.issuer) {
            if existing.updated >= list.updated {
                return Ok(false);
            }
        }
        relays.insert(list.issuer.clone(), list);
        Ok(true)
    }

    /// Snapshot all stored relay lists (for gossip).
    pub fn relay_lists(&self) -> Vec<RelayList> {
        self.relays.lock().unwrap().values().cloned().collect()
    }

    /// Push every stored relay list to a peer registry's `POST /relays` (issue
    /// #25 part 3). The peer re-verifies each signature before storing, so this
    /// is safe to relay untrusted. Returns the number the peer accepted as
    /// newly stored.
    pub async fn gossip_relays_to(&self, peer_base: &str) -> usize {
        let peer = normalize(peer_base);
        let mut accepted = 0;
        for list in self.relay_lists() {
            let resp = self
                .http
                .post(format!("{peer}/relays"))
                .json(&list)
                .send()
                .await;
            if let Ok(resp) = resp {
                if let Ok(body) = resp.json::<Value>().await {
                    if body.get("stored").and_then(Value::as_bool) == Some(true) {
                        accepted += 1;
                    }
                }
            }
        }
        accepted
    }
}

/// Build the discovery-server router.
pub fn build_app(state: AppState) -> Router {
    Router::new()
        // A human-clickable index at the bare hostname (else: 404 on click).
        .route("/", get(root))
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

/// The canonical bytes an announce signs over: the announced `url` (normalized).
///
/// Kept deliberately small and stable so the proof reads "I, holder of this
/// key, vouch for this URL". The detached signature lives outside these bytes.
fn announce_signing_bytes(url: &str) -> Result<Vec<u8>, String> {
    canonical_json(&json!({ "url": normalize(url) })).map_err(|e| e.to_string())
}

/// Build a detached ES256 announce signature for `url` using `id`. Single
/// source of truth for the signed-announce byte definition (used by clients and
/// tests).
pub fn sign_announce(id: &Identity, url: &str) -> Result<Signature, String> {
    let bytes = announce_signing_bytes(url)?;
    Ok(Signature {
        alg: "ES256".to_string(),
        kid: id.public_key_pem().map_err(|e| e.to_string())?,
        sig: id.sign_es256(&bytes),
    })
}

#[derive(Deserialize)]
struct Announce {
    url: String,
    /// Optional detached ES256 self-signature proving control of the server's
    /// key. When present, the registry requires the well-known to publish a
    /// matching `"key"`. Absent → legacy well-known-only authentication.
    #[serde(default)]
    signature: Option<Signature>,
}

/// `POST /announce {url, signature?}` — verify the announced server then record
/// it.
///
/// The well-known fetch is always required (corroboration). If a `signature` is
/// present, the registry additionally verifies it is a valid ES256 signature
/// over the announced URL and that the signing key matches the key the server
/// publishes in its well-known — proving the announcer controls that key.
async fn announce(
    State(state): State<AppState>,
    Json(body): Json<Announce>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let base = normalize(&body.url);

    let reject = |msg: &str| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": msg })),
        )
    };

    // Corroboration: the server must serve a conformant well-known.
    let published_key = state
        .fetch_well_known(&base)
        .await
        .map_err(|e| reject(&e))?;

    // Signed announce: prove control of the published key.
    let mut signed = false;
    if let Some(sig) = &body.signature {
        if sig.alg != "ES256" {
            return Err(reject("announce signature alg must be ES256"));
        }
        let key = published_key
            .as_ref()
            .ok_or_else(|| reject("signed announce but server publishes no key"))?;
        // Bind the signing key to the published key: they must be the same key.
        let derived_sig = issuer_id_from_pem(&sig.kid).map_err(|_| reject("bad signature kid"))?;
        let derived_pub =
            issuer_id_from_pem(key).map_err(|_| reject("server published an invalid key"))?;
        if derived_sig != derived_pub {
            return Err(reject(
                "announce signature key does not match published key",
            ));
        }
        let bytes = announce_signing_bytes(&base).map_err(|_| reject("could not canonicalize"))?;
        verify_es256(&sig.kid, &bytes, &sig.sig)
            .map_err(|_| reject("announce signature is invalid"))?;
        signed = true;
    }

    let now = state.clock.now_unix();
    state.servers.lock().unwrap().insert(
        base.clone(),
        ServerEntry {
            last_verified: now,
            key_pem: published_key,
        },
    );
    Ok(Json(
        json!({ "ok": true, "server": base, "signed": signed }),
    ))
}

/// `GET /servers` — the flat list of announced (live) servers.
async fn servers(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "servers": state.servers() }))
}

/// `POST /relays` — publish a self-signed NIP-65-style relay list.
///
/// The registry verifies the issuer's signature (and the issuer/key binding)
/// before storing; it never takes an issuer's claimed servers on faith. The
/// record is replaceable: a newer `updated` supersedes an older one. The same
/// path ingests gossiped lists from peer registries (verify-before-store).
async fn post_relays(
    State(state): State<AppState>,
    Json(list): Json<RelayList>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let issuer = list.issuer.clone();
    match state.ingest_relay_list(list) {
        Ok(stored) => Ok(Json(json!({
            "ok": true,
            "issuer": issuer,
            "stored": stored,
            "reason": if stored { Value::Null } else { json!("not newer") },
        }))),
        Err(msg) => Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": msg })),
        )),
    }
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

/// `GET /` — a tiny human-clickable index (else a bare-hostname click 404s).
async fn root(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "name": "freedback-discovery-server",
        "version": env!("CARGO_PKG_VERSION"),
        "servers": format!("{}/servers", state.base_url),
        "resolve": format!("{}/resolve?target=", state.base_url),
        "well_known": format!("{}/.well-known/freedback", state.base_url),
        "docs": "https://freedback.net/",
    }))
}

/// `GET /.well-known/freedback` — the registry's own self-description.
async fn well_known(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "freedback/1",
        "formats": ["application/ld+json"],
        "capabilities": ["discovery-registry", "relay-list", "relay-gossip", "signed-announce"],
        "conformsTo": "https://freedback.net/profile/1",
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
