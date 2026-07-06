//! The local "My feedback" journal (redb).
//!
//! One row per publish — `{dedup_id, target, server, created, kind, summary,
//! status}` — so the app can list, update (supersede) and erase the user's own
//! feedback without asking a server who they are. The storage pattern follows
//! `crates/advanced-client`'s redb `LocalStore` (INVARIANT: local KV = redb).

use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// dedup id -> JSON-serialized [`JournalEntry`].
const JOURNAL: TableDefinition<&str, &str> = TableDefinition::new("journal");

/// Journal errors.
#[derive(Debug, Error)]
pub enum JournalError {
    #[error("db: {0}")]
    Db(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

fn db_err<E: std::fmt::Display>(e: E) -> JournalError {
    JournalError::Db(e.to_string())
}

/// The next insertion sequence: one past the highest stamped so far. Rows are
/// never removed (history is kept), and writes hold the single redb write
/// transaction, so this is race-free and monotonic.
fn next_seq<T>(table: &T) -> Result<u64>
where
    T: ReadableTable<&'static str, &'static str>,
{
    let mut max = 0;
    for item in table.iter().map_err(db_err)? {
        let (_k, v) = item.map_err(db_err)?;
        let e: JournalEntry = serde_json::from_str(v.value())?;
        max = max.max(e.seq);
    }
    Ok(max + 1)
}

type Result<T> = std::result::Result<T, JournalError>;

/// Lifecycle of a journal row (ADR 0021 semantics, mirrored locally).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EntryStatus {
    /// The live version.
    Active,
    /// Replaced by a newer edit (same key + target, newest wins).
    Superseded {
        /// The dedup id of the superseding annotation.
        by: String,
    },
    /// Erased on the server (signed delete); kept locally as history.
    Deleted,
}

impl EntryStatus {
    /// A short name for error messages and the UI.
    pub fn name(&self) -> &'static str {
        match self {
            EntryStatus::Active => "active",
            EntryStatus::Superseded { .. } => "superseded",
            EntryStatus::Deleted => "deleted",
        }
    }
}

/// One published contribution, as the journal remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Content-addressed dedup id of the published annotation.
    pub dedup_id: String,
    /// The canonical target URI the feedback is about.
    pub target: String,
    /// The server base URL it was published to.
    pub server: String,
    /// The annotation's `created` timestamp (RFC 3339 UTC).
    pub created: String,
    /// Contribution kind: `stars` / `thumb` / `comment` / `tag`.
    // TODO(issue-type): add `issue` once `Body::Issue` (branch
    // claude/issue-type) lands in freedback-protocol.
    pub kind: String,
    /// A short human summary ("★ 4", first words of a comment, ...).
    pub summary: String,
    /// Lifecycle status.
    pub status: EntryStatus,
    /// Local insertion sequence, stamped by [`Journal::record`]: breaks
    /// `created` ties (second precision) so "newest first" is stable within
    /// one second.
    #[serde(default)]
    pub seq: u64,
}

/// The journal store.
pub struct Journal {
    db: Database,
}

