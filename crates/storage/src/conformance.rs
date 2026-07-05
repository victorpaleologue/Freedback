//! Shared conformance suite every [`FeedbackStore`] backend must pass.
//!
//! Uses fixed issuers and timestamps so results are deterministic.

use freedback_protocol::{Annotation, Body, Creator, Motivation, Target};

use crate::{FeedbackStore, Query, StoreError};

const T1: &str = "https://example.com/item/1";
const T2: &str = "https://example.com/item/2";
const K1: &str = "did:key:issuer-one";
const K2: &str = "did:key:issuer-two";

// Unix 100/120/150/200 as RFC 3339, so `iat()` ordering is unambiguous.
const TS100: &str = "1970-01-01T00:01:40Z";
const TS120: &str = "1970-01-01T00:02:00Z";
const TS150: &str = "1970-01-01T00:02:30Z";
const TS200: &str = "1970-01-01T00:03:20Z";

fn ann(
    target: &str,
    issuer: &str,
    created: &str,
    motivation: Motivation,
    body: Body,
) -> Annotation {
    Annotation::new(motivation, Target::Iri(target.into()), vec![body])
        .with_creator(Creator::new(issuer))
        .with_created(created)
}

/// Run the full conformance suite against `store`.
pub async fn run<S: FeedbackStore>(store: &S) {
    // a1: T1/K1 @100; a2 edit: T1/K1 @200; a3: T1/K2 @150; a4: T2/K1 @120.
    let a1 = ann(T1, K1, TS100, Motivation::Assessing, Body::star(4.0));
    let a2 = ann(T1, K1, TS200, Motivation::Assessing, Body::star(5.0));
    let a3 = ann(T1, K2, TS150, Motivation::Assessing, Body::thumb(true));
    let a4 = ann(
        T2,
        K1,
        TS120,
        Motivation::Commenting,
        Body::Comment { value: "hi".into() },
    );

    let id1 = store.put(&a1).await.unwrap();
    let id2 = store.put(&a2).await.unwrap();
    let id3 = store.put(&a3).await.unwrap();
    let _id4 = store.put(&a4).await.unwrap();
    assert!(id1.created && id2.created && id3.created);

    // Idempotency: re-putting a1 does not create.
    let again = store.put(&a1).await.unwrap();
    assert!(!again.created, "re-put must be idempotent");
    assert_eq!(again.dedup_id, id1.dedup_id);

    // get
    let got = store.get(&id1.dedup_id).await.unwrap().expect("a1 present");
    assert_eq!(got.target.source(), T1);
    assert!(store.get("deadbeef").await.unwrap().is_none());

    // query T1: a1(100), a3(150), a2(200) ordered by iat asc.
    let page = store
        .query(&Query {
            target: Some(T1.into()),
            page: 0,
            page_size: 0,
        })
        .await
        .unwrap();
    assert_eq!(page.total, 3, "three annotations target T1");
    let iats: Vec<i64> = page.items.iter().map(|a| a.iat().unwrap()).collect();
    assert_eq!(iats, vec![100, 150, 200], "ordered by iat ascending");

    // paging: page_size 2 → [a1,a3] then [a2].
    let p0 = store
        .query(&Query {
            target: Some(T1.into()),
            page: 0,
            page_size: 2,
        })
        .await
        .unwrap();
    assert_eq!(p0.items.len(), 2);
    assert_eq!(p0.total, 3);
    let p1 = store
        .query(&Query {
            target: Some(T1.into()),
            page: 1,
            page_size: 2,
        })
        .await
        .unwrap();
    assert_eq!(p1.items.len(), 1);
    assert_eq!(p1.items[0].iat().unwrap(), 200);

    // query all targets
    let all = store.query(&Query::default()).await.unwrap();
    assert_eq!(all.total, 4);

    // sync gt_iat (no collapse): T1 with iat > 140 → a3(150), a2(200).
    let newer = store.sync(T1, 140, false).await.unwrap();
    let newer_iats: Vec<i64> = newer.iter().map(|a| a.iat().unwrap()).collect();
    assert_eq!(newer_iats, vec![150, 200]);

    // sync no-op when cursor is ahead of everything.
    assert!(store.sync(T1, 1000, false).await.unwrap().is_empty());

    // latest_edits_only: per (issuer,target) keep latest → K1=a2(200), K2=a3(150).
    let latest = store.sync(T1, 0, true).await.unwrap();
    let latest_iats: Vec<i64> = latest.iter().map(|a| a.iat().unwrap()).collect();
    assert_eq!(
        latest_iats,
        vec![150, 200],
        "K1 collapses to its latest edit"
    );
    // The collapsed K1 row is the star-5 edit, not star-4.
    let k1_latest = latest
        .iter()
        .find(|a| a.creator.as_ref().unwrap().id == K1)
        .unwrap();
    assert_eq!(k1_latest.iat().unwrap(), 200);
}

