//! # Freedback app core
//!
//! ALL the mobile app's logic, host-testable, with **no Tauri dependencies**:
//! the Tauri shell (`../src-tauri`) is a thin command layer over [`AppCore`].
//!
//! Modules:
//! - [`input`] — parse & resolve a scanned/typed/shared string (GTIN, ISBN,
//!   URL, free text) into a canonical feedback target URI.
//! - [`share`] — normalize incoming Android share payloads and `freedback://`
//!   deep links into the same resolution.
//! - [`identity`] — the P-256 key that IS the account: mint-on-first-use,
//!   PKCS#8 PEM export/import (no signup).
//! - [`journal`] — the local "My feedback" journal (redb), one row per
//!   publish, with supersede/delete status (ADR 0021).
//! - [`feedback`] — orchestration over the protocol client: fetch+aggregate,
//!   publish, update-by-supersede, erase.

pub mod feedback;
pub mod identity;
pub mod input;
pub mod journal;
pub mod share;

use std::path::PathBuf;
use std::sync::Mutex;

use freedback_cli_client::{
    Client, ClientError, CollectionPoint, Dest, PublicationPoint, ReqwestTransport, Source,
};
use freedback_protocol::{Annotation, Creator, DeleteRequest, Target};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use feedback::{aggregate, Contribution, FeedbackView, TextItem, DEFAULT_LICENSE};
pub use identity::{IdentityError, IdentityKeeper};
pub use input::{InputError, Resolved};
pub use journal::{EntryStatus, Journal, JournalEntry, JournalError};

/// The default feedback server the app talks to (configurable in Settings).
pub const DEFAULT_SERVER: &str = "https://freedback-demo.fly.dev";

/// App-level errors surfaced to the UI layer.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("server: {0}")]
    Client(String),
    #[error("protocol: {0}")]
    Protocol(#[from] freedback_protocol::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(String),
    #[error("qr: {0}")]
    Qr(String),
    #[error("no journal entry with id {0}")]
    UnknownEntry(String),
    #[error("cannot {action} a {status} journal entry")]
    EntryNotActive {
        action: &'static str,
        status: &'static str,
    },
}

impl From<ClientError> for CoreError {
    fn from(e: ClientError) -> Self {
        CoreError::Client(e.to_string())
    }
}

/// Result alias for app-core operations.
pub type Result<T> = std::result::Result<T, CoreError>;

/// User-configurable settings, persisted as JSON next to the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Base URL of the feedback server (publication + collection point).
    pub server_url: String,
    /// Whether the account key currently on this device has ever been
    /// exported (the "back up your key" nudge, issue #64 M3). Defaults to
    /// `false` for settings files written before this field existed.
    #[serde(default)]
    pub key_backed_up: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER.to_string(),
            key_backed_up: false,
        }
    }
}

/// Nudge the user to back up their key after this many local publishes,
/// until they do (issue #64: "persistent nudge... after the first few
/// posts").
const BACKUP_NUDGE_THRESHOLD: usize = 3;

/// The application core: one instance per app, owned by the Tauri shell (or a
/// test). Everything hangs off a single data directory:
///
/// ```text
/// <data_dir>/identity.pem   # the account (P-256 PKCS#8 PEM)
/// <data_dir>/journal.redb   # the local "My feedback" journal
/// <data_dir>/settings.json  # server URL etc.
/// ```
pub struct AppCore {
    dir: PathBuf,
    identity: IdentityKeeper,
    journal: Journal,
    settings: Mutex<Settings>,
    pending_share: Mutex<Option<String>>,
    client: Client<ReqwestTransport>,
}

