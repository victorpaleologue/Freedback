//! Freedback advanced client (component 6): a LOCAL COPY so queries resume
//! without starting over — the "RSS-style" incremental update.
//!
//! A [`LocalStore`] (redb, pure Rust) keyed by content-addressed dedup id holds
//! every annotation seen, with a per-`(server, target)` resume cursor (the max
//! `iat`). [`AdvancedClient::sync`] pulls only `iat > cursor` from a server's
//! `/sync` endpoint and merges: identical ids drop, and a newer edit supersedes
//! its predecessor for the same `(issuer, target)`. [`AdvancedClient::reconcile_full`]
//! does a from-scratch pull to catch backdated items (a simple stand-in for the
//! negentropy set reconciliation noted in the roadmap).

use freedback_cli_client::{Client, CollectionPoint, ReqwestTransport, Transport};
use freedback_protocol::{dedup_id, Annotation};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ANNS: TableDefinition<&str, &str> = TableDefinition::new("annotations");
const CURSORS: TableDefinition<&str, i64> = TableDefinition::new("cursors");

/// Errors from the advanced client.
#[derive(Debug, Error)]
pub enum AdvancedError {
    #[error("db: {0}")]
    Db(String),
    #[error("client: {0}")]
    Client(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] freedback_protocol::Error),
}

type Result<T> = std::result::Result<T, AdvancedError>;

fn db_err<E: std::fmt::Display>(e: E) -> AdvancedError {
    AdvancedError::Db(e.to_string())
}

/// A locally stored annotation plus its index columns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub dedup_id: String,
    pub target: String,
    pub issuer: String,
    pub iat: i64,
    /// Set to the dedup id of a newer edit (same issuer+target) that supersedes
    /// this one; `None` means this is the live version.
    pub superseded_by: Option<String>,
    pub annotation: Annotation,
}

impl Record {
    fn from_annotation(ann: &Annotation) -> Result<Self> {
        let dedup_id = dedup_id(ann)?;
        let issuer = ann
            .creator
            .as_ref()
            .map(|c| c.id.clone())
            .unwrap_or_else(|| format!("anon:{dedup_id}"));
        Ok(Self {
            dedup_id,
            target: ann.target.source().to_string(),
            issuer,
            iat: ann.iat().unwrap_or(0),
            superseded_by: None,
            annotation: ann.clone(),
        })
    }
}

/// The local sync store.
pub struct LocalStore {
    db: Database,
}

impl LocalStore {
    /// Open (or create) a store at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let db = Database::create(path).map_err(db_err)?;
        Self::init(db)
    }

    /// An in-memory store (tests).
    pub fn in_memory() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(db_err)?;
        Self::init(db)
    }

    fn init(db: Database) -> Result<Self> {
        // Ensure tables exist.
        let w = db.begin_write().map_err(db_err)?;
        {
            w.open_table(ANNS).map_err(db_err)?;
            w.open_table(CURSORS).map_err(db_err)?;
        }
        w.commit().map_err(db_err)?;
        Ok(Self { db })
    }

    /// Insert an annotation if unseen; maintain edit-supersession. Returns true
    /// if newly inserted.
    pub fn upsert(&self, ann: &Annotation) -> Result<bool> {
        let record = Record::from_annotation(ann)?;
        let w = self.db.begin_write().map_err(db_err)?;
        let created;
        {
            let mut table = w.open_table(ANNS).map_err(db_err)?;
            if table
                .get(record.dedup_id.as_str())
                .map_err(db_err)?
                .is_some()
            {
                created = false;
            } else {
                table
                    .insert(
                        record.dedup_id.as_str(),
                        serde_json::to_string(&record)?.as_str(),
                    )
                    .map_err(db_err)?;
                created = true;
            }

            if created {
                // Recompute supersession for this (issuer, target) group.
                let mut group: Vec<Record> = Vec::new();
                for item in table.iter().map_err(db_err)? {
                    let (_k, v) = item.map_err(db_err)?;
                    let r: Record = serde_json::from_str(v.value())?;
                    if r.issuer == record.issuer && r.target == record.target {
                        group.push(r);
                    }
                }
                if let Some(live_iat) = group.iter().map(|r| r.iat).max() {
                    // Highest iat (then highest id) is live; others superseded by it.
                    let live = group
                        .iter()
                        .filter(|r| r.iat == live_iat)
                        .max_by(|a, b| a.dedup_id.cmp(&b.dedup_id))
                        .unwrap()
                        .dedup_id
                        .clone();
                    for mut r in group {
                        let new_sup = if r.dedup_id == live {
                            None
                        } else {
                            Some(live.clone())
                        };
                        if r.superseded_by != new_sup {
                            r.superseded_by = new_sup;
                            table
                                .insert(r.dedup_id.as_str(), serde_json::to_string(&r)?.as_str())
                                .map_err(db_err)?;
                        }
                    }
                }
            }
        }
        w.commit().map_err(db_err)?;
        Ok(created)
    }

    /// Fetch a record by dedup id.
    pub fn get(&self, dedup_id: &str) -> Result<Option<Record>> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(ANNS).map_err(db_err)?;
        match table.get(dedup_id).map_err(db_err)? {
            Some(v) => Ok(Some(serde_json::from_str(v.value())?)),
            None => Ok(None),
        }
    }

    /// All records (live and superseded).
    pub fn records(&self) -> Result<Vec<Record>> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(ANNS).map_err(db_err)?;
        let mut out = Vec::new();
        for item in table.iter().map_err(db_err)? {
            let (_k, v) = item.map_err(db_err)?;
            out.push(serde_json::from_str::<Record>(v.value())?);
        }
        Ok(out)
    }

    /// Live (non-superseded) records for a target, ordered by `iat`.
    pub fn live_by_target(&self, target: &str) -> Result<Vec<Record>> {
        let mut out: Vec<Record> = self
            .records()?
            .into_iter()
            .filter(|r| r.target == target && r.superseded_by.is_none())
            .collect();
        out.sort_by_key(|r| r.iat);
        Ok(out)
    }

    /// The resume cursor (max `iat` seen) for a `(server, target)`.
    pub fn cursor(&self, server: &str, target: &str) -> Result<i64> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(CURSORS).map_err(db_err)?;
        Ok(table
            .get(cursor_key(server, target).as_str())
            .map_err(db_err)?
            .map(|v| v.value())
            .unwrap_or(0))
    }

    fn set_cursor(&self, server: &str, target: &str, iat: i64) -> Result<()> {
        let w = self.db.begin_write().map_err(db_err)?;
        {
            let mut table = w.open_table(CURSORS).map_err(db_err)?;
            table
                .insert(cursor_key(server, target).as_str(), iat)
                .map_err(db_err)?;
        }
        w.commit().map_err(db_err)?;
        Ok(())
    }
}