impl Journal {
    /// Open (or create) the journal at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path).map_err(db_err)?;
        Self::init(db)
    }

    /// An in-memory journal (tests).
    pub fn in_memory() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(db_err)?;
        Self::init(db)
    }

    fn init(db: Database) -> Result<Self> {
        let w = db.begin_write().map_err(db_err)?;
        {
            w.open_table(JOURNAL).map_err(db_err)?;
        }
        w.commit().map_err(db_err)?;
        Ok(Self { db })
    }

    /// Insert (or overwrite) a row, stamping its insertion sequence. Returns
    /// the row as stored.
    pub fn record(&self, entry: &JournalEntry) -> Result<JournalEntry> {
        let w = self.db.begin_write().map_err(db_err)?;
        let stored;
        {
            let mut table = w.open_table(JOURNAL).map_err(db_err)?;
            let mut entry = entry.clone();
            entry.seq = next_seq(&table)?;
            table
                .insert(
                    entry.dedup_id.as_str(),
                    serde_json::to_string(&entry)?.as_str(),
                )
                .map_err(db_err)?;
            stored = entry;
        }
        w.commit().map_err(db_err)?;
        Ok(stored)
    }

    /// Fetch one row by dedup id.
    pub fn get(&self, dedup_id: &str) -> Result<Option<JournalEntry>> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(JOURNAL).map_err(db_err)?;
        match table.get(dedup_id).map_err(db_err)? {
            Some(v) => Ok(Some(serde_json::from_str(v.value())?)),
            None => Ok(None),
        }
    }

    /// All rows, newest first (by `created`, then dedup id for stability).
    pub fn list(&self) -> Result<Vec<JournalEntry>> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(JOURNAL).map_err(db_err)?;
        let mut out = Vec::new();
        for item in table.iter().map_err(db_err)? {
            let (_k, v) = item.map_err(db_err)?;
            out.push(serde_json::from_str::<JournalEntry>(v.value())?);
        }
        out.sort_by(|a, b| {
            b.created
                .cmp(&a.created)
                .then_with(|| b.seq.cmp(&a.seq))
                .then_with(|| b.dedup_id.cmp(&a.dedup_id))
        });
        Ok(out)
    }

    /// Mark `old` as superseded by `new` (update-by-supersede).
    pub fn mark_superseded(&self, old: &str, new: &str) -> Result<()> {
        self.update_status(
            old,
            EntryStatus::Superseded {
                by: new.to_string(),
            },
        )
    }

    /// Mark a row erased (ADR 0021).
    pub fn mark_deleted(&self, dedup_id: &str) -> Result<()> {
        self.update_status(dedup_id, EntryStatus::Deleted)
    }

    fn update_status(&self, dedup_id: &str, status: EntryStatus) -> Result<()> {
        let w = self.db.begin_write().map_err(db_err)?;
        {
            let mut table = w.open_table(JOURNAL).map_err(db_err)?;
            let entry: Option<JournalEntry> = match table.get(dedup_id).map_err(db_err)? {
                Some(v) => Some(serde_json::from_str(v.value())?),
                None => None,
            };
            if let Some(mut entry) = entry {
                entry.status = status;
                table
                    .insert(dedup_id, serde_json::to_string(&entry)?.as_str())
                    .map_err(db_err)?;
            }
        }
        w.commit().map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, created: &str) -> JournalEntry {
        JournalEntry {
            dedup_id: id.to_string(),
            target: "https://id.gs1.org/01/03017620422003".to_string(),
            server: "http://127.0.0.1:1".to_string(),
            created: created.to_string(),
            kind: "stars".to_string(),
            summary: "★ 4".to_string(),
            status: EntryStatus::Active,
            seq: 0,
        }
    }

    #[test]
    fn list_is_newest_first() {
        let j = Journal::in_memory().unwrap();
        j.record(&entry("aa", "2026-07-01T10:00:00Z")).unwrap();
        j.record(&entry("bb", "2026-07-03T10:00:00Z")).unwrap();
        j.record(&entry("cc", "2026-07-02T10:00:00Z")).unwrap();
        let ids: Vec<_> = j.list().unwrap().into_iter().map(|e| e.dedup_id).collect();
        assert_eq!(ids, vec!["bb", "cc", "aa"]);
    }

    #[test]
    fn statuses_are_updated_in_place() {
        let j = Journal::in_memory().unwrap();
        j.record(&entry("aa", "2026-07-01T10:00:00Z")).unwrap();
        j.record(&entry("bb", "2026-07-02T10:00:00Z")).unwrap();

        j.mark_superseded("aa", "bb").unwrap();
        assert_eq!(
            j.get("aa").unwrap().unwrap().status,
            EntryStatus::Superseded { by: "bb".into() }
        );

        j.mark_deleted("bb").unwrap();
        assert_eq!(j.get("bb").unwrap().unwrap().status, EntryStatus::Deleted);
        assert_eq!(j.list().unwrap().len(), 2, "history rows are kept");
    }

    #[test]
    fn same_second_publishes_keep_insertion_order_newest_first() {
        let j = Journal::in_memory().unwrap();
        // All in the same second — `created` alone cannot order them.
        j.record(&entry("cc", "2026-07-01T10:00:00Z")).unwrap();
        j.record(&entry("aa", "2026-07-01T10:00:00Z")).unwrap();
        j.record(&entry("bb", "2026-07-01T10:00:00Z")).unwrap();
        let ids: Vec<_> = j.list().unwrap().into_iter().map(|e| e.dedup_id).collect();
        assert_eq!(ids, vec!["bb", "aa", "cc"], "last inserted first");
    }

    #[test]
    fn marking_an_unknown_id_is_a_no_op() {
        let j = Journal::in_memory().unwrap();
        j.mark_deleted("nope").unwrap();
        assert!(j.get("nope").unwrap().is_none());
    }
}
