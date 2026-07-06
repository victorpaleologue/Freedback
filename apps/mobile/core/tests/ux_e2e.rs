//! End-to-end UX paths: a real [`AppCore`] in a temp dir against an
//! in-process feedback server (in-memory store), exactly like
//! `crates/cli-client/tests/e2e.rs`.
//!
//! Repo testing rules: deterministic fixed keypairs + fixed timestamps, so
//! signatures and dedup ids are stable across runs.

use std::sync::Arc;

use freedback_app_core::{AppCore, Contribution, CoreError, EntryStatus, IdentityError};
use freedback_feedback_server::{build_app, AppState};
use freedback_protocol::Identity;
use freedback_storage::MemoryStore;

const TARGET: &str = "https://id.gs1.org/01/03017620422003";
const T1: &str = "2026-07-01T10:00:00Z";
const T2: &str = "2026-07-02T10:00:00Z";
const T3: &str = "2026-07-03T10:00:00Z";

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

/// A fresh AppCore in its own temp dir, pointed at `server`.
fn core_at(dir: &std::path::Path, server: &str) -> AppCore {
    let core = AppCore::open(dir).unwrap();
    core.set_server_url(server).unwrap();
    core
}

// --- identity lifecycle -------------------------------------------------------

#[tokio::test]
async fn first_run_mints_an_identity_and_second_run_reuses_it() {
    let dir = tempfile::tempdir().unwrap();

    // First run: no key yet; exporting mints one (the key IS the account).
    let core = AppCore::open(dir.path()).unwrap();
    assert!(!core.identity().exists(), "fresh install has no key");
    let pem = core.export_identity().unwrap();
    assert!(pem.contains("BEGIN PRIVATE KEY"), "PKCS#8 PEM: {pem}");
    assert!(core.identity().exists(), "the key is persisted");
    let issuer = core.issuer_id().unwrap();
    drop(core);

    // Second run (same data dir): the SAME account.
    let core = AppCore::open(dir.path()).unwrap();
    assert_eq!(core.issuer_id().unwrap(), issuer);
    assert_eq!(core.export_identity().unwrap(), pem);
}

#[tokio::test]
async fn export_import_pem_roundtrip_publishes_as_the_same_issuer() {
    let server = spawn_server().await;

    // Device A publishes.
    let dir_a = tempfile::tempdir().unwrap();
    let core_a = core_at(dir_a.path(), &server);
    core_a
        .publish_at(TARGET, Contribution::Stars { value: 4.0 }, None, T1)
        .await
        .unwrap();
    let pem = core_a.export_identity().unwrap();
    let issuer_a = core_a.issuer_id().unwrap();

    // Device B (fresh store) imports the PEM and publishes.
    let dir_b = tempfile::tempdir().unwrap();
    let core_b = core_at(dir_b.path(), &server);
    let imported_issuer = core_b.import_identity(&pem).unwrap();
    assert_eq!(imported_issuer, issuer_a);
    core_b
        .publish_at(
            TARGET,
            Contribution::Comment {
                text: "same me".into(),
            },
            None,
            T2,
        )
        .await
        .unwrap();

    // Both annotations on the server carry the SAME creator.
    let anns = core_b.fetch_annotations(TARGET).await.unwrap();
    assert_eq!(anns.len(), 2);
    for ann in &anns {
        assert_eq!(ann.creator.as_ref().unwrap().id, issuer_a);
    }
}

#[tokio::test]
async fn import_garbage_pem_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let core = AppCore::open(dir.path()).unwrap();
    let err = core
        .import_identity("-----BEGIN GARBAGE-----\nzzz\n")
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Identity(IdentityError::InvalidPem(_))),
        "got {err:?}"
    );
}

// --- publish → journal → server ------------------------------------------------

#[tokio::test]
async fn publish_stars_lands_in_journal_and_on_server() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let entry = core
        .publish_at(TARGET, Contribution::Stars { value: 4.0 }, None, T1)
        .await
        .unwrap();
    assert_eq!(entry.kind, "stars");
    assert_eq!(entry.target, TARGET);
    assert_eq!(entry.server, server);
    assert_eq!(entry.status, EntryStatus::Active);
    assert_eq!(entry.created, T1);

    // Journal row appears ("My feedback").
    let journal = core.my_feedback().unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0], entry);

    // The server has it, signed, with the default CC BY 4.0 license.
    let anns = core.fetch_annotations(TARGET).await.unwrap();
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0].target.source(), TARGET);
    assert_eq!(
        anns[0].rights.as_deref(),
        Some("https://creativecommons.org/licenses/by/4.0/"),
        "CC BY 4.0 is the default license"
    );
    freedback_protocol::verify_annotation(&anns[0]).expect("server copy verifies");
    assert_eq!(
        freedback_protocol::dedup_id(&anns[0]).unwrap(),
        entry.dedup_id
    );
}

