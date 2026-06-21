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
    let store = Arc::new(MemoryStore::new());
    for ann in seed {
        store.put(ann).await.unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_fb(FbState::new(store, base.clone()));
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
    let fb_a = spawn_feedback(&[signed(A, 4.0)]).await;
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
async fn rate_limiter_caps_upstream_bursts() {
    let fb_a = spawn_feedback(&[signed(A, 4.0)]).await;
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
