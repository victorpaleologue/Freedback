//! Feedback-server binary.
//!
//! Config via env:
//! - `FREEDBACK_BIND`       (default `127.0.0.1:8080`)
//! - `FREEDBACK_BASE_URL`   (default `http://<bind>`)
//! - `FREEDBACK_STORE_PATH` (optional JSON-Lines snapshot file: loaded on boot,
//!   re-snapshotted every 60s and on graceful shutdown — durable demo storage
//!   for the in-memory backend)
//! - `FREEDBACK_ROCKSDB_PATH` (optional, **requires the `rocksdb` feature**: a
//!   durable on-disk Oxigraph/RocksDB store directory. When set, writes persist
//!   directly and the JSON-Lines snapshot loop is skipped.)
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
    let rocksdb_path = std::env::var("FREEDBACK_ROCKSDB_PATH").ok();

    // Pick the backend. A durable RocksDB store (when built with the feature and
    // given a path) persists writes directly; otherwise the in-memory store with
    // the optional JSON-Lines snapshot.
    let (store, durable) = build_store(&rocksdb_path)?;

    // JSON-Lines snapshots apply only to the in-memory backend (ADR 0008); a
    // durable RocksDB store already persists every write.
    if !durable {
        if let Some(path) = &store_path {
            match store.load_jsonl(path).await {
                Ok(n) => tracing::info!("loaded {n} annotations from {path}"),
                Err(e) => tracing::warn!("could not load snapshot {path}: {e}"),
            }
        }
    }

    let mut state = AppState::new(store.clone(), base_url.clone());

    // Permissive CORS for cross-origin browser widgets (off unless asked).
    if env_flag("FREEDBACK_CORS_PERMISSIVE") {
        state = state.with_cors_permissive(true);
    }

    // Optionally override the collection `Cache-Control: max-age` (seconds). An
    // aggregator can be told to always revalidate with `0`, which the widgets
    // E2E uses for a deterministic publish→read-back through the collection.
    if let Ok(secs) = std::env::var("FREEDBACK_CACHE_MAX_AGE") {
        if let Ok(secs) = secs.parse::<u64>() {
            state = state.with_cache_max_age(secs);
        }
    }

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

    // Periodic snapshots while running (in-memory backend only).
    if !durable {
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
    }

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("feedback-server listening on {bind} (base {base_url})");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Final snapshot on shutdown (in-memory backend only).
    if !durable {
        if let Some(path) = &store_path {
            match store.dump_jsonl(path).await {
                Ok(n) => tracing::info!("snapshotted {n} annotations to {path} on shutdown"),
                Err(e) => tracing::warn!("shutdown snapshot failed: {e}"),
            }
        }
    }
    Ok(())
}

/// Build the feedback store, returning `(store, durable)`. `durable == true`
/// only for the on-disk RocksDB backend, which makes the JSON-Lines snapshot
/// path moot. Selecting RocksDB needs both the `rocksdb` feature at build time
/// and `FREEDBACK_ROCKSDB_PATH` at run time.
fn build_store(rocksdb_path: &Option<String>) -> anyhow::Result<(Arc<dyn FeedbackStore>, bool)> {
    #[cfg(feature = "rocksdb")]
    if let Some(path) = rocksdb_path {
        tracing::info!("durable RocksDB store at {path}");
        return Ok((Arc::new(OxigraphStore::open(path)?), true));
    }
    #[cfg(not(feature = "rocksdb"))]
    if rocksdb_path.is_some() {
        tracing::warn!(
            "FREEDBACK_ROCKSDB_PATH is set but this build lacks the `rocksdb` feature; \
             using the in-memory store (rebuild with --features rocksdb for durability)"
        );
    }
    Ok((Arc::new(OxigraphStore::new()?), false))
}

/// A truthy env flag: set and not one of `0`/`false`/`no`/`off`/empty.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
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