#[tokio::test]
async fn publish_comment_tag_and_thumb() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    core.publish_at(
        TARGET,
        Contribution::Comment {
            text: "great hazelnut ratio".into(),
        },
        None,
        T1,
    )
    .await
    .unwrap();
    core.publish_at(
        TARGET,
        Contribution::Tag {
            text: "breakfast".into(),
        },
        None,
        T2,
    )
    .await
    .unwrap();
    core.publish_at(TARGET, Contribution::Thumb { up: true }, None, T3)
        .await
        .unwrap();
    // TODO(issue-type): publish an issue once Body::Issue lands in
    // freedback-protocol (branch claude/issue-type).

    let view = core.get_feedback(TARGET).await.unwrap();
    assert_eq!(view.comments.len(), 1);
    assert_eq!(view.comments[0].text, "great hazelnut ratio");
    assert_eq!(view.tags.len(), 1);
    assert_eq!(view.tags[0].text, "breakfast");
    assert_eq!(view.thumbs_up, 1);
    assert_eq!(view.total, 3);

    let kinds: Vec<_> = core
        .my_feedback()
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    // Newest first.
    assert_eq!(kinds, vec!["thumb", "tag", "comment"]);
}

#[tokio::test]
async fn publish_with_explicit_license_overrides_the_default() {
    const CC0: &str = "https://creativecommons.org/publicdomain/zero/1.0/";
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    core.publish_at(
        TARGET,
        Contribution::Stars { value: 5.0 },
        Some(CC0.to_string()),
        T1,
    )
    .await
    .unwrap();
    let anns = core.fetch_annotations(TARGET).await.unwrap();
    assert_eq!(anns[0].rights.as_deref(), Some(CC0));
}

// --- aggregation ------------------------------------------------------------------

#[tokio::test]
async fn aggregate_math_over_a_live_target() {
    let server = spawn_server().await;

    // Three users rate; one thumbs; one comments.
    let times = [
        "2026-07-01T10:00:00Z",
        "2026-07-01T11:00:00Z",
        "2026-07-01T12:00:00Z",
    ];
    let stars = [3.0, 4.0, 5.0];
    for (i, (value, t)) in stars.iter().zip(times).enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_at(dir.path(), &server);
        core.publish_at(TARGET, Contribution::Stars { value: *value }, None, t)
            .await
            .unwrap();
        if i == 0 {
            core.publish_at(TARGET, Contribution::Thumb { up: false }, None, T2)
                .await
                .unwrap();
            core.publish_at(
                TARGET,
                Contribution::Comment { text: "meh".into() },
                None,
                T3,
            )
            .await
            .unwrap();
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let reader = core_at(dir.path(), &server);
    let view = reader.get_feedback(TARGET).await.unwrap();
    assert_eq!(view.star_avg, Some(4.0), "(3+4+5)/3");
    assert_eq!(view.star_count, 3);
    assert_eq!((view.thumbs_up, view.thumbs_down), (0, 1));
    assert_eq!(view.comments.len(), 1);
    assert_eq!(view.total, 5);
}

#[tokio::test]
async fn aggregate_of_an_empty_target_is_empty_not_an_error() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let view = core
        .get_feedback("https://example.com/never-reviewed")
        .await
        .unwrap();
    assert_eq!(view.star_avg, None);
    assert_eq!(view.star_count, 0);
    assert_eq!(view.total, 0);
    assert!(view.comments.is_empty() && view.tags.is_empty());
}

// --- update by supersession ----------------------------------------------------------

#[tokio::test]
async fn update_supersedes_and_the_sync_view_shows_only_the_latest() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let first = core
        .publish_at(TARGET, Contribution::Stars { value: 2.0 }, None, T1)
        .await
        .unwrap();
    let second = core
        .update_entry_at(
            &first.dedup_id,
            Contribution::Stars { value: 5.0 },
            None,
            T2,
        )
        .await
        .unwrap();
    assert_ne!(first.dedup_id, second.dedup_id);

    // Journal: old row superseded-by new, new row active; newest first.
    let journal = core.my_feedback().unwrap();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].dedup_id, second.dedup_id);
    assert_eq!(journal[0].status, EntryStatus::Active);
    assert_eq!(
        journal[1].status,
        EntryStatus::Superseded {
            by: second.dedup_id.clone()
        }
    );

    // The sync (latest-edits) view shows ONLY the update: the edit chain
    // collapsed per (issuer, target), newest wins.
    let view = core.get_feedback_latest(TARGET).await.unwrap();
    assert_eq!(view.star_count, 1, "the edit replaced its predecessor");
    assert_eq!(view.star_avg, Some(5.0));
}

