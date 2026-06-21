//! Freedback feedback server (component 1).
//!
//! Implements the Web Annotation Protocol container semantics we need plus the
//! Freedback net-new `/sync` cursor and `/.well-known/freedback`. Exposed as a
//! library so integration tests (and the future `TestCluster`) can run the real
//! router in-process.

use std::collections::HashMap;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use freedback_protocol::Validator;
use freedback_storage::FeedbackStore;
use tower_http::trace::TraceLayer;

pub mod auth;
pub mod collection;
pub mod error;
pub mod handlers;

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
        }
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
            post(handlers::post_annotations).get(handlers::get_collection),
        )
        .route("/annotations/{id}", get(handlers::get_one))
        .route("/sync", get(handlers::get_sync))
        .route("/.well-known/freedback", get(handlers::well_known))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
