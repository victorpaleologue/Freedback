//! Feedback-server binary.
//!
//! Config via env:
//! - `FREEDBACK_BIND`       (default `127.0.0.1:8080`)
//! - `FREEDBACK_BASE_URL`   (default `http://<bind>`)
//! - `FREEDBACK_STORE_PATH` (optional JSON-Lines snapshot file: loaded on boot,
//!   re-snapshotted every 60s and on graceful shutdown — durable demo storage)
//! - `FREEDBACK_OAUTH_TOKEN` + `FREEDBACK_OAUTH_APP` + `FREEDBACK_OAUTH_USER`
//!   (optional single demo bearer token → app-scoped identity)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use freedback_feedback_server::{build_app, AppState};
use freedback_storage::{FeedbackStore, OxigraphStore};

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
    let store_path = std::env::var("FREEDBACK_STORE_PATH").ok();

    let store: Arc<dyn FeedbackStore> = Arc::new(OxigraphStore::new()?);

    // Durable demo storage: load the snapshot on boot (see ADR 0008).
    if let Some(path) = &store_path {
        match store.load_jsonl(path).await {
            Ok(n) => tracing::info!("loaded {n} annotations from {path}"),
            Err(e) => tracing::warn!("could not load snapshot {path}: {e}"),
        }
    }

    let mut state = AppState::new(store.clone(), base_url.clone());

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

    // Periodic snapshots while running.
    if let Some(path) = store_path.clone() {
        let s = store.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(e) = s.dump_jsonl(&path).await {
                    tracing::warn!("periodic snapshot failed: {e}");
                }
            }
        });
    }

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("feedback-server listening on {bind} (base {base_url})");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Final snapshot on shutdown.
    if let Some(path) = &store_path {
        match store.dump_jsonl(path).await {
            Ok(n) => tracing::info!("snapshotted {n} annotations to {path} on shutdown"),
            Err(e) => tracing::warn!("shutdown snapshot failed: {e}"),
        }
    }
    Ok(())
}

/// Resolve when the process receives Ctrl-C or (on Unix) SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