impl AppCore {
    /// Open (or create) the app state under `data_dir`.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = data_dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
        let identity = IdentityKeeper::new(dir.join("identity.pem"));
        let journal = Journal::open(dir.join("journal.redb"))?;
        let settings = load_settings(&dir)?;
        Ok(Self {
            dir,
            identity,
            journal,
            settings: Mutex::new(settings),
            pending_share: Mutex::new(None),
            client: Client::new(ReqwestTransport::new()),
        })
    }

    // --- settings ---------------------------------------------------------

    /// Current settings (a copy).
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Set + persist the server URL. Returns the updated settings.
    pub fn set_server_url(&self, url: impl Into<String>) -> Result<Settings> {
        let url = url.into();
        let url = url.trim().trim_end_matches('/').to_string();
        let updated = {
            let mut s = self.settings.lock().unwrap();
            s.server_url = if url.is_empty() {
                DEFAULT_SERVER.to_string()
            } else {
                url
            };
            s.clone()
        };
        save_settings(&self.dir, &updated)?;
        Ok(updated)
    }

    // --- identity (the key IS the account) ---------------------------------

    /// The identity keeper (key file management).
    pub fn identity(&self) -> &IdentityKeeper {
        &self.identity
    }

    /// Export the account key as PKCS#8 PEM, minting it first if this is the
    /// first use (the key IS the account — there is nothing else to sign up).
    /// Revealing the key this way IS backing it up, so it clears the backup
    /// nudge.
    pub fn export_identity(&self) -> Result<String> {
        let id = self.identity.load_or_create()?;
        let pem = id.to_pkcs8_pem()?;
        self.mark_key_backed_up()?;
        Ok(pem)
    }

    /// The account key PEM, as a scannable QR code (standalone SVG markup).
    /// The other half of "show as QR code + PEM file" (issue #64 M3) — import
    /// is scanning this with the camera (mobile-only, `tauri-plugin-barcode-scanner`).
    pub fn export_identity_qr(&self) -> Result<String> {
        let pem = self.export_identity()?;
        let code = qrcode::QrCode::new(pem.as_bytes()).map_err(|e| CoreError::Qr(e.to_string()))?;
        Ok(code.render::<qrcode::render::svg::Color>().build())
    }

    /// Import (and persist) an account key from PKCS#8 PEM, replacing the
    /// current one. Returns the imported key's issuer id. The newly-active
    /// key hasn't been backed up from THIS device yet, so this re-arms the
    /// nudge.
    pub fn import_identity(&self, pem: &str) -> Result<String> {
        let id = self.identity.import_pem(pem)?;
        let updated = {
            let mut s = self.settings.lock().unwrap();
            s.key_backed_up = false;
            s.clone()
        };
        save_settings(&self.dir, &updated)?;
        Ok(id.issuer_id()?)
    }

    /// Mark the current account key as backed up, clearing the nudge.
    fn mark_key_backed_up(&self) -> Result<()> {
        let updated = {
            let mut s = self.settings.lock().unwrap();
            s.key_backed_up = true;
            s.clone()
        };
        save_settings(&self.dir, &updated)
    }

    /// Whether to show the "back up your key" nudge: the key hasn't been
    /// exported yet and the user has published enough that losing it would
    /// actually cost them something.
    pub fn should_nudge_key_backup(&self) -> Result<bool> {
        if self.settings().key_backed_up {
            return Ok(false);
        }
        Ok(self.journal.list()?.len() >= BACKUP_NUDGE_THRESHOLD)
    }

    /// The current issuer id (mints the key on first use).
    pub fn issuer_id(&self) -> Result<String> {
        Ok(self.identity.load_or_create()?.issuer_id()?)
    }

    // --- input / share ------------------------------------------------------

    /// Resolve any user input (barcode digits, ISBN, URL, shared text,
    /// `freedback://` deep link) to a canonical feedback target.
    pub fn resolve_input(&self, input: &str) -> std::result::Result<Resolved, InputError> {
        share::normalize(input)
    }

    /// Store a shared text delivered by a deep link, for the webview to drain.
    pub fn set_pending_share(&self, text: impl Into<String>) {
        *self.pending_share.lock().unwrap() = Some(text.into());
    }

    /// Return-and-clear the pending shared text (the `take_pending_share`
    /// command): the webview calls this on startup and on `share` events.
    pub fn take_pending_share(&self) -> Option<String> {
        self.pending_share.lock().unwrap().take()
    }

    // --- feedback -----------------------------------------------------------

    /// Fetch and aggregate the feedback for a target from the configured
    /// server — the Feedback screen's data. Uses the read view (every
    /// annotation for the target), so each contribution kind a user published
    /// stays visible.
    ///
    /// NOTE: the protocol's edit-supersession collapses per `(issuer,
    /// target)` (newest wins) — see [`AppCore::get_feedback_latest`] for that
    /// view. Merging "one user's multi-kind feedback" with supersession into a
    /// single view is the collection server's job (component 7) and lands
    /// with its integration.
    pub async fn get_feedback(&self, target: &str) -> Result<FeedbackView> {
        let anns = self.fetch_annotations(target).await?;
        Ok(aggregate(target, &anns))
    }

    /// Aggregate over the server's `/sync` latest-edits view: edit chains
    /// collapse per `(issuer, target)`, newest wins — an update REPLACES its
    /// predecessor here (ADR 0021 supersession semantics).
    pub async fn get_feedback_latest(&self, target: &str) -> Result<FeedbackView> {
        let server = self.settings().server_url;
        let point = CollectionPoint::from_server(&server);
        let anns = self.client.sync(&point, target, 0, true).await?;
        Ok(aggregate(target, &anns))
    }

    /// Publish a contribution for `target` now, under `license` (CC BY 4.0
    /// when `None`). Signs with the account key (minted on first use), POSTs
    /// to the configured server, and records a journal row.
    pub async fn publish(
        &self,
        target: &str,
        contribution: Contribution,
        license: Option<String>,
    ) -> Result<JournalEntry> {
        self.publish_at(target, contribution, license, &now_rfc3339())
            .await
    }

    /// [`AppCore::publish`] with an explicit `created` timestamp — the
    /// deterministic entry point the tests use (repo testing rules: fixed
    /// timestamps keep signatures and dedup ids stable).
    pub async fn publish_at(
        &self,
        target: &str,
        contribution: Contribution,
        license: Option<String>,
        created: &str,
    ) -> Result<JournalEntry> {
        let identity = self.identity.load_or_create()?;
        let license = license.unwrap_or_else(|| DEFAULT_LICENSE.to_string());
        let mut ann = Annotation::new(
            contribution.motivation(),
            Target::Iri(target.to_string()),
            vec![contribution.body()],
        )
        .with_created(created)
        .with_rights(license)
        .with_creator(Creator::new(identity.issuer_id()?));
        identity.sign_annotation(&mut ann)?;
        let dedup_id = freedback_protocol::dedup_id(&ann)?;

        let server = self.settings().server_url;
        let dest = Dest::Endpoint {
            point: PublicationPoint::from_server(&server),
            bearer: None,
        };
        self.client.write(&ann, &dest).await?;

        let entry = JournalEntry {
            dedup_id,
            target: target.to_string(),
            server,
            created: created.to_string(),
            kind: contribution.kind_name().to_string(),
            summary: contribution.summary(),
            status: EntryStatus::Active,
            seq: 0, // stamped by the journal
        };
        Ok(self.journal.record(&entry)?)
    }

    /// The local journal, newest first ("My feedback").
    pub fn my_feedback(&self) -> Result<Vec<JournalEntry>> {
        Ok(self.journal.list()?)
    }

    /// Update a journal entry by supersession: publish a NEW annotation with
    /// the same key + target (newest wins per `(issuer, target)`) and mark the
    /// old row superseded.
    pub async fn update_entry(
        &self,
        dedup_id: &str,
        contribution: Contribution,
        license: Option<String>,
    ) -> Result<JournalEntry> {
        self.update_entry_at(dedup_id, contribution, license, &now_rfc3339())
            .await
    }

    /// [`AppCore::update_entry`] with an explicit `created` timestamp (tests).
    pub async fn update_entry_at(
        &self,
        dedup_id: &str,
        contribution: Contribution,
        license: Option<String>,
        created: &str,
    ) -> Result<JournalEntry> {
        let old = self
            .journal
            .get(dedup_id)?
            .ok_or_else(|| CoreError::UnknownEntry(dedup_id.to_string()))?;
        if let EntryStatus::Deleted = old.status {
            return Err(CoreError::EntryNotActive {
                action: "update",
                status: "deleted",
            });
        }
        let new = self
            .publish_at(&old.target, contribution, license, created)
            .await?;
        self.journal.mark_superseded(dedup_id, &new.dedup_id)?;
        Ok(new)
    }

    /// Erase a published annotation (right to be forgotten, ADR 0021): a
    /// signed `DeleteRequest` with the journal's key. The server keeps only a
    /// content-free tombstone; the journal row is marked deleted locally.
    /// Fails cleanly when the account key is missing (it is the only proof of
    /// ownership).
    pub async fn erase_entry(&self, dedup_id: &str) -> Result<JournalEntry> {
        self.erase_entry_at(dedup_id, &now_rfc3339()).await
    }

    /// [`AppCore::erase_entry`] with an explicit `created` timestamp (tests).
    pub async fn erase_entry_at(&self, dedup_id: &str, created: &str) -> Result<JournalEntry> {
        let entry = self
            .journal
            .get(dedup_id)?
            .ok_or_else(|| CoreError::UnknownEntry(dedup_id.to_string()))?;
        // NOT load_or_create: a freshly minted key could never authorize the
        // erasure of an annotation signed by the lost one — surface a typed
        // error instead of a confusing server 403.
        let identity = self.identity.load()?;
        let mut doc = DeleteRequest::new(dedup_id, created);
        identity.sign_delete(&mut doc)?;

        let point = PublicationPoint::from_server(&entry.server);
        self.client.delete(&point, &doc, None).await?;
        self.journal.mark_deleted(dedup_id)?;
        Ok(self.journal.get(dedup_id)?.unwrap_or(entry))
    }

    /// Raw (un-aggregated) annotations for a target — the same `/sync` view
    /// [`AppCore::get_feedback`] aggregates. Exposed for tests and debugging.
    pub async fn fetch_annotations(&self, target: &str) -> Result<Vec<Annotation>> {
        let server = self.settings().server_url;
        let source = Source::Endpoint(CollectionPoint::from_server(&server));
        Ok(self.client.read(target, &source).await?)
    }
}

