//! Advanced-client sync tests against a real feedback server.

use std::sync::Arc;

use freedback_advanced_client::{AdvancedClient, LocalStore, ReconcileVia};
use freedback_feedback_server::{build_app, AppState};
use freedback_protocol::{Annotation, Body, Creator, Motivation, Target};
use freedback_storage::{FeedbackStore, MemoryStore};

const T: &str = "https://example.com/item/T";

fn ann(issuer: &str, created: &str, stars: f64) -> Annotation {
    Annotation::new(
        Motivation::Assessing,
        Target::Iri(T.into()),
        vec![Body::star(stars)],
    )
    .with_creator(Creator::new(issuer))
    .with_created(created)
}

/// A distinct annotation at a given unix `iat` (seconds since epoch). Varying
/// the issuer keeps every dedup id unique even at the same timestamp.
fn ann_at(issuer: &str, iat: i64) -> Annotation {
    let created = time::OffsetDateTime::from_unix_timestamp(iat)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    Annotation::new(
        Motivation::Assessing,
        Target::Iri(T.into()),
        vec![Body::star(3.0)],
    )
    .with_creator(Creator::new(issuer))
    .with_created(created)
}

async fn spawn_feedback(store: Arc<MemoryStore>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_app(AppState::new(store, base.clone()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

#[tokio::test]
async fn incremental_then_noop() {
    let store = Arc::new(MemoryStore::new());
    store
        .put(&ann("k1", "1970-01-01T00:01:40Z", 3.0))
        .await
        .unwrap(); // iat 100
    store
        .put(&ann("k2", "1970-01-01T00:02:30Z", 4.0))
        .await
        .unwrap(); // iat 150
    let server = spawn_feedback(store.clone()).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());

    // First sync pulls everything; cursor advances to 150.
    let r1 = client.sync(&server, T).await.unwrap();
    assert_eq!(r1.fetched, 2);
    assert_eq!(r1.new, 2);
    assert_eq!(r1.cursor, 150);

    // A newer annotation appears on the server.
    store
        .put(&ann("k3", "1970-01-01T00:05:00Z", 5.0))
        .await
        .unwrap(); // iat 300

    // Second sync transfers ONLY the newer item.
    let r2 = client.sync(&server, T).await.unwrap();
    assert_eq!(r2.fetched, 1, "only iat > cursor is transferred");
    assert_eq!(r2.new, 1);
    assert_eq!(r2.cursor, 300);

    // Third sync with nothing new is a no-op.
    let r3 = client.sync(&server, T).await.unwrap();
    assert_eq!(r3.fetched, 0);
    assert_eq!(r3.new, 0);

    assert_eq!(client.store().live_by_target(T).unwrap().len(), 3);
}

#[tokio::test]
async fn duplicates_from_two_servers_collapse() {
    // The same annotation (same content id) lives on two servers.
    let dup = ann("k1", "1970-01-01T00:03:20Z", 4.0); // iat 200
    let store_a = Arc::new(MemoryStore::new());
    let store_b = Arc::new(MemoryStore::new());
    store_a.put(&dup).await.unwrap();
    store_b.put(&dup).await.unwrap();
    let server_a = spawn_feedback(store_a).await;
    let server_b = spawn_feedback(store_b).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    let ra = client.sync(&server_a, T).await.unwrap();
    let rb = client.sync(&server_b, T).await.unwrap();

    assert_eq!(ra.new, 1);
    assert_eq!(rb.new, 0, "same content id from a second server is not new");
    assert_eq!(client.store().records().unwrap().len(), 1);
}

#[tokio::test]
async fn backdated_item_reconciled_by_full_pull() {
    let store = Arc::new(MemoryStore::new());
    store
        .put(&ann("k1", "1970-01-01T00:03:20Z", 4.0))
        .await
        .unwrap(); // iat 200
    let server = spawn_feedback(store.clone()).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    let r1 = client.sync(&server, T).await.unwrap();
    assert_eq!(r1.cursor, 200);

    // A backdated annotation (iat 50 < cursor) shows up later.
    store
        .put(&ann("k2", "1970-01-01T00:00:50Z", 2.0))
        .await
        .unwrap(); // iat 50

    // A plain cursor sync misses it.
    let r2 = client.sync(&server, T).await.unwrap();
    assert_eq!(r2.fetched, 0, "gt_iat cursor cannot see backdated items");

    // Full reconciliation catches it.
    let r3 = client.reconcile_full(&server, T).await.unwrap();
    assert!(r3.new >= 1, "full pull reconciles the backdated item");
    assert!(client
        .store()
        .records()
        .unwrap()
        .iter()
        .any(|r| r.iat == 50));
}

#[tokio::test]
async fn negentropy_reconciles_backdated_item() {
    let store = Arc::new(MemoryStore::new());
    store
        .put(&ann("k1", "1970-01-01T00:03:20Z", 4.0))
        .await
        .unwrap(); // iat 200
    let server = spawn_feedback(store.clone()).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    client.sync(&server, T).await.unwrap();

    // A backdated annotation the cursor can never see.
    store
        .put(&ann("k2", "1970-01-01T00:00:50Z", 2.0))
        .await
        .unwrap(); // iat 50
    assert_eq!(client.sync(&server, T).await.unwrap().fetched, 0);

    // Negentropy reconciliation catches it, transferring ONLY the one diff.
    let r = client.reconcile(&server, T).await.unwrap();
    assert_eq!(r.via, ReconcileVia::Negentropy);
    assert_eq!(r.transferred, 1, "only the differing id is transferred");
    assert_eq!(r.new, 1);
    assert!(client
        .store()
        .records()
        .unwrap()
        .iter()
        .any(|r| r.iat == 50));
}

#[tokio::test]
async fn second_reconcile_transfers_o_diff_not_o_all() {
    // The acceptance test for issue #26: a large already-synced set, then a
    // handful of backdated inserts; the reconcile must transfer ~the number of
    // NEW items, far fewer than the total.
    const BASE: i64 = 500;
    const BACKDATED: i64 = 5;

    let store = Arc::new(MemoryStore::new());
    // 500 distinct annotations spread across timestamps 1000..1500.
    for i in 0..BASE {
        store
            .put(&ann_at(&format!("base{i}"), 1000 + i))
            .await
            .unwrap();
    }
    let server = spawn_feedback(store.clone()).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    // First reconcile: the client is empty, so it legitimately pulls everything.
    let first = client.reconcile(&server, T).await.unwrap();
    assert_eq!(first.via, ReconcileVia::Negentropy);
    assert_eq!(first.transferred, BASE as usize);
    assert_eq!(client.store().records().unwrap().len(), BASE as usize);

    // A handful of BACKDATED inserts (low timestamps, below everything synced).
    for i in 0..BACKDATED {
        store
            .put(&ann_at(&format!("back{i}"), 10 + i))
            .await
            .unwrap();
    }

    // Second reconcile: must transfer ONLY the new items — O(diff), not O(all).
    let second = client.reconcile(&server, T).await.unwrap();
    assert_eq!(second.via, ReconcileVia::Negentropy);
    assert_eq!(
        second.transferred, BACKDATED as usize,
        "second reconcile transfers only the {BACKDATED} new items, not all {BASE}"
    );
    assert_eq!(second.new, BACKDATED as usize);
    // The transfer is far smaller than the full set: prove the O(diff) win.
    assert!(
        second.transferred * 10 < BASE as usize,
        "transferred {} must be << total {}",
        second.transferred,
        BASE
    );
    // Convergence stays shallow (logarithmic), not one round per item.
    assert!(
        second.rounds < 10,
        "took {} rounds, expected logarithmic",
        second.rounds
    );
    assert_eq!(
        client.store().records().unwrap().len(),
        (BASE + BACKDATED) as usize
    );

    // A third reconcile with nothing new transfers zero.
    let third = client.reconcile(&server, T).await.unwrap();
    assert_eq!(third.transferred, 0, "no diff → no transfer");
    assert_eq!(third.new, 0);
}

/// The ADR 0021 follow-up: a sync pass also consumes the server's tombstone
/// feed, evicts the erased annotation locally, remembers the erased id, and
/// refuses to re-ingest a stale copy.
#[tokio::test]
async fn sync_consumes_tombstones_and_guards_resurrection() {
    let a1 = ann("k1", "1970-01-01T00:01:40Z", 3.0); // iat 100
    let a2 = ann("k2", "1970-01-01T00:02:30Z", 4.0); // iat 150
    let store = Arc::new(MemoryStore::new());
    store.put(&a1).await.unwrap();
    store.put(&a2).await.unwrap();
    let server = spawn_feedback(store.clone()).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    let r1 = client.sync(&server, T).await.unwrap();
    assert_eq!(r1.new, 2);

    // The author erases a1 on the server (the store-level erasure the signed
    // DELETE handler performs — the HTTP authorization path has its own tests).
    let id1 = freedback_protocol::dedup_id(&a1).unwrap();
    store
        .delete(&id1, 200, serde_json::json!({"type": "Delete"}))
        .await
        .unwrap();

    // The next sync pulls the tombstone feed and evicts the erased copy.
    let r2 = client.sync(&server, T).await.unwrap();
    assert_eq!(r2.fetched, 0, "no new annotations, only an erasure");
    let records = client.store().records().unwrap();
    assert_eq!(records.len(), 1, "exactly the survivor remains");
    assert_eq!(
        records[0].dedup_id,
        freedback_protocol::dedup_id(&a2).unwrap()
    );
    assert!(client.store().get(&id1).unwrap().is_none());
    assert!(
        client.store().is_erased(&id1).unwrap(),
        "erased id remembered"
    );
    assert_eq!(
        client.store().tombstone_cursor(&server).unwrap(),
        200,
        "tombstone cursor advanced to the deleted_at"
    );

    // Stale data arriving later (a re-ingestion attempt) is ignored.
    assert!(!client.store().upsert(&a1).unwrap());
    assert_eq!(client.store().records().unwrap().len(), 1);
    assert_eq!(client.store().live_by_target(T).unwrap().len(), 1);
}

/// A second server still holding a copy of the erased annotation cannot
/// resurrect it — neither via cursor sync nor via negentropy reconciliation
/// (the client only pulls during reconcile; the ingestion guard covers both).
#[tokio::test]
async fn stale_server_cannot_resurrect_erased_annotation() {
    let a1 = ann("k1", "1970-01-01T00:01:40Z", 3.0); // iat 100
    let a2 = ann("k2", "1970-01-01T00:02:30Z", 4.0); // iat 150
    let store_a = Arc::new(MemoryStore::new());
    store_a.put(&a1).await.unwrap();
    store_a.put(&a2).await.unwrap();
    // Server B is stale: it still holds a1 and knows nothing of the erasure.
    let store_b = Arc::new(MemoryStore::new());
    store_b.put(&a1).await.unwrap();
    store_b.put(&a2).await.unwrap();
    let server_a = spawn_feedback(store_a.clone()).await;
    let server_b = spawn_feedback(store_b).await;

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    client.sync(&server_a, T).await.unwrap();

    let id1 = freedback_protocol::dedup_id(&a1).unwrap();
    store_a
        .delete(&id1, 200, serde_json::json!({"type": "Delete"}))
        .await
        .unwrap();
    client.sync(&server_a, T).await.unwrap();
    assert!(client.store().is_erased(&id1).unwrap());

    // Negentropy reconciliation against the stale server identifies a1 as a
    // "need" and transfers it — but the ingestion guard keeps it out.
    let r = client.reconcile(&server_b, T).await.unwrap();
    assert_eq!(r.via, ReconcileVia::Negentropy);
    assert_eq!(r.new, 0, "the erased annotation is not re-ingested");
    assert!(client.store().get(&id1).unwrap().is_none());
    assert_eq!(client.store().records().unwrap().len(), 1);

    // Same for a plain full pull from the stale server.
    let r = client.reconcile_full(&server_b, T).await.unwrap();
    assert_eq!(r.new, 0);
    assert!(client.store().get(&id1).unwrap().is_none());
}

/// Erasure state (tombstone cursor + erased ids) survives a store reopen, and
/// an old database created before the tombstone tables existed still opens.
#[tokio::test]
async fn erasure_state_persists_across_reopen() {
    let a1 = ann("k1", "1970-01-01T00:01:40Z", 3.0);
    let a2 = ann("k2", "1970-01-01T00:02:30Z", 4.0);
    let store = Arc::new(MemoryStore::new());
    store.put(&a1).await.unwrap();
    store.put(&a2).await.unwrap();
    let server = spawn_feedback(store.clone()).await;
    let id1 = freedback_protocol::dedup_id(&a1).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.redb");
    {
        let client = AdvancedClient::new(LocalStore::open(&path).unwrap());
        client.sync(&server, T).await.unwrap();
        store
            .delete(&id1, 200, serde_json::json!({"type": "Delete"}))
            .await
            .unwrap();
        client.sync(&server, T).await.unwrap();
        assert!(client.store().is_erased(&id1).unwrap());
    }

    // Reopen: the eviction, the erased-id memory, and the cursor are durable.
    let client = AdvancedClient::new(LocalStore::open(&path).unwrap());
    assert!(client.store().get(&id1).unwrap().is_none());
    assert!(client.store().is_erased(&id1).unwrap());
    assert_eq!(client.store().tombstone_cursor(&server).unwrap(), 200);
    assert_eq!(client.store().records().unwrap().len(), 1);
    // A further sync resumes from the persisted cursors without error.
    let r = client.sync(&server, T).await.unwrap();
    assert_eq!(r.fetched, 0);
    assert_eq!(r.new, 0);
}

/// A local store created before the erasure tables existed (only the
/// `annotations` + `cursors` tables) opens unchanged — the tombstone tables
/// are an additive migration.
#[tokio::test]
async fn pre_erasure_local_store_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.redb");
    {
        // Simulate the old on-disk schema with raw redb.
        const OLD_ANNS: redb::TableDefinition<&str, &str> =
            redb::TableDefinition::new("annotations");
        const OLD_CURSORS: redb::TableDefinition<&str, i64> = redb::TableDefinition::new("cursors");
        let db = redb::Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        {
            w.open_table(OLD_ANNS).unwrap();
            let mut c = w.open_table(OLD_CURSORS).unwrap();
            c.insert("http://old.example\nurn:t", 42).unwrap();
        }
        w.commit().unwrap();
    }

    let store = LocalStore::open(&path).unwrap();
    assert_eq!(store.cursor("http://old.example", "urn:t").unwrap(), 42);
    assert!(!store.is_erased("deadbeef").unwrap());
    assert_eq!(store.tombstone_cursor("http://old.example").unwrap(), 0);
    assert!(store
        .upsert(&ann("k1", "1970-01-01T00:01:40Z", 3.0))
        .unwrap());
}

/// A server without the `/tombstones` endpoint (pre-erasure, ADR 0021) is
/// skipped silently: sync and reconcile still work.
#[tokio::test]
async fn server_without_tombstones_endpoint_syncs_fine() {
    use axum::routing::get;
    use axum::Router;

    let store = Arc::new(MemoryStore::new());
    store
        .put(&ann("k1", "1970-01-01T00:03:20Z", 4.0))
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = AppState::new(store.clone(), base.clone());
    // Mount ONLY the cursor + collection routes — no /tombstones.
    let app = Router::new()
        .route(
            "/annotations/",
            get(freedback_feedback_server::handlers::get_collection),
        )
        .route("/sync", get(freedback_feedback_server::handlers::get_sync))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    let r = client.sync(&base, T).await.unwrap();
    assert_eq!(r.new, 1, "sync succeeds despite the missing tombstone feed");
    assert_eq!(client.store().tombstone_cursor(&base).unwrap(), 0);
}

#[tokio::test]
async fn reconcile_falls_back_to_full_pull_without_negentropy() {
    // A server with no /negentropy route (a bare router with just /sync and the
    // collection read) must still reconcile, via the labeled full-pull fallback.
    use axum::routing::get;
    use axum::Router;

    let store = Arc::new(MemoryStore::new());
    store
        .put(&ann("k1", "1970-01-01T00:03:20Z", 4.0))
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = AppState::new(store.clone(), base.clone());
    // Mount ONLY the cursor + collection routes — no /negentropy, no /by-id.
    let app = Router::new()
        .route(
            "/annotations/",
            get(freedback_feedback_server::handlers::get_collection),
        )
        .route("/sync", get(freedback_feedback_server::handlers::get_sync))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = AdvancedClient::new(LocalStore::in_memory().unwrap());
    let r = client.reconcile(&base, T).await.unwrap();
    assert_eq!(r.via, ReconcileVia::FullPull, "degrades to full pull");
    assert!(r.new >= 1, "fallback still reconciles");
}
