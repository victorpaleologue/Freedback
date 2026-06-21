//! Durable persistence for the collection server's derived state (ADR 0012's
//! "persistent index/cache across restarts" follow-up).
//!
//! The aggregator's working set is otherwise pure in-memory: the registered
//! upstream `servers`, the per-`(server, uri)` `cache`, and the URI
//! `equivalence` union-find. None of it is hard to rebuild from upstream, but
//! losing it on every restart means a cold aggregator re-fans-out to every
//! server and forgets every asserted equivalence — exactly the impolite,
//! redundant traffic the cache was built to avoid.
//!
//! We persist it to a single embedded [`redb`] database (pure Rust, the same KV
//! the advanced client uses — no Clang/RocksDB), write-through on every mutation:
//!
//! - `servers`: a set table (key = normalized base URL, value = `()`).
//! - `equivalence`: an append-only log of asserted `(a, b, proof)` unions,
//!   replayed in order to rebuild the union-find. Storing the *proofs* (not the
//!   collapsed parent map) keeps the audit trail and is trivially mergeable.
//! - `cache`: per-`(server, uri)` entries as JSON. The freshness deadline
//!   (`fresh_until`, an [`Instant`](std::time::Instant)) is intentionally **not**
//!   persisted: an `Instant` is not portable across process restarts, and a
//!   reloaded entry SHOULD be treated as stale so the first post-restart read
//!   revalidates (cheap `304`) rather than serving possibly-stale data without
//!   any check. The validators (`ETag`, `Last-Modified`) and last-known items
//!   ARE persisted, so that first revalidation is conditional and usually a `304`.

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::CacheEntry;

const SERVERS: TableDefinition<&str, ()> = TableDefinition::new("servers");
/// Append-only union log: monotonic u64 key -> JSON `(a, b, proof)`.
const EQUIV_LOG: TableDefinition<u64, &str> = TableDefinition::new("equivalence_log");
/// Cache entries keyed by `"<server>\n<uri>"` -> JSON [`StoredCacheEntry`].
const CACHE: TableDefinition<&str, &str> = TableDefinition::new("cache");

/// The serializable projection of a [`CacheEntry`] (everything except the
/// non-portable `fresh_until` `Instant`).
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredCacheEntry {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub items: Vec<freedback_protocol::Annotation>,
}

/// A durable store for the aggregator's derived state.
pub(crate) struct PersistStore {
    db: Database,
}

