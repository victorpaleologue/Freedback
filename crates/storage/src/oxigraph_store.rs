//! Oxigraph-backed [`FeedbackStore`] — the primary production store.
//!
//! Annotations are persisted as RDF: each is a subject
//! `urn:freedback:ann:<dedup>` carrying its raw JSON-LD as a literal under
//! `freedback:raw`. This keeps the door open for the collection server's SPARQL
//! index/equivalence work (M6) while remaining a faithful annotation store now.
//! The in-memory backend is used here (`Store::new`); the RocksDB backend stays
//! native/durable and is enabled at deployment time.

use async_trait::async_trait;
use freedback_protocol::Annotation;
use oxigraph::model::{GraphName, Literal, NamedNode, NamedOrBlankNodeRef, Quad, Term};
use oxigraph::store::Store;

use crate::{
    latest_edits, order_records, FeedbackStore, Page, PutOutcome, Query, Record, Result, StoreError,
};

const RAW_PRED: &str = "https://freedback.net/ns#raw";

/// An Oxigraph-backed store (in-memory backend).
pub struct OxigraphStore {
    store: Store,
}

impl OxigraphStore {
    /// Create a new in-memory Oxigraph store.
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: Store::new().map_err(be)?,
        })
    }

    /// Open a **durable** on-disk Oxigraph store backed by RocksDB at `path`,
    /// creating it if absent. Annotations survive process restarts with no
    /// snapshot loop (the `FREEDBACK_STORE_PATH` JSON-Lines mechanism is for the
    /// in-memory build; this is the production durable backend).
    ///
    /// Requires the `rocksdb` feature (native only — pulls a C/C++ build).
    #[cfg(feature = "rocksdb")]
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            store: Store::open(path).map_err(be)?,
        })
    }

    fn subject(dedup_id: &str) -> Result<NamedNode> {
        NamedNode::new(format!("urn:freedback:ann:{dedup_id}")).map_err(be)
    }

    fn raw_pred() -> NamedNode {
        NamedNode::new_unchecked(RAW_PRED)
    }

    /// Rebuild every record by deserializing the stored raw JSON-LD.
    fn all_records(&self) -> Result<Vec<Record>> {
        let pred = Self::raw_pred();
        let mut recs = Vec::new();
        for q in self
            .store
            .quads_for_pattern(None, Some(pred.as_ref()), None, None)
        {
            let q = q.map_err(be)?;
            if let Term::Literal(l) = q.object {
                let ann: Annotation = serde_json::from_str(l.value())?;
                recs.push(Record::from_annotation(&ann)?);
            }
        }
        Ok(recs)
    }
}

#[async_trait]
impl FeedbackStore for OxigraphStore {
    async fn put(&self, ann: &Annotation) -> Result<PutOutcome> {
        let record = Record::from_annotation(ann)?;
        let subj = Self::subject(&record.dedup_id)?;
        let pred = Self::raw_pred();
        let exists = self
            .store
            .quads_for_pattern(
                Some(NamedOrBlankNodeRef::NamedNode(subj.as_ref())),
                Some(pred.as_ref()),
                None,
                None,
            )
            .next()
            .is_some();
        if !exists {
            let json = serde_json::to_string(ann)?;
            let quad = Quad::new(
                subj,
                pred,
                Literal::new_simple_literal(json),
                GraphName::DefaultGraph,
            );
            self.store.insert(&quad).map_err(be)?;
        }
        Ok(PutOutcome {
            dedup_id: record.dedup_id,
            created: !exists,
        })
    }

    async fn get(&self, dedup_id: &str) -> Result<Option<Annotation>> {
        let subj = Self::subject(dedup_id)?;
        let pred = Self::raw_pred();
        if let Some(q) = self
            .store
            .quads_for_pattern(
                Some(NamedOrBlankNodeRef::NamedNode(subj.as_ref())),
                Some(pred.as_ref()),
                None,
                None,
            )
            .next()
        {
            let q = q.map_err(be)?;
            if let Term::Literal(l) = q.object {
                return Ok(Some(serde_json::from_str(l.value())?));
            }
        }
        Ok(None)
    }

    async fn query(&self, q: &Query) -> Result<Page> {
        let mut records: Vec<Record> = self
            .all_records()?
            .into_iter()
            .filter(|r| q.target.as_ref().is_none_or(|t| &r.target == t))
            .collect();
        order_records(&mut records);
        let total = records.len();
        let items = if q.page_size == 0 {
            records.into_iter().map(|r| r.ann).collect()
        } else {
            records
                .into_iter()
                .skip(q.page * q.page_size)
                .take(q.page_size)
                .map(|r| r.ann)
                .collect()
        };
        Ok(Page {
            items,
            total,
            page: q.page,
            page_size: q.page_size,
        })
    }

    async fn sync(
        &self,
        target: &str,
        gt_iat: i64,
        latest_edits_only: bool,
    ) -> Result<Vec<Annotation>> {
        let mut records: Vec<Record> = self
            .all_records()?
            .into_iter()
            .filter(|r| r.target == target && r.iat > gt_iat)
            .collect();
        if latest_edits_only {
            records = latest_edits(records);
        } else {
            order_records(&mut records);
        }
        Ok(records.into_iter().map(|r| r.ann).collect())
    }
}

fn be<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    #[tokio::test]
    async fn oxigraph_store_conformance() {
        conformance::run(&OxigraphStore::new().unwrap()).await;
    }

    #[tokio::test]
    async fn oxigraph_store_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snap.jsonl");
        conformance::persistence(
            &OxigraphStore::new().unwrap(),
            &OxigraphStore::new().unwrap(),
            path.to_str().unwrap(),
        )
        .await;
    }

    // The durable RocksDB backend must satisfy the same contract as the
    // in-memory one, and — unlike it — survive a full reopen of the database.
    #[cfg(feature = "rocksdb")]
    #[tokio::test]
    async fn rocksdb_store_conformance() {
        let dir = tempfile::tempdir().unwrap();
        conformance::run(&OxigraphStore::open(dir.path()).unwrap()).await;
    }

    #[cfg(feature = "rocksdb")]
    #[tokio::test]
    async fn rocksdb_store_survives_reopen() {
        use freedback_protocol::{Annotation, Body, Creator, Motivation, Target};
        let dir = tempfile::tempdir().unwrap();
        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(4.0)],
        )
        .with_creator(Creator::new("did:key:issuer-one"))
        .with_created("1970-01-01T00:01:40Z");

        // Write, then close the database (drop the store → release the RocksDB lock).
        {
            let store = OxigraphStore::open(dir.path()).unwrap();
            store.put(&ann).await.unwrap();
        }

        // Reopen the same directory — the write must still be there.
        let store = OxigraphStore::open(dir.path()).unwrap();
        let page = store.query(&Query::default()).await.unwrap();
        assert_eq!(
            page.total, 1,
            "the RocksDB backend must persist writes across a reopen"
        );
    }
}
