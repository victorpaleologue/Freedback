//! End-to-end client tests: the same `Client` reads from a live endpoint and a
//! file fixture, and round-trips write → read → sync against a real server.

use std::sync::Arc;

use freedback_cli_client::{
    Client, CollectionPoint, Dest, PublicationPoint, ReqwestTransport, Source,
};
use freedback_feedback_server::{build_app, AppState};
use freedback_protocol::{Annotation, Body, Creator, Identity, Motivation, Target};
use freedback_storage::MemoryStore;

const TARGET: &str = "https://example.com/item/1";

async fn spawn_server() -> String {
    let store = Arc::new(MemoryStore::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let app = build_app(AppState::new(store, base.clone()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

fn signed_star(value: f64) -> Annotation {
    let id = Identity::generate();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(TARGET.into()),
        vec![Body::star(value)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_creator(Creator::new(id.issuer_id().unwrap()));
    id.sign_annotation(&mut ann).unwrap();
    ann
}

#[tokio::test]
async fn write_read_sync_roundtrip() {
    let base = spawn_server().await;
    let client = Client::new(ReqwestTransport::new());
    let ann = signed_star(4.0);

    // write → endpoint
    let dest = Dest::Endpoint {
        point: PublicationPoint::from_server(&base),
        bearer: None,
    };
    let stored = client.write(&ann, &dest).await.unwrap();
    assert!(stored.id.is_some(), "server assigns an id");

    // read → endpoint
    let read = client
        .read(
            TARGET,
            &Source::Endpoint(CollectionPoint::from_server(&base)),
        )
        .await
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].target.source(), TARGET);

    // sync → cursor
    let synced = client
        .sync(&CollectionPoint::from_server(&base), TARGET, 0, true)
        .await
        .unwrap();
    assert_eq!(synced.len(), 1);

    // sync with a cursor ahead of everything → empty
    let none = client
        .sync(
            &CollectionPoint::from_server(&base),
            TARGET,
            9_999_999_999,
            true,
        )
        .await
        .unwrap();
    assert!(none.is_empty());
}

/// The `write --key-file` → `delete` ownership roundtrip (ADR 0021): the key
/// persisted at write time is what authorizes the erasure later — and a
/// different key is refused.
#[tokio::test]
async fn write_then_delete_roundtrip_with_key_file() {
    let base = spawn_server().await;
    let client = Client::new(ReqwestTransport::new());
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("identity.pem");

    // write with --key-file semantics: generate once, save the PEM.
    let id = Identity::generate();
    std::fs::write(&key_file, id.to_pkcs8_pem().unwrap()).unwrap();
    let mut ann = Annotation::new(
        Motivation::Assessing,
        Target::Iri(TARGET.into()),
        vec![Body::star(4.0)],
    )
    .with_created("2026-06-21T10:00:00Z")
    .with_creator(Creator::new(id.issuer_id().unwrap()));
    id.sign_annotation(&mut ann).unwrap();

    let point = PublicationPoint::from_server(&base);
    let dest = Dest::Endpoint {
        point: point.clone(),
        bearer: None,
    };
    let stored = client.write(&ann, &dest).await.unwrap();

    // The `--id` flag accepts the full annotation URL `write` printed.
    let full_url = stored.id.clone().unwrap();
    let dedup = freedback_cli_client::dedup_id_from_url(&full_url).to_string();
    assert_eq!(dedup, freedback_protocol::dedup_id(&ann).unwrap());

    // read shows it.
    let source = Source::Endpoint(CollectionPoint::from_server(&base));
    assert_eq!(client.read(TARGET, &source).await.unwrap().len(), 1);

    // delete with a FRESH key → 403 and the annotation survives.
    let stranger = Identity::generate();
    let mut bad = freedback_protocol::DeleteRequest::new(&dedup, "2026-07-05T12:00:00Z");
    stranger.sign_delete(&mut bad).unwrap();
    let err = client.delete(&point, &bad, None).await.unwrap_err();
    assert!(
        err.to_string().contains("403"),
        "a different key must be refused: {err}"
    );
    assert_eq!(client.read(TARGET, &source).await.unwrap().len(), 1);

    // delete with the SAME key-file identity → gone.
    let reloaded = Identity::from_pkcs8_pem(&std::fs::read_to_string(&key_file).unwrap()).unwrap();
    let mut doc = freedback_protocol::DeleteRequest::new(&dedup, "2026-07-05T12:00:00Z");
    reloaded.sign_delete(&mut doc).unwrap();
    client.delete(&point, &doc, None).await.unwrap();

    // read no longer shows it.
    assert!(client.read(TARGET, &source).await.unwrap().is_empty());
}

#[tokio::test]
async fn read_from_file_fixture_same_code_path() {
    // Persist an array of annotations to a file, then read via Source::File.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anns.json");
    let anns = vec![signed_star(5.0)];
    std::fs::write(&path, serde_json::to_string(&anns).unwrap()).unwrap();

    let client = Client::new(ReqwestTransport::new());
    let read = client
        .read(TARGET, &Source::File(path.to_str().unwrap().to_string()))
        .await
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].target.source(), TARGET);
}

#[tokio::test]
async fn write_to_file_then_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.json");
    let client = Client::new(ReqwestTransport::new());

    client
        .write(
            &signed_star(3.0),
            &Dest::File(path.to_str().unwrap().to_string()),
        )
        .await
        .unwrap();
    client
        .write(
            &signed_star(4.0),
            &Dest::File(path.to_str().unwrap().to_string()),
        )
        .await
        .unwrap();

    let read = client
        .read(TARGET, &Source::File(path.to_str().unwrap().to_string()))
        .await
        .unwrap();
    assert_eq!(read.len(), 2, "appended two annotations");
}
