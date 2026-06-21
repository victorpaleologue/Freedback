//! TestCluster: real feedback servers + a discovery server on ephemeral ports.
//! Proves announce-with-verify and target resolution end to end.

use std::sync::Arc;

use freedback_discovery_server::{build_app as build_disc, AppState as DiscState};
use freedback_feedback_server::{build_app as build_fb, AppState as FbState};
use freedback_protocol::{Annotation, Body, Creator, Identity, Motivation, Target};
use freedback_storage::{FeedbackStore, MemoryStore};
use serde_json::{json, Value};

const TARGET_A: &str = "https://example.com/item/A";
const TARGET_B: &str = "https://example.com/item/B";

fn signed(target: &str) -> Annotation {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(target.into()),
        vec![Body::star(4.0)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_creator(Creator::new(id.issuer_id().unwrap()));
    id.sign_annotation(&mut ann).unwrap();
    ann
}

/// Spawn a feedback server pre-seeded with one annotation for `target`.
async fn spawn_feedback(target: &str) -> String {
    let store = Arc::new(MemoryStore::new());
    store.put(&signed(target)).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_fb(FbState::new(store, base.clone()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

async fn spawn_discovery() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_disc(DiscState::new(base.clone()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

/// A bound-then-released address — guaranteed closed (connection refused).
async fn dead_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn announce_verify_and_resolve() {
    let fb_a = spawn_feedback(TARGET_A).await;
    let fb_b = spawn_feedback(TARGET_B).await;
    let disc = spawn_discovery().await;
    let http = reqwest::Client::new();

    // Announce both feedback servers; the registry verifies via their well-known.
    for url in [&fb_a, &fb_b] {
        let resp = http
            .post(format!("{disc}/announce"))
            .json(&json!({ "url": url }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "announce of {url} should succeed");
    }

    // Both are listed.
    let servers: Value = http
        .get(format!("{disc}/servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(servers["servers"].as_array().unwrap().len(), 2);

    // A server with no valid well-known is rejected (never trusted blindly).
    let dead = dead_url().await;
    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": dead }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "unverifiable server must be rejected");

    // Announcing a live server that is NOT a Freedback server is also rejected:
    // point the registry at the discovery server's own non-matching... actually
    // the discovery well-known DOES advertise freedback/1, so use a 404 path.
    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": format!("{disc}/nope") }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Resolve TARGET_A → only fb_a holds it.
    let resolved: Value = http
        .get(format!("{disc}/resolve?target={TARGET_A}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let holders = resolved["servers"].as_array().unwrap();
    assert_eq!(holders.len(), 1, "exactly one server holds TARGET_A");
    assert_eq!(holders[0].as_str().unwrap(), fb_a);

    // Resolve an unknown target → no holders.
    let resolved: Value = http
        .get(format!(
            "{disc}/resolve?target=https://example.com/item/ZZZ"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resolved["servers"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn discovery_is_itself_conformant() {
    let disc = spawn_discovery().await;
    let http = reqwest::Client::new();
    let doc: Value = http
        .get(format!("{disc}/.well-known/freedback"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doc["protocol"], "freedback/1");
    assert!(doc["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "discovery-registry"));
}