type Result<T> = std::result::Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl PersistStore {
    /// Open (or create) the store at `path`.
    pub(crate) fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let db = Database::create(path).map_err(err)?;
        Self::init(db)
    }

    /// An in-memory store (tests).
    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(err)?;
        Self::init(db)
    }

    fn init(db: Database) -> Result<Self> {
        let w = db.begin_write().map_err(err)?;
        {
            w.open_table(SERVERS).map_err(err)?;
            w.open_table(EQUIV_LOG).map_err(err)?;
            w.open_table(CACHE).map_err(err)?;
        }
        w.commit().map_err(err)?;
        Ok(Self { db })
    }

    // --- writes (write-through on mutation) --------------------------------

    /// Record a registered upstream server.
    pub(crate) fn put_server(&self, base: &str) -> Result<()> {
        let w = self.db.begin_write().map_err(err)?;
        {
            let mut t = w.open_table(SERVERS).map_err(err)?;
            t.insert(base, ()).map_err(err)?;
        }
        w.commit().map_err(err)
    }

    /// Append an asserted equivalence to the replay log.
    pub(crate) fn append_equivalence(&self, a: &str, b: &str, proof: &str) -> Result<()> {
        let w = self.db.begin_write().map_err(err)?;
        {
            let mut t = w.open_table(EQUIV_LOG).map_err(err)?;
            let next = t
                .iter()
                .map_err(err)?
                .next_back()
                .transpose()
                .map_err(err)?;
            let key = next.map(|(k, _)| k.value() + 1).unwrap_or(0);
            let json = serde_json::to_string(&(a, b, proof)).map_err(err)?;
            t.insert(key, json.as_str()).map_err(err)?;
        }
        w.commit().map_err(err)
    }

    /// Store (upsert) a cache entry for `(server, uri)`.
    pub(crate) fn put_cache(&self, server: &str, uri: &str, entry: &CacheEntry) -> Result<()> {
        let stored = StoredCacheEntry {
            etag: entry.etag.clone(),
            last_modified: entry.last_modified.clone(),
            items: entry.items.clone(),
        };
        let json = serde_json::to_string(&stored).map_err(err)?;
        let w = self.db.begin_write().map_err(err)?;
        {
            let mut t = w.open_table(CACHE).map_err(err)?;
            t.insert(cache_key(server, uri).as_str(), json.as_str())
                .map_err(err)?;
        }
        w.commit().map_err(err)
    }

    // --- reads (replay on boot) --------------------------------------------

    /// Every persisted server base URL.
    pub(crate) fn servers(&self) -> Result<Vec<String>> {
        let r = self.db.begin_read().map_err(err)?;
        let t = r.open_table(SERVERS).map_err(err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(err)? {
            let (k, _) = item.map_err(err)?;
            out.push(k.value().to_string());
        }
        Ok(out)
    }

    /// Every asserted equivalence, in assertion order, for union-find replay.
    pub(crate) fn equivalences(&self) -> Result<Vec<(String, String, String)>> {
        let r = self.db.begin_read().map_err(err)?;
        let t = r.open_table(EQUIV_LOG).map_err(err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(err)? {
            let (_k, v) = item.map_err(err)?;
            let (a, b, p): (String, String, String) =
                serde_json::from_str(v.value()).map_err(err)?;
            out.push((a, b, p));
        }
        Ok(out)
    }

    /// Every persisted cache entry as `((server, uri), entry)`. Reloaded entries
    /// have `fresh_until: None` (stale) so the first read revalidates.
    pub(crate) fn cache(&self) -> Result<Vec<((String, String), CacheEntry)>> {
        let r = self.db.begin_read().map_err(err)?;
        let t = r.open_table(CACHE).map_err(err)?;
        let mut out = Vec::new();
        for item in t.iter().map_err(err)? {
            let (k, v) = item.map_err(err)?;
            let Some((server, uri)) = k.value().split_once('\n') else {
                continue;
            };
            let stored: StoredCacheEntry = serde_json::from_str(v.value()).map_err(err)?;
            out.push((
                (server.to_string(), uri.to_string()),
                CacheEntry {
                    etag: stored.etag,
                    last_modified: stored.last_modified,
                    fresh_until: None,
                    items: stored.items,
                },
            ));
        }
        Ok(out)
    }
}

fn cache_key(server: &str, uri: &str) -> String {
    format!("{server}\n{uri}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_and_equivalences_round_trip() {
        let s = PersistStore::in_memory().unwrap();
        s.put_server("https://a.example").unwrap();
        s.put_server("https://b.example").unwrap();
        s.append_equivalence("urn:x", "urn:y", "manual").unwrap();
        s.append_equivalence("urn:y", "urn:z", "agent").unwrap();

        let mut servers = s.servers().unwrap();
        servers.sort();
        assert_eq!(servers, ["https://a.example", "https://b.example"]);

        // Order is preserved for deterministic union-find replay.
        let eqs = s.equivalences().unwrap();
        assert_eq!(
            eqs,
            vec![
                ("urn:x".into(), "urn:y".into(), "manual".into()),
                ("urn:y".into(), "urn:z".into(), "agent".into()),
            ]
        );
    }

    #[test]
    fn cache_entry_round_trips_without_freshness() {
        let s = PersistStore::in_memory().unwrap();
        let entry = CacheEntry {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Sat, 21 Jun 2026 10:00:00 GMT".into()),
            fresh_until: None,
            items: vec![],
        };
        s.put_cache("https://srv", "urn:t", &entry).unwrap();

        let loaded = s.cache().unwrap();
        assert_eq!(loaded.len(), 1);
        let ((server, uri), got) = &loaded[0];
        assert_eq!(server, "https://srv");
        assert_eq!(uri, "urn:t");
        assert_eq!(got.etag.as_deref(), Some("\"abc\""));
        // Reloaded entries are always stale (revalidate on first read).
        assert!(got.fresh_until.is_none());
    }
}
