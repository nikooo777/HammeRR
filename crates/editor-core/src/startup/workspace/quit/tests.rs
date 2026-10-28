use super::*;
use crate::session::{PropertyChange, PropertyTarget};

fn fixture() -> (
    tempfile::TempDir,
    Application,
    WorkspaceDocument,
    WorkspaceDocument,
) {
    let directory = tempfile::tempdir().unwrap();
    let application = Application::isolated(
        Err("No content needed".into()),
        directory.path().join("recovery"),
    )
    .unwrap();
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    workspace
        .set_visibility(first.document.id(), DocumentVisibility::Visible)
        .unwrap();
    let second = workspace
        .prepare_new(Actor::Agent)
        .unwrap()
        .publish(DocumentVisibility::Hidden)
        .unwrap();
    (directory, application, first, second)
}

fn assert_live(workspace: &Workspace, entries: &[&WorkspaceDocument]) {
    assert!(!workspace.busy());
    for entry in entries {
        assert_eq!(entry.document.lifecycle(), DocumentLifecycle::Live);
        assert_eq!(
            workspace.resolve(Some(entry.selector())).unwrap().state(),
            WorkspaceDocumentState::Live
        );
        assert_eq!(
            lock(entry.document.session())
                .document_close_review()
                .inspection()["closing"],
            false
        );
    }
}

#[test]
fn complete_review_requires_consent_for_hidden_drafts_and_freezes_every_document_together() {
    let (_directory, application, first, second) = fixture();
    let workspace = application.workspace();
    lock(second.document.session())
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    let token = lock(second.document.session())
        .property_draft()
        .unwrap()
        .token;
    let review = workspace.quit_review().unwrap();
    assert!(review.requires_discard());
    let report = review.inspection();
    assert_eq!(report["documents"].as_array().unwrap().len(), 2);
    assert_eq!(report["documents"][1]["visibility"], "hidden");
    assert_eq!(
        report["documents"][1]["close"]["drafts"],
        json!(["properties"])
    );
    assert!(workspace.commit_quit(&review, false, || Ok(())).is_err());
    assert_live(&workspace, &[&first, &second]);
    assert_eq!(
        lock(second.document.session())
            .property_draft()
            .unwrap()
            .token,
        token
    );
    let (mut accepted, ()) = workspace.commit_quit(&review, true, || Ok(())).unwrap();
    assert!(workspace.busy());
    assert!(workspace.prepare_new(Actor::Agent).is_err());
    assert!(
        workspace
            .set_visibility(second.document.id(), DocumentVisibility::Visible)
            .is_err()
    );
    assert!(workspace.commit_quit(&review, true, || Ok(())).is_err());
    for entry in [&first, &second] {
        assert_eq!(entry.document.lifecycle(), DocumentLifecycle::Draining);
        assert!(entry.document.admit().is_err());
        assert!(
            lock(entry.document.session())
                .save(Actor::Agent, None)
                .is_err()
        );
        entry.document.shutdown().unwrap();
        accepted
            .finish_document(entry.document.id(), Ok(()))
            .unwrap();
    }
    accepted.finish();
    assert!(workspace.documents().is_empty());
    assert!(!workspace.busy());
}

#[test]
fn changed_hidden_draft_visibility_and_workspace_identity_reject_without_partial_close() {
    let (_directory, application, first, second) = fixture();
    let workspace = application.workspace();
    lock(second.document.session())
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    let token = lock(second.document.session())
        .property_draft()
        .unwrap()
        .token;
    let review = workspace.quit_review().unwrap();
    lock(second.document.session())
        .update_property_edit(
            token,
            vec![PropertyChange::Set {
                key: "message".into(),
                value: "edited after review".into(),
            }],
        )
        .unwrap();
    assert!(
        workspace
            .commit_quit(&review, true, || Ok(()))
            .err()
            .unwrap()
            .contains("changed")
    );
    assert_live(&workspace, &[&first, &second]);
    let review = workspace.quit_review().unwrap();
    workspace
        .set_visibility(second.document.id(), DocumentVisibility::Visible)
        .unwrap();
    assert!(workspace.commit_quit(&review, true, || Ok(())).is_err());
    assert_live(&workspace, &[&first, &second]);
    let (_other_directory, other, _, _) = fixture();
    let foreign = other.workspace().quit_review().unwrap();
    assert!(workspace.commit_quit(&foreign, true, || Ok(())).is_err());
    assert_live(&workspace, &[&first, &second]);
}

