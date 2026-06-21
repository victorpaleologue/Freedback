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
