//! Storage abstraction for Freedback (INVARIANT 6).
//!
//! The [`FeedbackStore`] trait is the single seam between the servers and
//! persistence. The in-memory [`MemoryStore`] is the fast, deterministic test
//! mock; the Oxigraph backend (behind the `oxigraph` feature) is the primary
//! production store. Both must satisfy the same semantics, exercised by the
//! shared [`conformance`] suite.

use async_trait::async_trait;
use freedback_protocol::{dedup_id, Annotation};
use thiserror::Error;

pub mod memory;
pub use memory::MemoryStore;

#[cfg(feature = "oxigraph")]
pub mod oxigraph_store;
#[cfg(feature = "oxigraph")]
pub use oxigraph_store::OxigraphStore;

#[cfg(feature = "sqlite")]
pub mod sqlite_store;
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;

/// Storage errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A protocol-level error (canonicalization, etc.).
    #[error(transparent)]
    Protocol(#[from] freedback_protocol::Error),
    /// JSON (de)serialization of a stored annotation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A `put` was rejected because the dedup id is tombstoned (erased — ADR
    /// 0021). Deleted content must not resurrect through re-POSTs, gossip, or
    /// reconciliation; the server maps this to `410 Gone`.
    #[error("annotation {0} was deleted (tombstoned)")]
    Tombstoned(String),
    /// A backend-specific failure.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Result alias for storage operations.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Outcome of a [`FeedbackStore::put`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    /// The content-addressed dedup id of the stored annotation.
    pub dedup_id: String,
    /// `true` if newly inserted, `false` if it already existed (idempotent).
    pub created: bool,
}

/// A query over stored annotations.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Restrict to annotations whose target source equals this IRI.
    pub target: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Page size (items per page). `0` is treated as "all".
    pub page_size: usize,
}

/// A content-free deletion marker (ADR 0021 — right to erasure).
///
/// On deletion the annotation's content (body, target, creator, timestamps) is
/// **removed**; only this marker remains so the erasure itself can propagate to
/// sync consumers and so the dedup id cannot be re-`put`. `proof` is the delete
/// document that authorized the erasure (`{type, annotation, created}` +
/// optional detached signature) — it carries no feedback content and no
/// personal data beyond the issuer's already-public key.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tombstone {
    /// The erased annotation's content-addressed dedup id.
    pub dedup_id: String,
    /// Unix timestamp of the deletion — the tombstone feed's cursor position.
    pub deleted_at: i64,
    /// The authorizing delete document (content-free).
    pub proof: serde_json::Value,
}

/// A page of annotations plus collection metadata.
#[derive(Debug, Clone)]
pub struct Page {
    /// The annotations on this page (ordered by `iat` ascending, then dedup id).
    pub items: Vec<Annotation>,
    /// Total number of annotations matching the query (across all pages).
    pub total: usize,
    /// The page index echoed back.
    pub page: usize,
    /// The page size echoed back.
    pub page_size: usize,
}

/// Persistence for Freedback annotations.
#[async_trait]
pub trait FeedbackStore: Send + Sync {
    /// Insert an annotation. Idempotent by content-addressed dedup id.
    async fn put(&self, ann: &Annotation) -> Result<PutOutcome>;

    /// Fetch a single annotation by its dedup id.
    async fn get(&self, dedup_id: &str) -> Result<Option<Annotation>>;

    /// Paginated query, ordered by `iat` ascending then dedup id.
    async fn query(&self, q: &Query) -> Result<Page>;

    /// Incremental cursor read (Mangrove `getReviews` model): annotations with
    /// `iat > gt_iat`. When `latest_edits_only`, collapse edit chains to the
    /// latest annotation per `(issuer, target)`.
    async fn sync(
        &self,
        target: &str,
        gt_iat: i64,
        latest_edits_only: bool,
    ) -> Result<Vec<Annotation>>;

    /// Erase the annotation with this dedup id (ADR 0021), leaving a
    /// content-free [`Tombstone`] `{dedup_id, deleted_at, proof}` behind.
    ///
    /// Semantics (shared by every backend, exercised by the conformance suite):
    /// * the tombstone is recorded whether or not the annotation was present
    ///   (so an erasure can be replayed / arrive before the content it erases),
    ///   but an **existing** tombstone is never overwritten (first delete wins);
    /// * returns `true` iff an annotation was actually removed;
    /// * a subsequent [`put`](FeedbackStore::put) of the same dedup id fails
    ///   with [`StoreError::Tombstoned`].
    async fn delete(
        &self,
        dedup_id: &str,
        deleted_at: i64,
        proof: serde_json::Value,
    ) -> Result<bool>;

    /// Whether this dedup id has been erased (a tombstone exists).
    async fn is_tombstoned(&self, dedup_id: &str) -> Result<bool>;

