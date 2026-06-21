//! TestCluster: real feedback servers + a discovery server on ephemeral ports.
//! Proves announce-with-verify and target resolution end to end.

use std::sync::Arc;

use freedback_discovery_server::clock::TestClock;
use freedback_discovery_server::{
    build_app as build_disc, sign_announce, AppState as DiscState, RegistryConfig,
};
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

/// Spawn a discovery server whose state is returned too, so tests can drive
/// `sweep`/`gossip` and the injected clock directly (no wall-clock sleeps).
async fn spawn_discovery_with(state: DiscState) -> (String, DiscState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let returned = state.clone();
    let app = build_disc(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, returned)
}

/// A feedback server that publishes `key_pem` in its well-known (for signed
/// announce corroboration), pre-seeded with one annotation for `target`.
async fn spawn_feedback_with_key(target: &str, key_pem: &str) -> String {
    let store = Arc::new(MemoryStore::new());
    store.put(&signed(target)).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = build_fb(FbState::new(store, base.clone()).with_server_key_pem(key_pem));
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
async fn relay_list_outbox_publish_resolve_and_reject() {
    use freedback_discovery_server::relays::RelayList;

    let fb_a = spawn_feedback(TARGET_A).await;
    let disc = spawn_discovery().await;
    let http = reqwest::Client::new();

    let id = Identity::generate();
    let issuer = id.issuer_id().unwrap();

    // The issuer declares it writes to fb_a, and signs the record.
    let mut list = RelayList::new(
        issuer.clone(),
        vec![],
        vec![fb_a.clone()],
        "2026-06-21T10:00:00Z",
    );
    list.sign(&id).unwrap();

    let resp = http
        .post(format!("{disc}/relays"))
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap()["stored"], true);

    // Retrievable, and the signature still verifies after the round-trip.
    let got: RelayList = http
        .get(format!("{disc}/relays?issuer={issuer}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    got.verify().expect("served relay list must still verify");
    assert_eq!(got.write, vec![fb_a.clone()]);

    // Outbox resolution: where does this issuer publish? → fb_a, no fan-out.
    let resolved: Value = http
        .get(format!("{disc}/resolve?issuer={issuer}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["servers"].as_array().unwrap(), &vec![json!(fb_a)]);

    // A stale re-publish (older `updated`) is ignored.
    let mut older = RelayList::new(
        issuer.clone(),
        vec![],
        vec!["http://stale.example".into()],
        "2026-06-20T00:00:00Z",
    );
    older.sign(&id).unwrap();
    let resp = http
        .post(format!("{disc}/relays"))
        .json(&older)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.json::<Value>().await.unwrap()["stored"], false);

    // A tampered list (signature no longer matches) is rejected.
    let mut tampered = list.clone();
    tampered.write.push("http://evil.example".into());
    let resp = http
        .post(format!("{disc}/relays"))
        .json(&tampered)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "tampered relay list must be rejected");

    // The stored list is unchanged (still only fb_a).
    let resolved: Value = http
        .get(format!("{disc}/resolve?issuer={issuer}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["servers"].as_array().unwrap(), &vec![json!(fb_a)]);
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

/// Part 1 — liveness/expiry. A live server survives sweeps (its stamp is
/// refreshed); an unreachable server that is also past its TTL is evicted from
/// `/servers`. Driven by an injected clock — no wall-clock sleeps.
#[tokio::test]
async fn sweep_removes_unreachable_past_ttl() {
    let clock = Arc::new(TestClock::new(1000));
    let config = RegistryConfig {
        server_ttl_secs: 100,
        sweep_interval_secs: 10,
    };
    let (disc, state) = spawn_discovery_with(
        DiscState::new("http://disc.test")
            .with_config(config)
            .with_clock(clock.clone()),
    )
    .await;
    let http = reqwest::Client::new();

    // A short-lived feedback server: announce it while alive, then shut it down.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let store = Arc::new(MemoryStore::new());
    store.put(&signed(TARGET_A)).await.unwrap();
    let app = build_fb(FbState::new(store, url.clone()));
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": url }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(state.servers().len(), 1);

    // Kill the server so its well-known stops answering.
    handle.abort();
    // Give the abort a moment to take effect without a wall-clock-dependent
    // assertion: poll until the port refuses connections.
    for _ in 0..50 {
        if http
            .get(format!("{url}/.well-known/freedback"))
            .send()
            .await
            .is_err()
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Sweep while still inside the TTL grace window: unreachable but NOT yet
    // expired, so it is kept (transient-blip tolerance).
    clock.advance(50);
    let removed = state.sweep().await;
    assert!(removed.is_empty(), "within grace window, keep: {removed:?}");
    assert_eq!(state.servers().len(), 1);

    // Advance past the TTL: now the unreachable server is evicted.
    clock.advance(100);
    let removed = state.sweep().await;
    assert_eq!(removed, vec![url.clone()], "stale server must be dropped");
    assert!(state.servers().is_empty());

    // And it no longer appears in `/servers`.
    let servers: Value = http
        .get(format!("{disc}/servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(servers["servers"].as_array().unwrap().is_empty());
}

/// Part 2 — signed announce. A server proves control of the key it publishes in
/// its well-known; a forged signature (key mismatch) is rejected.
#[tokio::test]
async fn signed_announce_proves_key_control() {
    let id = Identity::generate();
    let key_pem = id.public_key_pem().unwrap();
    let fb = spawn_feedback_with_key(TARGET_A, &key_pem).await;
    let disc = spawn_discovery().await;
    let http = reqwest::Client::new();

    // Valid signed announce: signature key matches the published key.
    let sig = sign_announce(&id, &fb).unwrap();
    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": fb, "signature": sig }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["signed"], true, "announce recognized as signed");

    // Forged announce: a different key signs for the same URL → rejected,
    // because it does not match the server's published key.
    let attacker = Identity::generate();
    let bad_sig = sign_announce(&attacker, &fb).unwrap();
    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": fb, "signature": bad_sig }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "key-mismatch signature must be rejected"
    );

    // Unsigned announce of the same server still works (backward compatible).
    let resp = http
        .post(format!("{disc}/announce"))
        .json(&json!({ "url": fb }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap()["signed"], false);
}

/// Part 3 — cross-registry relay-list gossip. A relay list published to
/// registry A propagates to registry B (which re-verifies it) and becomes
/// resolvable there. Acceptance criterion of issue #25.
#[tokio::test]
async fn relay_list_gossips_across_registries() {
    use freedback_discovery_server::relays::RelayList;

    let fb_a = spawn_feedback(TARGET_A).await;
    let (disc_a, state_a) = spawn_discovery_with(DiscState::new("http://a.test")).await;
    let disc_b = spawn_discovery().await;
    let http = reqwest::Client::new();

    let id = Identity::generate();
    let issuer = id.issuer_id().unwrap();

    // Publish a signed relay list to registry A only.
    let mut list = RelayList::new(
        issuer.clone(),
        vec![],
        vec![fb_a.clone()],
        "2026-06-21T10:00:00Z",
    );
    list.sign(&id).unwrap();
    let resp = http
        .post(format!("{disc_a}/relays"))
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.json::<Value>().await.unwrap()["stored"], true);

    // Registry B does not know this issuer yet.
    let resp = http
        .get(format!("{disc_b}/relays?issuer={issuer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "B has not learned the list yet");

    // Gossip A → B (driven explicitly; B re-verifies each signature on receipt).
    let accepted = state_a.gossip_relays_to(&disc_b).await;
    assert_eq!(accepted, 1, "B accepted and stored the gossiped list");

    // Now B serves the list, it still verifies, and outbox resolution works on B.
    let got: RelayList = http
        .get(format!("{disc_b}/relays?issuer={issuer}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    got.verify().expect("gossiped list must still verify on B");
    assert_eq!(got.write, vec![fb_a.clone()]);

    let resolved: Value = http
        .get(format!("{disc_b}/resolve?issuer={issuer}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["servers"].as_array().unwrap(), &vec![json!(fb_a)]);

    // Re-gossiping the same (not-newer) list is a no-op on B.
    let accepted = state_a.gossip_relays_to(&disc_b).await;
    assert_eq!(accepted, 0, "already-current list is not re-stored");
}
