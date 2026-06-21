//! In-memory [`FeedbackStore`] — the fast, deterministic test mock.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use freedback_protocol::Annotation;

use crate::{latest_edits, order_records, FeedbackStore, Page, PutOutcome, Query, Record, Result};

/// A thread-safe in-memory store keyed by dedup id.
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<HashMap<String, Record>>,
}

impl MemoryStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self) -> Vec<Record> {
        self.inner.lock().unwrap().values().cloned().collect()
    }
}

#[async_trait]
impl FeedbackStore for MemoryStore {
    async fn put(&self, ann: &Annotation) -> Result<PutOutcome> {
        let record = Record::from_annotation(ann)?;
        let dedup_id = record.dedup_id.clone();
        let mut map = self.inner.lock().unwrap();
        let created = !map.contains_key(&dedup_id);
        map.entry(dedup_id.clone()).or_insert(record);
        Ok(PutOutcome { dedup_id, created })
    }

    async fn get(&self, dedup_id: &str) -> Result<Option<Annotation>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(dedup_id)
            .map(|r| r.ann.clone()))
    }

    async fn query(&self, q: &Query) -> Result<Page> {
        let mut records: Vec<Record> = self
            .snapshot()
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
            .snapshot()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    #[tokio::test]
    async fn memory_store_conformance() {
        conformance::run(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn memory_store_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snap.jsonl");
        conformance::persistence(
            &MemoryStore::new(),
            &MemoryStore::new(),
            path.to_str().unwrap(),
        )
        .await;
    }
}
