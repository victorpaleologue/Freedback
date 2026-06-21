//! Feedback-server binary.
//!
//! Config via env:
//! - `FREEDBACK_BIND`      (default `127.0.0.1:8080`)
//! - `FREEDBACK_BASE_URL`  (default `http://<bind>`)
//! - `FREEDBACK_OAUTH_TOKEN` + `FREEDBACK_OAUTH_APP` + `FREEDBACK_OAUTH_USER`
//!   (optional single demo bearer token → app-scoped identity)

use std::collections::HashMap;
use std::sync::Arc;

use freedback_feedback_server::{build_app, AppState};
use freedback_storage::OxigraphStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let bind = std::env::var("FREEDBACK_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let base_url = std::env::var("FREEDBACK_BASE_URL").unwrap_or_else(|_| format!("http://{bind}"));

    let store = Arc::new(OxigraphStore::new()?);
    let mut state = AppState::new(store, base_url.clone());

    // Optional single demo OAuth token.
    if let (Ok(token), Ok(app), Ok(user)) = (
        std::env::var("FREEDBACK_OAUTH_TOKEN"),
        std::env::var("FREEDBACK_OAUTH_APP"),
        std::env::var("FREEDBACK_OAUTH_USER"),
    ) {
        let mut tokens = HashMap::new();
        tokens.insert(token, (app, user));
        state = state.with_oauth(tokens);
    }

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("feedback-server listening on {bind} (base {base_url})");
    axum::serve(listener, app).await?;
    Ok(())
}
