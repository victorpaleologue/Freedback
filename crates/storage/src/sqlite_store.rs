//! SQLite-backed [`FeedbackStore`] — a durable, dependency-light mock.
//!
//! This is the durable counterpart to [`MemoryStore`](crate::MemoryStore): the
//! same semantics, but persisted to a single SQLite file (or an in-memory
//! database). It uses [`rusqlite`] (a native-only C dependency, `bundled` so no
//! system SQLite is needed) and is therefore gated behind the `sqlite` feature
//! and **never** compiled for `wasm32` (INVARIANT 5 / 6).
//!
//! Each annotation is stored as one row keyed by its content-addressed dedup id,
//! with the indexable columns (`target`, `issuer`, `iat`) denormalized out of the
//! raw JSON-LD so `query`/`sync` can filter and order in SQL. The raw annotation
//! is kept verbatim as JSON so the round-trip is lossless.

use std::sync::Mutex;

use async_trait::async_trait;
use freedback_protocol::Annotation;
use rusqlite::Connection;

use crate::{
    latest_edits, order_records, FeedbackStore, Page, PutOutcome, Query, Record, Result, StoreError,
};

/// A SQLite-backed store keyed by dedup id.
///
/// The connection is wrapped in a [`Mutex`] so the store is `Send + Sync` and
/// satisfies the [`FeedbackStore`] bound; SQLite serializes writes anyway.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open (or create) a durable store at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(be)?;
        Self::init(conn)
    }

    /// Create an in-memory store (fast, ephemeral — for tests).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(be)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS annotations (
                 dedup_id TEXT PRIMARY KEY,
                 target   TEXT NOT NULL,
                 issuer   TEXT NOT NULL,
                 iat      INTEGER NOT NULL,
                 raw      TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_target ON annotations (target);
             CREATE INDEX IF NOT EXISTS idx_target_iat ON annotations (target, iat);",
        )
        .map_err(be)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Rebuild [`Record`]s from rows matching an optional target filter.
    fn records_where_target(&self, target: Option<&str>) -> Result<Vec<Record>> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        let mut push_row = |raw: String| -> Result<()> {
            let ann: Annotation = serde_json::from_str(&raw)?;
            out.push(Record::from_annotation(&ann)?);
            Ok(())
        };
        match target {
            Some(t) => {
                let mut stmt = conn
                    .prepare("SELECT raw FROM annotations WHERE target = ?1")
                    .map_err(be)?;
                let rows = stmt.query_map([t], |r| r.get::<_, String>(0)).map_err(be)?;
                for raw in rows {
                    push_row(raw.map_err(be)?)?;
                }
            }
            None => {
                let mut stmt = conn.prepare("SELECT raw FROM annotations").map_err(be)?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(be)?;
                for raw in rows {
                    push_row(raw.map_err(be)?)?;
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl FeedbackStore for SqliteStore {
    async fn put(&self, ann: &Annotation) -> Result<PutOutcome> {
        let record = Record::from_annotation(ann)?;
        let raw = serde_json::to_string(ann)?;
        let conn = self.conn.lock().unwrap();
        // INSERT OR IGNORE makes this idempotent by content id (the PRIMARY KEY).
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO annotations (dedup_id, target, issuer, iat, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    record.dedup_id,
                    record.target,
                    record.issuer,
                    record.iat,
                    raw
                ],
            )
            .map_err(be)?;
        Ok(PutOutcome {
            dedup_id: record.dedup_id,
            created: changed == 1,
        })
    }

    async fn get(&self, dedup_id: &str) -> Result<Option<Annotation>> {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn
            .query_row(
                "SELECT raw FROM annotations WHERE dedup_id = ?1",
                [dedup_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(be)?;
        match raw {
            Some(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            None => Ok(None),
        }
    }

    async fn query(&self, q: &Query) -> Result<Page> {
        let mut records = self.records_where_target(q.target.as_deref())?;
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
            .records_where_target(Some(target))?
            .into_iter()
            .filter(|r| r.iat > gt_iat)
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
    async fn sqlite_store_conformance() {
        conformance::run(&SqliteStore::in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn sqlite_store_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snap.jsonl");
        conformance::persistence(
            &SqliteStore::in_memory().unwrap(),
            &SqliteStore::in_memory().unwrap(),
            path.to_str().unwrap(),
        )
        .await;
    }

    /// A durable file-backed store survives being closed and reopened.
    #[tokio::test]
    async fn sqlite_store_survives_reopen() {
        use freedback_protocol::{Body, Creator, Motivation, Target};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("store.sqlite");

        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/x".into()),
            vec![Body::star(4.0)],
        )
        .with_creator(Creator::new("did:key:k1"))
        .with_created("1970-01-01T00:01:40Z");

        let id = {
            let store = SqliteStore::open(&db).unwrap();
            store.put(&ann).await.unwrap().dedup_id
        };

        // Reopen: the annotation is still there.
        let store = SqliteStore::open(&db).unwrap();
        let got = store.get(&id).await.unwrap();
        assert!(got.is_some(), "annotation survives reopen");
        assert_eq!(store.query(&Query::default()).await.unwrap().total, 1);
    }
}