/// Right-to-erasure semantics every backend must share (ADR 0021):
/// delete → query/get gone → re-put rejected → tombstone feed lists it —
/// and deletion composes with edit supersession (deleting the newest edit
/// re-exposes the older one as the latest).
pub async fn erasure<S: FeedbackStore>(store: &S) {
    // a1: T1/K1 @100; a2 edit: T1/K1 @200 (supersedes a1); a3: T1/K2 @150.
    let a1 = ann(T1, K1, TS100, Motivation::Assessing, Body::star(4.0));
    let a2 = ann(T1, K1, TS200, Motivation::Assessing, Body::star(5.0));
    let a3 = ann(T1, K2, TS150, Motivation::Assessing, Body::thumb(true));
    let id1 = store.put(&a1).await.unwrap().dedup_id;
    let id2 = store.put(&a2).await.unwrap().dedup_id;
    let _id3 = store.put(&a3).await.unwrap().dedup_id;

    // Before the delete: a2 is K1's latest edit.
    let latest = store.sync(T1, 0, true).await.unwrap();
    assert_eq!(latest.len(), 2);
    assert!(!store.is_tombstoned(&id2).await.unwrap());

    // Delete a2 (the newest edit), keeping the content-free proof.
    let proof = serde_json::json!({
        "type": "Delete", "annotation": id2, "created": TS200,
    });
    let removed = store.delete(&id2, 300, proof.clone()).await.unwrap();
    assert!(removed, "an existing annotation was actually removed");

    // query no longer returns it.
    let page = store
        .query(&Query {
            target: Some(T1.into()),
            page: 0,
            page_size: 0,
        })
        .await
        .unwrap();
    assert_eq!(page.total, 2, "T1 is down to a1 + a3");
    assert!(
        page.items.iter().all(|a| a.iat().unwrap() != 200),
        "the deleted annotation is gone from query"
    );

    // get by id no longer returns it, and the tombstone is visible.
    assert!(store.get(&id2).await.unwrap().is_none());
    assert!(store.is_tombstoned(&id2).await.unwrap());

    // Re-putting the same content is rejected: erased stays erased.
    assert!(matches!(
        store.put(&a2).await,
        Err(StoreError::Tombstoned(id)) if id == id2
    ));

    // The tombstone feed lists it (with the proof); the cursor is exclusive.
    let tombs = store.tombstones(0).await.unwrap();
    assert_eq!(tombs.len(), 1);
    assert_eq!(tombs[0].dedup_id, id2);
    assert_eq!(tombs[0].deleted_at, 300);
    assert_eq!(tombs[0].proof, proof);
    assert!(store.tombstones(300).await.unwrap().is_empty());

    // Accepted edit semantics: with the newest edit erased, the OLDER edit is
    // K1's latest again in the collapsed sync view.
    let latest = store.sync(T1, 0, true).await.unwrap();
    let k1_latest = latest
        .iter()
        .find(|a| a.creator.as_ref().unwrap().id == K1)
        .expect("K1 still has an (older) annotation");
    assert_eq!(k1_latest.iat().unwrap(), 100, "a1 is the latest again");

    // A second delete of the same id is a no-op: nothing left to remove, and
    // the original tombstone (deleted_at=300) is kept (first delete wins).
    let again = store
        .delete(&id2, 999, serde_json::json!({"replayed": true}))
        .await
        .unwrap();
    assert!(!again);
    let tombs = store.tombstones(0).await.unwrap();
    assert_eq!(tombs.len(), 1);
    assert_eq!(tombs[0].deleted_at, 300, "first delete wins");
    assert_eq!(tombs[0].proof, proof);

    // Deleting an id that never existed is sane: records the tombstone (an
    // erasure may propagate ahead of its content) but removes nothing.
    let ghost = "deadbeef".to_string();
    let removed = store
        .delete(&ghost, 400, serde_json::json!({"type": "Delete"}))
        .await
        .unwrap();
    assert!(!removed, "nothing existed to remove");
    assert!(store.is_tombstoned(&ghost).await.unwrap());

    // a1 itself is untouched.
    assert!(store.get(&id1).await.unwrap().is_some());
}

/// A snapshot dumped from `src` reloads into a fresh `dst` (durable persistence),
/// including tombstones (the snapshot format is additive: annotation lines plus
/// `{"type":"Tombstone",...}` lines — old annotation-only files still load).
pub async fn persistence<S: FeedbackStore>(src: &S, dst: &S, path: &str) {
    src.put(&ann(T1, K1, TS100, Motivation::Assessing, Body::star(4.0)))
        .await
        .unwrap();
    src.put(&ann(
        T1,
        K2,
        TS150,
        Motivation::Assessing,
        Body::thumb(true),
    ))
    .await
    .unwrap();

    let written = src.dump_jsonl(path).await.unwrap();
    assert_eq!(written, 2);

    let loaded = dst.load_jsonl(path).await.unwrap();
    assert_eq!(loaded, 2, "all annotations reload into a fresh store");

    let page = dst.query(&Query::default()).await.unwrap();
    assert_eq!(page.total, 2);

    // Reloading again is idempotent (dedup by content id).
    assert_eq!(dst.load_jsonl(path).await.unwrap(), 0);

    // Erase one annotation on the source; the tombstone must survive the
    // snapshot save/load cycle (FREEDBACK_STORE_PATH durability, ADR 0021).
    let deleted = &ann(T1, K1, TS100, Motivation::Assessing, Body::star(4.0));
    let deleted_id = freedback_protocol::dedup_id(deleted).unwrap();
    let proof = serde_json::json!({
        "type": "Delete", "annotation": deleted_id, "created": TS200,
    });
    assert!(src.delete(&deleted_id, 500, proof.clone()).await.unwrap());
    assert_eq!(
        src.dump_jsonl(path).await.unwrap(),
        1,
        "one live annotation"
    );

    let fresh_loaded = dst.load_jsonl(path).await.unwrap();
    assert_eq!(fresh_loaded, 0, "no new annotations (one was erased)");
    // NB: `dst` already held the annotation from the pre-erasure load; the
    // replayed tombstone must not fail the load, and marks the id erased.
    assert!(dst.is_tombstoned(&deleted_id).await.unwrap());
    let tombs = dst.tombstones(0).await.unwrap();
    assert_eq!(tombs.len(), 1);
    assert_eq!(tombs[0].proof, proof);
    assert!(matches!(
        dst.put(deleted).await,
        Err(StoreError::Tombstoned(_))
    ));
}