#[tokio::test]
async fn updating_an_unknown_or_deleted_entry_errors_cleanly() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let err = core
        .update_entry_at(
            "f".repeat(64).as_str(),
            Contribution::Stars { value: 1.0 },
            None,
            T1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::UnknownEntry(_)), "got {err:?}");

    let entry = core
        .publish_at(TARGET, Contribution::Stars { value: 3.0 }, None, T1)
        .await
        .unwrap();
    core.erase_entry_at(&entry.dedup_id, T2).await.unwrap();
    let err = core
        .update_entry_at(
            &entry.dedup_id,
            Contribution::Stars { value: 4.0 },
            None,
            T3,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::EntryNotActive { .. }),
        "got {err:?}"
    );
}

// --- erasure (ADR 0021) -----------------------------------------------------------------

#[tokio::test]
async fn erase_removes_from_server_marks_journal_and_blocks_reingestion() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let entry = core
        .publish_at(
            TARGET,
            Contribution::Comment {
                text: "delete me".into(),
            },
            None,
            T1,
        )
        .await
        .unwrap();
    assert_eq!(core.fetch_annotations(TARGET).await.unwrap().len(), 1);

    let erased = core.erase_entry_at(&entry.dedup_id, T2).await.unwrap();
    assert_eq!(erased.status, EntryStatus::Deleted);
    assert_eq!(
        core.my_feedback().unwrap()[0].status,
        EntryStatus::Deleted,
        "journal marks the row deleted"
    );

    // The server no longer serves it…
    assert!(core.fetch_annotations(TARGET).await.unwrap().is_empty());

    // …and the tombstone blocks re-ingestion of the same id: republishing the
    // exact same content (same key, target, timestamp → same dedup id)
    // answers 410 Gone.
    let err = core
        .publish_at(
            TARGET,
            Contribution::Comment {
                text: "delete me".into(),
            },
            None,
            T1,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("410"),
        "erased ids cannot resurrect: {err}"
    );
}

#[tokio::test]
async fn erase_with_a_missing_key_errors_cleanly() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let entry = core
        .publish_at(TARGET, Contribution::Stars { value: 1.0 }, None, T1)
        .await
        .unwrap();

    // The key file vanishes (new phone, no backup…).
    std::fs::remove_file(core.identity().path()).unwrap();

    let err = core.erase_entry_at(&entry.dedup_id, T2).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Identity(IdentityError::Missing(_))),
        "a fresh key must NOT be minted to sign an erasure: {err:?}"
    );
    // Nothing was deleted anywhere.
    assert_eq!(core.fetch_annotations(TARGET).await.unwrap().len(), 1);
    assert_eq!(core.my_feedback().unwrap()[0].status, EntryStatus::Active);
}

#[tokio::test]
async fn erase_with_a_different_key_is_refused_by_the_server() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core_at(dir.path(), &server);

    let entry = core
        .publish_at(TARGET, Contribution::Stars { value: 2.0 }, None, T1)
        .await
        .unwrap();

    // A stranger's key replaces ours (e.g. a bad import).
    let stranger = Identity::generate();
    core.import_identity(&stranger.to_pkcs8_pem().unwrap())
        .unwrap();

    let err = core.erase_entry_at(&entry.dedup_id, T2).await.unwrap_err();
    assert!(err.to_string().contains("403"), "not the owner: {err}");
    assert_eq!(core.fetch_annotations(TARGET).await.unwrap().len(), 1);
}

// --- persistence ----------------------------------------------------------------------

#[tokio::test]
async fn journal_and_settings_survive_reopen() {
    let server = spawn_server().await;
    let dir = tempfile::tempdir().unwrap();

    let entry = {
        let core = core_at(dir.path(), &server);
        core.publish_at(
            TARGET,
            Contribution::Tag {
                text: "keeper".into(),
            },
            None,
            T1,
        )
        .await
        .unwrap()
    };

    // Reopen the same data dir: journal row and server setting are back.
    let core = AppCore::open(dir.path()).unwrap();
    assert_eq!(core.settings().server_url, server);
    let journal = core.my_feedback().unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0], entry);
}

// --- pending share (the deep-link → webview bridge) ---------------------------------------

#[tokio::test]
async fn pending_share_is_drained_once() {
    let dir = tempfile::tempdir().unwrap();
    let core = AppCore::open(dir.path()).unwrap();

    assert_eq!(core.take_pending_share(), None);
    core.set_pending_share("3017620422003");
    assert_eq!(core.take_pending_share().as_deref(), Some("3017620422003"));
    assert_eq!(core.take_pending_share(), None, "drained");

    // The stored text is what a deep link carries; resolving it works.
    core.set_pending_share("freedback://share?text=97%38-0-306-40615-7");
    let text = core.take_pending_share().unwrap();
    let resolved = core.resolve_input(&text).unwrap();
    assert_eq!(resolved.uri(), "https://id.gs1.org/01/09780306406157");
}
