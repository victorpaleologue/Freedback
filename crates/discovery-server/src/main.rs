//! Discovery-server binary.
//!
//! Config: `FREEDBACK_BIND` (default `127.0.0.1:8090`), `FREEDBACK_BASE_URL`,
//! `FREEDBACK_SERVER_TTL_SECS` (default 3600), `FREEDBACK_SWEEP_INTERVAL_SECS`
//! (default 300) — the liveness/expiry tunables (issue #25 part 1).

use freedback_discovery_server::{build_app, AppState, RegistryConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let bind = std::env::var("FREEDBACK_BIND").unwrap_or_else(|_| "127.0.0.1:8090".into());
    let base_url = std::env::var("FREEDBACK_BASE_URL").unwrap_or_else(|_| format!("http://{bind}"));

    let mut config = RegistryConfig::default();
    if let Ok(v) = std::env::var("FREEDBACK_SERVER_TTL_SECS") {
        if let Ok(n) = v.parse() {
            config.server_ttl_secs = n;
        }
    }
    if let Ok(v) = std::env::var("FREEDBACK_SWEEP_INTERVAL_SECS") {
        if let Ok(n) = v.parse() {
            config.sweep_interval_secs = n;
        }
    }

    let state = AppState::new(base_url.clone()).with_config(config);

    // Background liveness sweep: periodically re-verify announced servers and
    // drop the stale/unreachable ones (issue #25 part 1).
    let sweep_state = state.clone();
    let interval = std::time::Duration::from_secs(sweep_state.sweep_interval_secs().max(1));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            let removed = sweep_state.sweep().await;
            if !removed.is_empty() {
                tracing::info!("liveness sweep removed {} stale server(s)", removed.len());
            }
        }
    });

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("discovery-server listening on {bind} (base {base_url})");
    axum::serve(listener, app).await?;
    Ok(())
}
