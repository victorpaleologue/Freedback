//! Freedback advanced client (component 6): a LOCAL COPY so queries resume
//! without starting over — the "RSS-style" incremental update.
//!
//! A [`LocalStore`] (redb, pure Rust) keyed by content-addressed dedup id holds
//! every annotation seen, with a per-`(server, target)` resume cursor (the max
//! `iat`). [`AdvancedClient::sync`] pulls only `iat > cursor` from a server's
//! `/sync` endpoint and merges: identical ids drop, and a newer edit supersedes
//! its predecessor for the same `(issuer, target)`.
//!
//! Backdated items (an `iat` below the cursor) are invisible to the cursor pull.
//! [`AdvancedClient::reconcile`] catches them efficiently via NIP-77 range-based
//! set reconciliation (negentropy, in [`freedback_protocol::negentropy`]): the
//! client and server exchange range fingerprints over the per-`(server, target)`
//! dedup-id set, recurse only into mismatching ranges, and the client fetches
//! ONLY the differing ids — O(diff), not O(all).
//! [`AdvancedClient::reconcile_full`] is kept as the labeled fallback (a
//! from-scratch pull) for peers that do not advertise `/negentropy`.
//!
//! The local copy also honors **erasure** (ADR 0021): every sync/reconcile pass
//! additionally pulls the server's `/tombstones?gt_deleted_at=` feed from a
//! per-server cursor, evicts the erased annotations from the local store, and
//! remembers the erased ids forever — so no later cursor sync, full pull, or
//! negentropy backfill (including stale copies arriving from *another* server)
//! can resurrect a deleted record. The client is a read-only local copy and
//! never pushes during reconciliation, so guarding ingestion is sufficient.
//! Servers without the `/tombstones` endpoint (pre-erasure) are skipped
//! silently.

