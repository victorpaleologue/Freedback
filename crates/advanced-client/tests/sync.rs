//! Advanced-client sync tests against a real feedback server.

use std::sync::Arc;

use freedback_advanced_client::{AdvancedClient, LocalStore};
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
