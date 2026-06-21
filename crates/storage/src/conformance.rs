//! Shared conformance suite every [`FeedbackStore`] backend must pass.
//!
//! Uses fixed issuers and timestamps so results are deterministic.

use freedback_protocol::{Annotation, Body, Creator, Motivation, Target};

use crate::{FeedbackStore, Query};

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
