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
        }
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
    Router::new()
        .route(
            "/annotations/",
            // GET (and HEAD, auto-derived by axum) reads the paged collection;
            // POST writes; OPTIONS advertises the container's methods/`Allow`.
            post(handlers::post_annotations)
                .get(handlers::get_collection)
                .options(handlers::options_container),
        )
        .route("/annotations/{id}", get(handlers::get_one))
        // Freedback-annotation JWT (payload = our annotation, ADR 0010).
        .route("/submit/{jwt}", put(handlers::submit))
        // Mangrove review-schema JWT (sub/rating/opinion → annotation).
        .route("/submit/mangrove/{jwt}", put(handlers::submit_mangrove))
        .route("/sync", get(handlers::get_sync))
        .route("/.well-known/freedback", get(handlers::well_known))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
