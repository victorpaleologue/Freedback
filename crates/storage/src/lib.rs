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

/// Storage errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A protocol-level error (canonicalization, etc.).
    #[error(transparent)]
    Protocol(#[from] freedback_protocol::Error),
    /// JSON (de)serialization of a stored annotation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
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
