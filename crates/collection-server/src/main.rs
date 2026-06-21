//! Collection-server binary.
//!
//! Config: `FREEDBACK_BIND` (default `127.0.0.1:8100`), `FREEDBACK_BASE_URL`,
//! `FREEDBACK_SERVERS` (comma-separated upstream feedback-server base URLs).

use freedback_collection_server::{build_app, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let bind = std::env::var("FREEDBACK_BIND").unwrap_or_else(|_| "127.0.0.1:8100".into());
    let base_url = std::env::var("FREEDBACK_BASE_URL").unwrap_or_else(|_| format!("http://{bind}"));

    let state = AppState::new(base_url.clone());
    if let Ok(servers) = std::env::var("FREEDBACK_SERVERS") {
        for s in servers.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            state.add_server(s);
        }
    }

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("collection-server listening on {bind} (base {base_url})");
    axum::serve(listener, app).await?;
    Ok(())
}
