//! Freedback feedback server (component 1).
//!
//! Implements the Web Annotation Protocol container semantics we need plus the
//! Freedback net-new `/sync` cursor and `/.well-known/freedback`. Exposed as a
//! library so integration tests (and the future `TestCluster`) can run the real
//! router in-process.

use std::collections::HashMap;
use std::sync::Arc;

use axum::routing::{get, post, put};
use axum::Router;
use freedback_protocol::Validator;
use freedback_storage::FeedbackStore;
use tower_http::trace::TraceLayer;

pub mod auth;
pub mod collection;
pub mod error;
pub mod handlers;
pub mod httpdate;

pub use auth::OAuth;
pub use error::ApiError;

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    /// The backing store.
    pub store: Arc<dyn FeedbackStore>,
    /// SHACL validator (shapes loaded once).
    pub validator: Arc<Validator>,
    /// Public base URL used to mint annotation ids and `partOf`/page links.
    pub base_url: String,
    /// OAuth bearer-token → `(app_id, user_id)` map (the non-federating identity).
    pub oauth: Arc<OAuth>,
    /// Default page size for collection reads.
    pub page_size: usize,
    /// `max-age` (seconds) advertised in the collection `Cache-Control` header,
    /// so a polite aggregator can serve from its cache without revalidating
    /// while the page is still fresh.
    pub cache_max_age: u64,
    /// Optional server-identity public key (P-256 SPKI PEM). When set it is
    /// published in `/.well-known/freedback` as `"key"`, letting a discovery
    /// registry corroborate a **signed announce** (the announce signature's key
    /// must match this published key). `None` keeps the legacy behavior where
    /// announce is authenticated by the well-known fetch alone.
    pub server_key_pem: Option<String>,
    /// When set, wrap the router in a permissive CORS layer so browser widgets
    /// served from a different origin can read and publish (the real
    /// cross-origin widget scenario). Off by default to keep server behavior
    /// unchanged; the binary enables it via `FREEDBACK_CORS_PERMISSIVE` and the
    /// widgets headless-browser E2E harness sets it.
    pub cors_permissive: bool,
}

impl AppState {
    /// Build state with sensible defaults around a store.
    pub fn new(store: Arc<dyn FeedbackStore>, base_url: impl Into<String>) -> Self {
        Self {
            store,
            validator: Arc::new(Validator::default()),
            base_url: base_url.into(),
            oauth: Arc::new(OAuth::default()),
            page_size: 50,
            cache_max_age: 30,
            server_key_pem: None,
            cors_permissive: false,
        }
    }

    /// Enable a permissive CORS layer (cross-origin browser widgets).
    pub fn with_cors_permissive(mut self, on: bool) -> Self {
        self.cors_permissive = on;
        self
    }

    /// Publish a server-identity public key (P-256 SPKI PEM) in the well-known,
    /// enabling signed-announce corroboration at a discovery registry.
    pub fn with_server_key_pem(mut self, pem: impl Into<String>) -> Self {
        self.server_key_pem = Some(pem.into());
        self
    }

    /// Override the collection `Cache-Control: max-age` (builder style).
    pub fn with_cache_max_age(mut self, secs: u64) -> Self {
        self.cache_max_age = secs;
        self
    }

    /// Replace the OAuth token map (builder style).
    pub fn with_oauth(mut self, tokens: HashMap<String, (String, String)>) -> Self {
        self.oauth = Arc::new(OAuth::new(tokens));
        self
    }
}

/// Build the axum router for the feedback server.
pub fn build_app(state: AppState) -> Router {
    let cors_permissive = state.cors_permissive;
    let router = Router::new()
        .route(
            "/annotations/",
            post(handlers::post_annotations).get(handlers::get_collection),
        )
        .route("/annotations/{id}", get(handlers::get_one))
        .route("/submit/{jwt}", put(handlers::submit))
        .route("/sync", get(handlers::get_sync))
        .route("/.well-known/freedback", get(handlers::well_known))
        .layer(TraceLayer::new_for_http());

    let router = if cors_permissive {
        router.layer(permissive_cors())
    } else {
        router
    };

    router.with_state(state)
}

/// A permissive CORS layer for cross-origin browser widgets: any origin, the
/// methods/headers the widgets use (GET/POST + `content-type`/`authorization`),
/// and the conditional-read response headers an aggregator/widget inspects.
fn permissive_cors() -> tower_http::cors::CorsLayer {
    use axum::http::{header, Method};
    tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
        .expose_headers([header::ETAG, header::LAST_MODIFIED, header::LINK])
}