use freedback_cli_client::{Client, CollectionPoint, ReqwestTransport, Transport};
use freedback_protocol::{dedup_id, Annotation, Item};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ANNS: TableDefinition<&str, &str> = TableDefinition::new("annotations");
const CURSORS: TableDefinition<&str, i64> = TableDefinition::new("cursors");
/// Erased dedup ids (ADR 0021): dedup id -> `deleted_at`. Content-free — the
/// id plus the deletion instant is all the resurrection guard needs (the
/// tombstone's `proof` is not stored, matching the collection server).
/// Additive: databases from before this table exist get it created on open.
const TOMBSTONES: TableDefinition<&str, i64> = TableDefinition::new("tombstones");
/// Per-server tombstone-feed resume cursor: server base URL -> highest
/// `deleted_at` seen (the next pull's `gt_deleted_at`). Additive, like
/// [`TOMBSTONES`].
const TOMBSTONE_CURSORS: TableDefinition<&str, i64> = TableDefinition::new("tombstone_cursors");

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
        // Ensure tables exist. Opening a missing table inside a write
        // transaction creates it, so the erasure tables (ADR 0021) are an
        // additive migration: local stores from before they existed open
        // unchanged and simply gain the empty tables.
        let w = db.begin_write().map_err(db_err)?;
        {
            w.open_table(ANNS).map_err(db_err)?;
            w.open_table(CURSORS).map_err(db_err)?;
            w.open_table(TOMBSTONES).map_err(db_err)?;
            w.open_table(TOMBSTONE_CURSORS).map_err(db_err)?;
        }
        w.commit().map_err(db_err)?;
        Ok(Self { db })
    }

    /// Insert an annotation if unseen; maintain edit-supersession. Returns true
    /// if newly inserted.
    ///
    /// An id this store has seen **erased** (a local tombstone exists, ADR
    /// 0021) is silently ignored — however the stale copy arrives (cursor
    /// sync, full pull, negentropy backfill, or another server) — so deleted
    /// content cannot resurrect locally.
    pub fn upsert(&self, ann: &Annotation) -> Result<bool> {
        let record = Record::from_annotation(ann)?;
        let w = self.db.begin_write().map_err(db_err)?;
        let created;
        {
            let tombs = w.open_table(TOMBSTONES).map_err(db_err)?;
            let erased = tombs
                .get(record.dedup_id.as_str())
                .map_err(db_err)?
                .is_some();
            let mut table = w.open_table(ANNS).map_err(db_err)?;
            if erased
                || table
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
                let group = group_of(&table, &record.issuer, &record.target)?;
                for r in supersession_updates(group) {
                    table
                        .insert(r.dedup_id.as_str(), serde_json::to_string(&r)?.as_str())
                        .map_err(db_err)?;
                }
            }
        }
        w.commit().map_err(db_err)?;
        Ok(created)
    }

    /// Erase an annotation locally because its server published a tombstone
    /// (ADR 0021): drop the record (if held) and remember the erased id so no
    /// later sync, backfill, or reconciliation can resurrect it. Only the id
    /// and `deleted_at` are kept — content-free, like the feed itself. The
    /// first tombstone wins (a replay never rewrites `deleted_at`), and a
    /// tombstone may arrive before the content it erases. Returns true iff an
    /// annotation was actually removed.
    pub fn evict(&self, dedup_id: &str, deleted_at: i64) -> Result<bool> {
        let w = self.db.begin_write().map_err(db_err)?;
        let removed;
        {
            let mut tombs = w.open_table(TOMBSTONES).map_err(db_err)?;
            if tombs.get(dedup_id).map_err(db_err)?.is_none() {
                tombs.insert(dedup_id, deleted_at).map_err(db_err)?;
            }
            let mut table = w.open_table(ANNS).map_err(db_err)?;
            let prev: Option<Record> = match table.get(dedup_id).map_err(db_err)? {
                Some(v) => Some(serde_json::from_str(v.value())?),
                None => None,
            };
            removed = prev.is_some();
            if let Some(r) = prev {
                table.remove(dedup_id).map_err(db_err)?;
                // The erased record may have been the live edit: recompute the
                // (issuer, target) group so its predecessor becomes live again.
                let group = group_of(&table, &r.issuer, &r.target)?;
                for u in supersession_updates(group) {
                    table
                        .insert(u.dedup_id.as_str(), serde_json::to_string(&u)?.as_str())
                        .map_err(db_err)?;
                }
            }
        }
        w.commit().map_err(db_err)?;
        Ok(removed)
    }

    /// Whether this dedup id was erased by a tombstone (ADR 0021).
    pub fn is_erased(&self, dedup_id: &str) -> Result<bool> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(TOMBSTONES).map_err(db_err)?;
        Ok(table.get(dedup_id).map_err(db_err)?.is_some())
    }

    /// The tombstone-feed resume cursor (highest `deleted_at` seen) for a
    /// server — the next pull's `gt_deleted_at`.
    pub fn tombstone_cursor(&self, server: &str) -> Result<i64> {
        let r = self.db.begin_read().map_err(db_err)?;
        let table = r.open_table(TOMBSTONE_CURSORS).map_err(db_err)?;
        Ok(table
            .get(server)
            .map_err(db_err)?
            .map(|v| v.value())
            .unwrap_or(0))
    }

    fn set_tombstone_cursor(&self, server: &str, deleted_at: i64) -> Result<()> {
        let w = self.db.begin_write().map_err(db_err)?;
        {
            let mut table = w.open_table(TOMBSTONE_CURSORS).map_err(db_err)?;
            table.insert(server, deleted_at).map_err(db_err)?;
        }
        w.commit().map_err(db_err)?;
        Ok(())
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

    /// The negentropy item set (`(iat, dedup_id)`) for a target — the **full**
    /// id set held locally (live AND superseded), so reconciliation diffs every
    /// content-addressed id one-for-one against the server's full set. Sorted
    /// into the canonical `(timestamp, id)` order both peers agree on.
    pub fn negentropy_items(&self, target: &str) -> Result<Vec<Item>> {
        let items = self
            .records()?
            .into_iter()
            .filter(|r| r.target == target)
            .map(|r| Item::new(r.iat, r.dedup_id))
            .collect();
        Ok(freedback_protocol::negentropy::sorted(items))
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

/// Collect every record of one `(issuer, target)` edit group.
fn group_of<T>(table: &T, issuer: &str, target: &str) -> Result<Vec<Record>>
where
    T: ReadableTable<&'static str, &'static str>,
{
    let mut group = Vec::new();
    for item in table.iter().map_err(db_err)? {
        let (_k, v) = item.map_err(db_err)?;
        let r: Record = serde_json::from_str(v.value())?;
        if r.issuer == issuer && r.target == target {
            group.push(r);
        }
    }
    Ok(group)
}

/// Which records of one `(issuer, target)` group need their `superseded_by`
/// rewritten so that exactly the record with the highest `(iat, dedup_id)` is
/// live. Shared by insertion (a newer edit supersedes) and eviction (erasing
/// the live edit revives its predecessor).
fn supersession_updates(group: Vec<Record>) -> Vec<Record> {
    let Some(live_iat) = group.iter().map(|r| r.iat).max() else {
        return Vec::new();
    };
    let live = group
        .iter()
        .filter(|r| r.iat == live_iat)
        .max_by(|a, b| a.dedup_id.cmp(&b.dedup_id))
        .unwrap()
        .dedup_id
        .clone();
    group
        .into_iter()
        .filter_map(|mut r| {
            let new_sup = if r.dedup_id == live {
                None
            } else {
                Some(live.clone())
            };
            (r.superseded_by != new_sup).then(|| {
                r.superseded_by = new_sup;
                r
            })
        })
        .collect()
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

/// Which path reconciled a backdated set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileVia {
    /// NIP-77 range-based reconciliation: only the differing ids transferred.
    Negentropy,
    /// The full-pull fallback (peer did not support `POST /negentropy`).
    FullPull,
}

/// Report from a backdated reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Which path was taken.
    pub via: ReconcileVia,
    /// Number of annotations actually transferred from the server (the O(diff)
    /// figure: for negentropy this is exactly the `need` set the client lacked,
    /// NOT the whole target set).
    pub transferred: usize,
    /// How many were newly added locally.
    pub new: usize,
    /// Number of negentropy rounds (0 for the full-pull fallback).
    pub rounds: usize,
    /// The resume cursor after merging.
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

    /// Backdated reconciliation, efficiently: run NIP-77 range-based set
    /// reconciliation against the server's `/negentropy` endpoint, transfer only
    /// the differing ids, and fall back to a full pull if the peer does not
    /// support negentropy.
    ///
    /// This is the replacement for the full-pull stand-in on the reconcile path
    /// (issue #26): a second reconciliation after a handful of backdated inserts
    /// transfers O(diff), not O(all).
    pub async fn reconcile(&self, server_base: &str, target: &str) -> Result<ReconcileReport> {
        let server = server_base.trim_end_matches('/');
        match self.reconcile_negentropy(server, target).await {
            Ok(report) => Ok(report),
            // The server lacks (or rejected) the negentropy endpoint — degrade
            // to the labeled full-pull fallback so reconciliation still works
            // against a peer that only speaks the cursor protocol.
            Err(AdvancedError::Client(_)) => {
                let full = self.pull(server, target, 0, false).await?;
                Ok(ReconcileReport {
                    via: ReconcileVia::FullPull,
                    transferred: full.fetched,
                    new: full.new,
                    rounds: 0,
                    cursor: full.cursor,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// NIP-77 reconciliation core: drive range fingerprint rounds to a fixpoint,
    /// collect the ids only the server holds, bulk-fetch exactly those, and
    /// merge. Returns a [`ReconcileReport`] whose `transferred` is the O(diff)
    /// count. Surfaces `AdvancedError::Client` (so [`Self::reconcile`] can fall
    /// back) when the peer has no negentropy endpoint.
    pub async fn reconcile_negentropy(
        &self,
        server_base: &str,
        target: &str,
    ) -> Result<ReconcileReport> {
        let server = server_base.trim_end_matches('/');
        let point = CollectionPoint::from_server(server);

        // Learn erasures first (ADR 0021), so the reconciliation below diffs
        // against a local set that already excludes what this server deleted.
        self.refresh_tombstones(server).await?;

        let local = self.store.negentropy_items(target)?;

        // Round 0: the client's opening full-range claim.
        let mut message = freedback_protocol::negentropy::initiate(&local);
        let mut need: Vec<String> = Vec::new();
        let mut rounds = 0;
        // Bound the loop defensively; convergence is logarithmic in the set size.
        const MAX_ROUNDS: usize = 64;
        loop {
            rounds += 1;
            let reply = self
                .client
                .negentropy_round(&point, target, &message)
                .await
                .map_err(|e| AdvancedError::Client(e.to_string()))?;
            let rec = freedback_protocol::negentropy::reconcile(&local, &reply);
            need.extend(rec.need);
            // `have` (ids only we hold) is intentionally ignored: the
            // advanced-client is a read-only local copy and does not push.
            if rec.next.is_empty() || rounds >= MAX_ROUNDS {
                break;
            }
            message = rec.next;
        }
        need.sort();
        need.dedup();

        // Fetch ONLY the differing ids — the O(diff) transfer.
        let fetched = self
            .client
            .fetch_by_id(&point, &need)
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

        Ok(ReconcileReport {
            via: ReconcileVia::Negentropy,
            transferred: fetched.len(),
            new,
            rounds,
            cursor: max_iat,
        })
    }

    /// Full reconciliation: pulls every annotation from `iat = 0` (catches
    /// backdated items a plain cursor pull would miss). Kept as the labeled
    /// fallback for [`Self::reconcile`] when a peer does not support negentropy;
    /// prefer [`Self::reconcile`] for the O(diff) path.
    pub async fn reconcile_full(&self, server_base: &str, target: &str) -> Result<SyncReport> {
        let server = server_base.trim_end_matches('/');
        self.pull(server, target, 0, false).await
    }

    /// Pull the server's tombstone feed from the per-server cursor, evict the
    /// erased annotations from the local store, and advance the cursor (ADR
    /// 0021). Every sync/reconcile pass calls this, so an erasure propagates
    /// no later than the next pull against that server. A server without the
    /// `/tombstones` endpoint (pre-erasure) — or any transport failure — is
    /// skipped silently: older servers must not break sync. Returns how many
    /// annotations were evicted.
    async fn refresh_tombstones(&self, server: &str) -> Result<usize> {
        let point = CollectionPoint::from_server(server);
        let cursor = self.store.tombstone_cursor(server)?;
        let Ok(tombs) = self.client.tombstones(&point, cursor).await else {
            return Ok(0);
        };
        let mut evicted = 0;
        let mut max = cursor;
        for t in &tombs {
            if self.store.evict(&t.dedup_id, t.deleted_at)? {
                evicted += 1;
            }
            max = max.max(t.deleted_at);
        }
        if max > cursor {
            self.store.set_tombstone_cursor(server, max)?;
        }
        Ok(evicted)
    }

    async fn pull(
        &self,
        server: &str,
        target: &str,
        gt_iat: i64,
        latest_edits_only: bool,
    ) -> Result<SyncReport> {
        let point = CollectionPoint::from_server(server);

        // The same pass also consumes the server's erasure feed (ADR 0021).
        self.refresh_tombstones(server).await?;

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

    #[test]
    fn evict_removes_guards_and_revives_predecessor() {
        let store = LocalStore::in_memory().unwrap();
        let v1 = ann("t", "k1", "1970-01-01T00:01:40Z", 3.0); // iat 100
        let v2 = ann("t", "k1", "1970-01-01T00:03:20Z", 5.0); // iat 200 (edit)
        store.upsert(&v1).unwrap();
        store.upsert(&v2).unwrap();
        let id2 = dedup_id(&v2).unwrap();

        // Evicting the live edit removes it and revives the predecessor.
        assert!(store.evict(&id2, 300).unwrap());
        assert!(store.is_erased(&id2).unwrap());
        assert!(store.get(&id2).unwrap().is_none());
        let live = store.live_by_target("t").unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].iat, 100, "the predecessor is live again");

        // A stale copy of the erased annotation cannot resurrect.
        assert!(!store.upsert(&v2).unwrap());
        assert!(store.get(&id2).unwrap().is_none());
        assert_eq!(store.records().unwrap().len(), 1);

        // Idempotent: replaying the tombstone removes nothing further.
        assert!(!store.evict(&id2, 999).unwrap());
    }

    #[test]
    fn tombstone_may_precede_content() {
        let store = LocalStore::in_memory().unwrap();
        let v1 = ann("t", "k1", "1970-01-01T00:01:40Z", 3.0);
        let id1 = dedup_id(&v1).unwrap();

        // The erasure arrives before the content it erases.
        assert!(!store.evict(&id1, 50).unwrap(), "nothing to remove yet");
        assert!(store.is_erased(&id1).unwrap());
        assert!(!store.upsert(&v1).unwrap(), "late content stays out");
        assert!(store.records().unwrap().is_empty());
    }
}