fn cursor_key(server: &str, target: &str) -> String {
    format!("{server}\n{target}")
}

/// Report from a sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// How many annotations the server returned.
    pub fetched: usize,
    /// How many were newly added locally.
    pub new: usize,
    /// The resume cursor after the sync.
    pub cursor: i64,
}

/// The advanced client: a local store plus an HTTP transport.
pub struct AdvancedClient<T: Transport = ReqwestTransport> {
    store: LocalStore,
    client: Client<T>,
}

impl AdvancedClient<ReqwestTransport> {
    /// Build over a local store with the default HTTP transport.
    pub fn new(store: LocalStore) -> Self {
        Self {
            store,
            client: Client::new(ReqwestTransport::new()),
        }
    }
}

impl<T: Transport> AdvancedClient<T> {
    /// Build over a local store with a custom transport.
    pub fn with_transport(store: LocalStore, transport: T) -> Self {
        Self {
            store,
            client: Client::new(transport),
        }
    }

    /// Borrow the local store.
    pub fn store(&self) -> &LocalStore {
        &self.store
    }

    /// Incrementally sync `target` from `server_base`: pulls only `iat > cursor`,
    /// merges, and advances the cursor.
    pub async fn sync(&self, server_base: &str, target: &str) -> Result<SyncReport> {
        let server = server_base.trim_end_matches('/');
        let cursor = self.store.cursor(server, target)?;
        self.pull(server, target, cursor, true).await
    }

    /// Full reconciliation: pulls from `iat = 0` (catches backdated items a
    /// plain cursor pull would miss). The efficient future path is negentropy.
    pub async fn reconcile_full(&self, server_base: &str, target: &str) -> Result<SyncReport> {
        let server = server_base.trim_end_matches('/');
        self.pull(server, target, 0, false).await
    }

    async fn pull(
        &self,
        server: &str,
        target: &str,
        gt_iat: i64,
        latest_edits_only: bool,
    ) -> Result<SyncReport> {
        let point = CollectionPoint::from_server(server);
        let fetched = self
            .client
            .sync(&point, target, gt_iat, latest_edits_only)
            .await
            .map_err(|e| AdvancedError::Client(e.to_string()))?;

        let mut new = 0;
        let mut max_iat = self.store.cursor(server, target)?;
        for ann in &fetched {
            if self.store.upsert(ann)? {
                new += 1;
            }
            max_iat = max_iat.max(ann.iat().unwrap_or(0));
        }
        self.store.set_cursor(server, target, max_iat)?;
        Ok(SyncReport {
            fetched: fetched.len(),
            new,
            cursor: max_iat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freedback_protocol::{Body, Creator, Motivation, Target};

    fn ann(target: &str, issuer: &str, created: &str, stars: f64) -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri(target.into()),
            vec![Body::star(stars)],
        )
        .with_creator(Creator::new(issuer))
        .with_created(created)
    }

    #[test]
    fn upsert_dedups_and_supersedes() {
        let store = LocalStore::in_memory().unwrap();
        let v1 = ann("t", "k1", "1970-01-01T00:01:40Z", 3.0); // iat 100
        let v2 = ann("t", "k1", "1970-01-01T00:03:20Z", 5.0); // iat 200 (edit)

        assert!(store.upsert(&v1).unwrap());
        assert!(!store.upsert(&v1).unwrap(), "duplicate id is dropped");
        assert!(store.upsert(&v2).unwrap());

        // Only the latest edit is live.
        let live = store.live_by_target("t").unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].iat, 200);
        // Both rows exist; the predecessor is marked superseded.
        assert_eq!(store.records().unwrap().len(), 2);
    }
}
