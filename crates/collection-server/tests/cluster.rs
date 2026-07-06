//! Collection-server cluster tests against real feedback servers.

use std::sync::Arc;

use freedback_collection_server::RateLimit;
use freedback_collection_server::{build_app as build_col, AppState as ColState};
use freedback_feedback_server::{build_app as build_fb, AppState as FbState};
use freedback_protocol::{Annotation, Body, Creator, Identity, Motivation, Target};
use freedback_storage::{FeedbackStore, MemoryStore};
use serde_json::{json, Value};

const A: &str = "https://example.com/item/A";
const B: &str = "https://example.com/item/B";

fn signed(target: &str, stars: f64) -> Annotation {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(target.into()),
        vec![Body::star(stars)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_creator(Creator::new(id.issuer_id().unwrap()));
    id.sign_annotation(&mut ann).unwrap();
    ann
}

async fn spawn_feedback(seed: &[Annotation]) -> String {
    spawn_feedback_maxage(seed, 30).await
}

/// Spawn a feedback server advertising a specific `Cache-Control: max-age`.
/// `max_age = 0` disables freshness, forcing the aggregator to revalidate.
async fn spawn_feedback_maxage(seed: &[Annotation], max_age: u64) -> String {
    let store = Arc::new(MemoryStore::new());
    for ann in seed {
        store.put(ann).await.unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_fb(FbState::new(store, base.clone()).with_cache_max_age(max_age));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

async fn spawn_collection(rate: RateLimit) -> (String, ColState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = ColState::with_rate(base.clone(), rate);
    let app = build_col(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, state)
}

/// Serve an already-built state on an ephemeral port, returning the base URL and
/// the server task handle (so the test can abort it to release file locks).
async fn serve_state(state: ColState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_col(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, handle)
}

#[tokio::test]
async fn equivalent_uris_unify_across_servers() {
    let fb_a = spawn_feedback(&[signed(A, 4.0)]).await;
    let fb_b = spawn_feedback(&[signed(B, 5.0)]).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb_a);
    state.add_server(&fb_b);
    let http = reqwest::Client::new();

    // Assert A ≡ B.
    let resp = http
        .post(format!("{col}/equivalence"))
        .json(&json!({ "a": A, "b": B, "proof": "test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // A query on A returns feedback anchored to A *and* B.
    let idx: Value = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(idx["total"], 2, "unified across the equivalence class");
    let eqs = idx["equivalents"].as_array().unwrap();
    assert!(eqs.iter().any(|u| u == A) && eqs.iter().any(|u| u == B));
}

#[tokio::test]
async fn duplicate_annotation_across_servers_collapses() {
    // The same annotation lives on two servers; content id is identical.
    let ann = signed(A, 3.0);
    let fb_a = spawn_feedback(std::slice::from_ref(&ann)).await;
    let fb_b = spawn_feedback(std::slice::from_ref(&ann)).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb_a);
    state.add_server(&fb_b);
    let http = reqwest::Client::new();

    let idx: Value = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(idx["total"], 1, "cross-server dedup by SHA-256 content id");
}

#[tokio::test]
async fn repeated_query_revalidates_with_304() {
    // max-age=0 disables freshness, so the aggregator must revalidate (and the
    // upstream answers 304 by ETag / If-Modified-Since).
    let fb_a = spawn_feedback_maxage(&[signed(A, 4.0)], 0).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb_a);
    let http = reqwest::Client::new();

    // First query populates the cache (200).
    let _ = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap();
    // Second query revalidates → upstream 304.
    let _ = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap();

    assert!(
        state.upstream_304() >= 1,
        "the aggregator should observe a 304 on revalidation, got {}",
        state.upstream_304()
    );
}

#[tokio::test]
async fn fresh_cache_serves_without_any_upstream_call() {
    // A generous max-age means the aggregator reuses the cached page without
    // revalidating — not even a conditional 304 — while it is fresh.
    let fb_a = spawn_feedback_maxage(&[signed(A, 4.0)], 300).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb_a);
    let http = reqwest::Client::new();

    for _ in 0..4 {
        let idx: Value = http
            .get(format!("{col}/index?target={A}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(idx["total"], 1);
    }

    assert_eq!(
        state.upstream_calls(),
        1,
        "only the first (cache-filling) query should reach upstream, got {}",
        state.upstream_calls()
    );
    assert_eq!(
        state.upstream_304(),
        0,
        "a fresh entry must not even revalidate, got {} 304s",
        state.upstream_304()
    );
    assert!(
        state.cache_hits() >= 3,
        "subsequent reads should be served from the fresh cache, got {} hits",
        state.cache_hits()
    );
}

/// The collection server honors upstream tombstones (ADR 0021): an annotation
/// erased at its feedback server disappears from the collection index on the
/// next poll.
#[tokio::test]
async fn tombstones_evict_deleted_annotations() {
    // Keep the author's key so the delete can be signed with the SAME identity.
    let author = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(A.into()),
        vec![Body::star(4.0)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_creator(Creator::new(author.issuer_id().unwrap()));
    author.sign_annotation(&mut ann).unwrap();
    let dedup = freedback_protocol::dedup_id(&ann).unwrap();

    // max-age=0: every /index query revalidates upstream (and pulls tombstones).
    let fb = spawn_feedback_maxage(std::slice::from_ref(&ann), 0).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb);
    let http = reqwest::Client::new();

    // The collection polls and sees it.
    let idx: Value = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(idx["total"], 1, "published annotation is indexed");

    // The author erases it at the upstream feedback server.
    let mut doc = freedback_protocol::DeleteRequest::new(&dedup, "2026-07-05T12:00:00Z");
    author.sign_delete(&mut doc).unwrap();
    let resp = http
        .delete(format!("{fb}/annotations/{dedup}"))
        .json(&doc)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "upstream delete succeeds");

    // The next poll pulls the tombstone feed and evicts the cached copy.
    let idx: Value = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        idx["total"], 0,
        "the collection index no longer returns the erased annotation"
    );
}

/// A target with more annotations than the upstream's default page size (50,
/// oldest-first) must still be fully aggregated: the collection server has to
/// ask upstream for the unbounded collection, not just its first page.
#[tokio::test]
async fn target_past_default_page_size_is_fully_aggregated() {
    let seed: Vec<Annotation> = (0..60).map(|i| signed(A, (i % 5 + 1) as f64)).collect();
    let fb_a = spawn_feedback(&seed).await;
    let (col, state) = spawn_collection(RateLimit::default()).await;
    state.add_server(&fb_a);
    let http = reqwest::Client::new();

    let idx: Value = http
        .get(format!("{col}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        idx["total"], 60,
        "the aggregate must not be capped at the upstream's default page size"
    );
}

#[tokio::test]
async fn rate_limiter_caps_upstream_bursts() {
    // max-age=0 so every query must revalidate (and thus spend a token) — this
    // isolates the rate limiter from the freshness short-circuit.
    let fb_a = spawn_feedback_maxage(&[signed(A, 4.0)], 0).await;
    // Hard cap: 2 tokens, no refill.
    let (col, state) = spawn_collection(RateLimit {
        capacity: 2.0,
        refill_per_sec: 0.0,
    })
    .await;
    state.add_server(&fb_a);
    let http = reqwest::Client::new();

    let mut last_total = 0u64;
    for _ in 0..5 {
        let idx: Value = http
            .get(format!("{col}/index?target={A}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        last_total = idx["total"].as_u64().unwrap();
    }

    assert!(
        state.upstream_calls() <= 2,
        "per-host budget must cap upstream calls, got {}",
        state.upstream_calls()
    );
    // Rate-limited queries still serve cached results.
    assert_eq!(
        last_total, 1,
        "cache serves results once the budget is spent"
    );
}

/// A collection-server restart preserves its registered servers, the URI
/// equivalence class, and the per-`(server, uri)` cache (issue #23 acceptance).
#[tokio::test]
async fn persisted_state_survives_restart() {
    // Upstreams stay up across the collection restart. max-age=300 so the first
    // run caches items and a generous freshness window is recorded.
    let fb_a = spawn_feedback_maxage(&[signed(A, 4.0)], 300).await;
    let fb_b = spawn_feedback_maxage(&[signed(B, 5.0)], 300).await;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("collection-state.redb");
    let http = reqwest::Client::new();

    // --- run #1: register servers, assert A ≡ B, warm the cache, then stop. ---
    {
        let state =
            ColState::with_persistence("http://restart.test", RateLimit::default(), &db).unwrap();
        let (col, handle) = serve_state(state).await;
        state_register(&http, &col, &fb_a).await;
        state_register(&http, &col, &fb_b).await;

        let resp = http
            .post(format!("{col}/equivalence"))
            .json(&json!({ "a": A, "b": B, "proof": "test" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let idx: Value = http
            .get(format!("{col}/index?target={A}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            idx["total"], 2,
            "run #1 unifies across the equivalence class"
        );

        // Stop run #1 and release the redb file lock.
        handle.abort();
        let _ = handle.await;
    }

    // --- run #2: a fresh process reopening the same state file. ---
    let state2 =
        ColState::with_persistence("http://restart.test", RateLimit::default(), &db).unwrap();
    let (col2, _h2) = serve_state(state2).await;

    // Servers were reloaded.
    let servers: Value = http
        .get(format!("{col2}/servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = servers["servers"].as_array().unwrap();
    assert_eq!(list.len(), 2, "registered servers persisted across restart");

    // Equivalence was reloaded.
    let eq: Value = http
        .get(format!("{col2}/equivalence?uri={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let class = eq["class"].as_array().unwrap();
    assert!(
        class.iter().any(|u| u == A) && class.iter().any(|u| u == B),
        "equivalence class persisted across restart, got {class:?}"
    );

    // Index still unifies A and B — cache + equivalence both survived.
    let idx: Value = http
        .get(format!("{col2}/index?target={A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        idx["total"], 2,
        "run #2 still unifies across the persisted equivalence class"
    );
}

async fn state_register(http: &reqwest::Client, col: &str, server: &str) {
    let resp = http
        .post(format!("{col}/servers"))
        .json(&json!({ "url": server }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "register {server}");
}
