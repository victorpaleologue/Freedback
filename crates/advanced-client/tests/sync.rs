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