#[test]
fn active_admission_and_output_reservation_failure_leave_all_maps_and_claims_live() {
    let (directory, application, first, second) = fixture();
    let workspace = application.workspace();
    let source = directory.path().join("first.vmf");
    lock(first.document.session())
        .save(Actor::User, Some(&source))
        .unwrap();
    let operation = second.document.admit().unwrap();
    let review = workspace.quit_review().unwrap();
    let reserved = std::cell::Cell::new(false);
    assert!(
        workspace
            .commit_quit(&review, true, || {
                reserved.set(true);
                Ok(())
            })
            .err()
            .unwrap()
            .contains("busy")
    );
    assert!(!reserved.get());
    assert_live(&workspace, &[&first, &second]);
    drop(operation);
    let review = workspace.quit_review().unwrap();
    let failed = workspace.commit_quit(&review, true, || {
        Err::<(), _>("injected output reservation failure".to_owned())
    });
    assert!(failed.err().unwrap().contains("output reservation"));
    assert_live(&workspace, &[&first, &second]);
    assert!(
        lock(second.document.session())
            .save(Actor::Agent, Some(&source))
            .is_err()
    );
    let candidate = workspace.services.upgrade().unwrap().document().unwrap();
    assert!(
        lock(candidate.session())
            .open_workspace_candidate(Actor::Agent, &source)
            .is_err()
    );
    candidate.shutdown().unwrap();
}

#[test]
fn failed_physical_cleanup_retains_failed_entry_and_original_source_claim() {
    let (directory, application, first, second) = fixture();
    let workspace = application.workspace();
    let source = directory.path().join("first.vmf");
    let broken = directory.path().join("broken-recovery");
    std::fs::write(&broken, "not a directory").unwrap();
    {
        let mut session = lock(first.document.session());
        session.save(Actor::User, Some(&source)).unwrap();
        session
            .create_block(
                Actor::User,
                glam::DVec3::ZERO,
                glam::DVec3::splat(16.0),
                "test",
                false,
            )
            .unwrap();
        session.configure_recovery(Ok(broken));
    }
    let review = workspace.quit_review().unwrap();
    let (mut accepted, ()) = workspace.commit_quit(&review, true, || Ok(())).unwrap();
    let failed = first.document.shutdown();
    assert!(failed.is_err());
    assert!(
        accepted
            .finish_document(first.document.id(), failed)
            .is_err()
    );
    second.document.shutdown().unwrap();
    accepted
        .finish_document(second.document.id(), Ok(()))
        .unwrap();
    accepted.finish();
    let remaining = workspace.documents();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].state(), WorkspaceDocumentState::Failed);
    assert!(remaining[0].error().is_some());
    let candidate = workspace.services.upgrade().unwrap().document().unwrap();
    assert!(
        lock(candidate.session())
            .open_workspace_candidate(Actor::Agent, &source)
            .is_err()
    );
    candidate.shutdown().unwrap();
}

#[test]
fn concurrent_acceptance_has_one_owner_and_rejects_membership_changes_until_cleanup() {
    let (_directory, application, first, second) = fixture();
    let workspace = application.workspace();
    let review = workspace.quit_review().unwrap();
    let pending_close = workspace.reserve_close(second.selector()).unwrap();
    assert!(workspace.quit_review().is_err());
    assert!(workspace.commit_quit(&review, false, || Ok(())).is_err());
    drop(pending_close);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let accepted = std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..2 {
            let (workspace, review, barrier) = (workspace.clone(), review.clone(), barrier.clone());
            threads.push(scope.spawn(move || {
                barrier.wait();
                workspace
                    .commit_quit(&review, false, || Ok(()))
                    .map(|(reservation, ())| reservation)
            }));
        }
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(accepted.iter().filter(|result| result.is_ok()).count(), 1);
    let mut accepted = accepted.into_iter().find_map(Result::ok).unwrap();
    assert!(workspace.busy());
    for entry in [&first, &second] {
        assert_eq!(entry.document.lifecycle(), DocumentLifecycle::Draining);
        entry.document.shutdown().unwrap();
        accepted
            .finish_document(entry.document.id(), Ok(()))
            .unwrap();
    }
    accepted.finish();
    assert!(workspace.documents().is_empty());
}