    /// Tombstones with `deleted_at > gt_deleted_at`, ordered by `deleted_at`
    /// ascending then dedup id — the erasure propagation feed (`deleted_at` is
    /// the cursor position).
    async fn tombstones(&self, gt_deleted_at: i64) -> Result<Vec<Tombstone>>;

    /// Snapshot every stored annotation (one per line) followed by every
    /// tombstone (as `{"type":"Tombstone",...}` lines) to a JSON-Lines file.
    /// Backend-agnostic; used for durable "demo" persistence on top of the
    /// in-memory backends (see ADR 0008). Returns the number of annotations
    /// written (tombstone lines are additive and not counted, so pre-erasure
    /// callers see unchanged totals).
    async fn dump_jsonl(&self, path: &str) -> Result<usize> {
        use std::io::Write;
        let page = self.query(&Query::default()).await?;
        let mut f = std::fs::File::create(path).map_err(|e| StoreError::Backend(e.to_string()))?;
        for ann in &page.items {
            writeln!(f, "{}", serde_json::to_string(ann)?)
                .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        for t in self.tombstones(i64::MIN).await? {
            let mut v = serde_json::to_value(&t)?;
            v["type"] = serde_json::Value::String("Tombstone".into());
            writeln!(f, "{v}").map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        Ok(page.items.len())
    }

    /// Load a JSON-Lines snapshot, `put`-ing each annotation and re-recording
    /// each tombstone (both idempotent). Old snapshot files (annotations only)
    /// load unchanged; an annotation line whose dedup id is tombstoned is
    /// skipped rather than failing the load. A missing file is treated as
    /// empty. Returns the number of annotations newly inserted.
    async fn load_jsonl(&self, path: &str) -> Result<usize> {
        let data = match std::fs::read_to_string(path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(StoreError::Backend(e.to_string())),
        };
        let mut new = 0;
        for line in data.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line)?;
            if v.get("type").and_then(serde_json::Value::as_str) == Some("Tombstone") {
                let t: Tombstone = serde_json::from_value(v)?;
                self.delete(&t.dedup_id, t.deleted_at, t.proof).await?;
                continue;
            }
            let ann: Annotation = serde_json::from_value(v)?;
            match self.put(&ann).await {
                Ok(outcome) if outcome.created => new += 1,
                Ok(_) => {}
                Err(StoreError::Tombstoned(_)) => {} // erased content stays erased
                Err(e) => return Err(e),
            }
        }
        Ok(new)
    }
}

/// Internal record shape shared by backends.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub dedup_id: String,
    pub target: String,
    pub issuer: String,
    pub iat: i64,
    pub ann: Annotation,
}

impl Record {
    pub(crate) fn from_annotation(ann: &Annotation) -> Result<Self> {
        let dedup_id = dedup_id(ann)?;
        let target = ann.target.source().to_string();
        let issuer = ann
            .creator
            .as_ref()
            .map(|c| c.id.clone())
            // Anonymous annotations cannot have their edits collapsed; key them
            // by their own dedup id so they never merge with others.
            .unwrap_or_else(|| format!("anon:{dedup_id}"));
        let iat = ann.iat().unwrap_or(0);
        Ok(Self {
            dedup_id,
            target,
            issuer,
            iat,
            ann: ann.clone(),
        })
    }

    /// The grouping key used to collapse edit chains.
    pub(crate) fn edit_key(&self) -> (String, String) {
        (self.issuer.clone(), self.target.clone())
    }
}

/// Order records by `iat` ascending, breaking ties by dedup id for determinism.
pub(crate) fn order_records(records: &mut [Record]) {
    records.sort_by(|a, b| a.iat.cmp(&b.iat).then_with(|| a.dedup_id.cmp(&b.dedup_id)));
}

/// Order tombstones by `deleted_at` ascending, then dedup id (determinism).
pub(crate) fn order_tombstones(tombstones: &mut [Tombstone]) {
    tombstones.sort_by(|a, b| {
        a.deleted_at
            .cmp(&b.deleted_at)
            .then_with(|| a.dedup_id.cmp(&b.dedup_id))
    });
}

/// Collapse to the latest record per `(issuer, target)` (highest `iat`, then
/// highest dedup id for a deterministic tiebreak).
pub(crate) fn latest_edits(records: Vec<Record>) -> Vec<Record> {
    use std::collections::HashMap;
    let mut latest: HashMap<(String, String), Record> = HashMap::new();
    for r in records {
        let key = r.edit_key();
        match latest.get(&key) {
            Some(existing) if (existing.iat, &existing.dedup_id) >= (r.iat, &r.dedup_id) => {}
            _ => {
                latest.insert(key, r);
            }
        }
    }
    let mut out: Vec<Record> = latest.into_values().collect();
    order_records(&mut out);
    out
}

#[cfg(test)]
mod conformance;
