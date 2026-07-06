//! Freedback mobile shell: a thin Tauri 2 command layer over
//! [`freedback_app_core::AppCore`]. NO logic lives here — every command
//! delegates to the host-testable core crate.
//!
//! ## The share bridge (Android)
//!
//! `ShareActivity` (see `gen/android`) receives `ACTION_SEND` /
//! `ACTION_PROCESS_TEXT`, rewrites the text as a
//! `freedback://share?text=<urlencoded>` VIEW intent at `MainActivity`, and
//! finishes. `tauri-plugin-deep-link` delivers the URI here; the handler
//! stores the decoded text as the pending share (drained by the
//! `take_pending_share` command) and emits a `share` event to the webview.

use std::sync::Arc;

use freedback_app_core::{AppCore, Contribution, FeedbackView, JournalEntry, Resolved, Settings};
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

type Core = Arc<AppCore>;

/// Map any core error into the string the webview shows.
fn ui_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run a core async operation to completion on a blocking thread.
///
/// The protocol client's `Transport` trait is `async_trait(?Send)` (the same
/// code path drives browser futures on wasm), so core futures are not `Send`
/// and cannot be awaited directly inside a Tauri async command. A one-shot
/// current-thread runtime on `spawn_blocking` keeps the IPC threads free
/// without touching the shared crates.
async fn run_core<T, Fut, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = freedback_app_core::Result<T>>,
{
    tauri::async_runtime::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(f())
            .map_err(ui_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn resolve_input(core: tauri::State<'_, Core>, input: String) -> Result<Resolved, String> {
    core.resolve_input(&input).map_err(ui_err)
}

#[tauri::command]
async fn get_feedback(
    core: tauri::State<'_, Core>,
    target: String,
) -> Result<FeedbackView, String> {
    let core = core.inner().clone();
    run_core(move || async move { core.get_feedback(&target).await }).await
}

#[tauri::command]
async fn publish(
    core: tauri::State<'_, Core>,
    target: String,
    contribution: Contribution,
    license: Option<String>,
) -> Result<JournalEntry, String> {
    let core = core.inner().clone();
    run_core(move || async move { core.publish(&target, contribution, license).await }).await
}

#[tauri::command]
fn my_feedback(core: tauri::State<'_, Core>) -> Result<Vec<JournalEntry>, String> {
    core.my_feedback().map_err(ui_err)
}

#[tauri::command]
async fn update_entry(
    core: tauri::State<'_, Core>,
    dedup_id: String,
    contribution: Contribution,
    license: Option<String>,
) -> Result<JournalEntry, String> {
    let core = core.inner().clone();
    run_core(move || async move { core.update_entry(&dedup_id, contribution, license).await }).await
}

#[tauri::command]
async fn erase_entry(
    core: tauri::State<'_, Core>,
    dedup_id: String,
) -> Result<JournalEntry, String> {
    let core = core.inner().clone();
    run_core(move || async move { core.erase_entry(&dedup_id).await }).await
}

#[tauri::command]
fn export_identity(core: tauri::State<'_, Core>) -> Result<String, String> {
    core.export_identity().map_err(ui_err)
}

#[tauri::command]
fn import_identity(core: tauri::State<'_, Core>, pem: String) -> Result<String, String> {
    core.import_identity(&pem).map_err(ui_err)
}

#[tauri::command]
fn export_identity_qr(core: tauri::State<'_, Core>) -> Result<String, String> {
    core.export_identity_qr().map_err(ui_err)
}

#[tauri::command]
fn get_settings(core: tauri::State<'_, Core>) -> Result<Settings, String> {
    Ok(core.settings())
}

#[tauri::command]
fn should_nudge_key_backup(core: tauri::State<'_, Core>) -> Result<bool, String> {
    core.should_nudge_key_backup().map_err(ui_err)
}

/// Whether camera scanning is available: `tauri-plugin-barcode-scanner` is
/// mobile-only (its crate root is `#![cfg(mobile)]`), so the desktop build —
/// the primary e2e tier — never has it. The UI keeps the Scan button
/// disabled unless this is true.
#[tauri::command]
fn scanning_supported() -> bool {
    cfg!(mobile)
}

#[tauri::command]
fn set_settings(core: tauri::State<'_, Core>, server_url: String) -> Result<Settings, String> {
    core.set_server_url(server_url).map_err(ui_err)
}

/// Return-and-clear the pending shared text delivered via deep link. The
/// webview calls this on startup (the link may predate its listeners) and
/// whenever a `share` event fires.
#[tauri::command]
fn take_pending_share(core: tauri::State<'_, Core>) -> Result<Option<String>, String> {
    Ok(core.take_pending_share())
}

/// Feed one opened deep-link URL into the pending-share state + `share` event.
fn handle_deep_link(app: &tauri::AppHandle, url: &str) {
    let Some(text) = freedback_app_core::share::extract_share_text(url) else {
        return;
    };
    let core = app.state::<Core>();
    core.set_pending_share(text.clone());
    // Losing the event is fine: take_pending_share drains on startup.
    let _ = app.emit("share", text);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_deep_link::init());
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());

    builder
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let core: Core = Arc::new(AppCore::open(data_dir)?);
            app.manage(core);

            // Deep links opened while running; the initial launch URL is also
            // replayed through this handler by the plugin on mobile.
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    handle_deep_link(&handle, url.as_str());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            resolve_input,
            get_feedback,
            publish,
            my_feedback,
            update_entry,
            erase_entry,
            export_identity,
            import_identity,
            export_identity_qr,
            get_settings,
            set_settings,
            should_nudge_key_backup,
            scanning_supported,
            take_pending_share,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Freedback app");
}