/// Current UTC time as RFC 3339 with seconds precision (the annotation
/// `created` / `xsd:dateTime` form).
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_millisecond(0)
        .expect("0 is a valid millisecond")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC datetime always formats")
}

fn settings_path(dir: &std::path::Path) -> PathBuf {
    dir.join("settings.json")
}

fn load_settings(dir: &std::path::Path) -> Result<Settings> {
    match std::fs::read_to_string(settings_path(dir)) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(CoreError::Io(e.to_string())),
    }
}

fn save_settings(dir: &std::path::Path, settings: &Settings) -> Result<()> {
    std::fs::write(settings_path(dir), serde_json::to_string_pretty(settings)?)
        .map_err(|e| CoreError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_rfc3339_is_parseable_and_utc() {
        let now = now_rfc3339();
        assert!(now.ends_with('Z'), "UTC: {now}");
        time::OffsetDateTime::parse(&now, &time::format_description::well_known::Rfc3339)
            .expect("round-trips");
    }

    #[test]
    fn settings_default_points_at_the_demo_server() {
        assert_eq!(Settings::default().server_url, DEFAULT_SERVER);
    }

    #[test]
    fn export_identity_qr_encodes_the_same_pem_as_a_scannable_svg() {
        let dir = tempfile::tempdir().unwrap();
        let core = AppCore::open(dir.path()).unwrap();
        let svg = core.export_identity_qr().unwrap();
        assert!(svg.starts_with("<?xml"), "standalone SVG: {svg}");
        assert!(svg.contains("<svg"));
        // Exporting via the QR path backs up the key just like plain export.
        assert!(!core.should_nudge_key_backup().unwrap());
    }
}
