use super::*;
use std::sync::mpsc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

fn application(directory: &Path) -> Application {
    Application::isolated(Err("No content needed".into()), directory.join("recovery")).unwrap()
}

fn edit(document: &DocumentHandle) {
    lock(document.session())
        .create_block(
            Actor::Agent,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
}

#[test]
fn factory_shares_content_and_settings_while_retaining_independent_documents() {
    let directory = tempfile::tempdir().unwrap();
    let mut application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let a = first.document();
    lock(a.session()).set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass = local_class [ ]").unwrap(),
    );
    let second = workspace
        .prepare_new(Actor::Agent)
        .unwrap()
        .publish(DocumentVisibility::Headless)
        .unwrap();
    let b = second.document();
    assert_ne!(first.selector(), second.selector());
    assert_ne!(a.id(), b.id());
    let a_game = lock(a.session()).game() as *const GameFs;
    let b_game = lock(b.session()).game() as *const GameFs;
    assert!(std::ptr::eq(a_game, b_game));
    assert_eq!(lock(a.session()).game_error(), Some("No content needed"));
    assert_eq!(lock(b.session()).game_error(), Some("No content needed"));
    assert!(
        lock(b.session())
            .definitions()
            .class("local_class")
            .is_none()
    );
    assert!(workspace.resolve(None).unwrap_err().contains("document_id"));
    assert!(!workspace.can_create());
    assert!(
        workspace
            .prepare_new(Actor::Agent)
            .err()
            .unwrap()
            .contains("full")
    );
    let before = lock(b.session()).history_snapshot();
    edit(&a);
    assert_eq!(lock(b.session()).history_snapshot(), before);
    lock(a.session())
        .favorite_material("dev/shared", true)
        .unwrap();
    assert!(lock(b.session()).is_material_favorite("dev/shared"));
    assert_eq!(
        workspace
            .resolve(Some(first.selector()))
            .unwrap()
            .document()
            .id(),
        a.id()
    );
    application.shutdown().unwrap();
    assert_eq!(a.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(b.lifecycle(), DocumentLifecycle::Finished);
    assert!(workspace.resolve(Some(second.selector())).is_err());
    drop(application);
    assert!(workspace.services.upgrade().is_none());
    assert!(workspace.prepare_new(Actor::Agent).is_err());
}

#[test]
fn abandoned_and_failed_candidates_release_only_their_own_membership_and_claims() {
    let directory = tempfile::tempdir().unwrap();
    let application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let initial = first.document();
    let before = lock(initial.session()).history_snapshot();
    let candidate = workspace.prepare_new(Actor::Agent).unwrap();
    assert!(workspace.busy());
    assert!(workspace.reserve_close(first.selector()).is_err());
    assert!(workspace.prepare_new(Actor::Agent).is_err());
    let abandoned = candidate.document();
    drop(candidate);
    assert_eq!(abandoned.lifecycle(), DocumentLifecycle::Finished);
    assert!(!workspace.busy());
    let malformed = directory.path().join("broken.vmf");
    std::fs::write(&malformed, "world { broken").unwrap();
    assert!(workspace.prepare_open(Actor::Agent, &malformed).is_err());
    assert_eq!(workspace.documents().len(), 1);
    assert_eq!(lock(initial.session()).history_snapshot(), before);
    let source = directory.path().join("candidate.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    let candidate = workspace.prepare_open(Actor::Agent, &source).unwrap();
    assert!(
        lock(initial.session())
            .save(Actor::Agent, Some(&source))
            .is_err()
    );
    let candidate_selector = candidate.selector().to_owned();
    drop(candidate);
    lock(initial.session())
        .save(Actor::Agent, Some(&source))
        .unwrap();
    assert!(workspace.resolve(Some(&candidate_selector)).is_err());
    assert_eq!(workspace.documents().len(), 1);
}

#[test]
fn close_planning_preserves_live_and_prevents_an_inaccessible_hidden_peer() {
    let directory = tempfile::tempdir().unwrap();
    let application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    workspace
        .set_visibility(first.document().id(), DocumentVisibility::Visible)
        .unwrap();
    let second = workspace
        .prepare_new(Actor::Agent)
        .unwrap()
        .publish(DocumentVisibility::Hidden)
        .unwrap();
    assert!(
        workspace
            .reserve_close(first.selector())
            .err()
            .unwrap()
            .contains("Show")
    );
    workspace
        .set_visibility(second.document().id(), DocumentVisibility::Visible)
        .unwrap();
    let reservation = workspace.reserve_close(first.selector()).unwrap();
    assert_eq!(reservation.disposition(), CloseDisposition::DocumentOnly);
    reservation.validate().unwrap();
    assert!(workspace.busy());
    assert!(
        workspace
            .set_visibility(second.document().id(), DocumentVisibility::Hidden)
            .is_err()
    );
    drop(reservation);
    assert!(!workspace.busy());
    assert_eq!(first.document().lifecycle(), DocumentLifecycle::Live);
    assert_eq!(
        workspace.resolve(Some(first.selector())).unwrap().state(),
        WorkspaceDocumentState::Live
    );
}

#[test]
fn candidate_restores_build_context_without_repairing_or_publishing_recent_history() {
    let directory = tempfile::tempdir().unwrap();
    let application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap().document();
    let source = directory.path().join("remembered.vmf");
    lock(first.session())
        .save(Actor::Agent, Some(&source))
        .unwrap();
    let saved_options = {
        let mut session = lock(first.session());
        session
            .update_build_settings(
                Actor::Agent,
                crate::session::compilation::BuildSettingsUpdate {
                    lighting: Some("direct_ldr"),
                    ..Default::default()
                },
            )
            .unwrap();
        let options = session.compile_options();
        session.new_map(Actor::Agent, true).unwrap();
        options
    };
    application
        .recent_maps
        .report_error("Keep the existing history warning".into());
    let before = application.recent_maps.snapshot();
    let history_file = before
        .directory
        .as_ref()
        .unwrap()
        .join(crate::recent_maps::FILE_NAME);
    let bytes = std::fs::read(&history_file).unwrap();
    let candidate = workspace.prepare_open(Actor::Agent, &source).unwrap();
    assert_eq!(
        lock(candidate.document().session()).compile_options(),
        saved_options
    );
    let after = application.recent_maps.snapshot();
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.error, before.error);
    assert_eq!(after.serial, before.serial);
    assert_eq!(std::fs::read(&history_file).unwrap(), bytes);
    drop(candidate);
    assert_eq!(application.recent_maps.snapshot().serial, before.serial);
}

#[test]
fn unconfirmed_candidate_retirement_keeps_failed_capacity_and_claims() {
    let directory = tempfile::tempdir().unwrap();
    let mut application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap().document();
    let source = directory.path().join("candidate.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    let prepared = workspace.prepare_open(Actor::Agent, &source).unwrap();
    let candidate = prepared.document();
    let failed = prepared
        .fail(
            "UI retirement was not confirmed".into(),
            DocumentVisibility::Hidden,
        )
        .unwrap();
    assert_eq!(candidate.lifecycle(), DocumentLifecycle::Finished);
    assert!(candidate.admit().is_err());
    assert_eq!(failed.state(), WorkspaceDocumentState::Failed);
    assert_eq!(failed.error(), Some("UI retirement was not confirmed"));
    assert!(!workspace.can_create());
    assert!(!workspace.busy());
    assert!(
        lock(first.session())
            .save(Actor::Agent, Some(&source))
            .is_err()
    );
    assert!(application.recent_maps.snapshot().entries.is_empty());
    assert!(
        application
            .shutdown()
            .unwrap_err()
            .contains("UI retirement was not confirmed")
    );
    assert_eq!(first.lifecycle(), DocumentLifecycle::Finished);
}

#[test]
fn application_shutdown_waits_for_candidate_retirement_and_rejects_late_publish() {
    let directory = tempfile::tempdir().unwrap();
    let mut application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap().document();
    edit(&first);
    let prepared = workspace.prepare_new(Actor::Agent).unwrap();
    let candidate = prepared.document();
    let (sealed, observed) = mpsc::sync_channel(1);
    let (finished, completion) = mpsc::sync_channel(1);
    let closing = workspace.clone();
    let worker = std::thread::spawn(move || {
        closing.close_admission();
        sealed.send(()).unwrap();
        finished.send(application.shutdown()).unwrap();
    });
    observed.recv_timeout(TIMEOUT).unwrap();
    assert!(workspace.resolve(None).is_err());
    assert!(matches!(
        completion.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(prepared.publish(DocumentVisibility::Headless).is_err());
    completion.recv_timeout(TIMEOUT).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(candidate.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(first.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(
        lock(first.session()).recovery_status()["pending_changes"],
        false
    );
}

#[cfg(feature = "mcp")]
fn accept(workspace: &Workspace, entry: &WorkspaceDocument) -> CloseReservation {
    let mut reservation = workspace.reserve_close(entry.selector()).unwrap();
    let document = reservation.document();
    let review = document.document_close_review().unwrap();
    document
        .commit_reviewed_close_with(&review, true, || {
            reservation.validate()?;
            reservation.commit();
            Ok(())
        })
        .unwrap();
    reservation
}

#[cfg(feature = "mcp")]
#[test]
fn terminal_membership_is_sealed_only_after_reviewed_commit() {
    let directory = tempfile::tempdir().unwrap();
    let application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let document = first.document();
    {
        let reservation = workspace.reserve_close(first.selector()).unwrap();
        assert_eq!(reservation.disposition(), CloseDisposition::Terminal);
        let review = document.document_close_review().unwrap();
        assert_eq!(
            document.commit_reviewed_close_with(&review, false, || {
                reservation.validate()?;
                Err::<(), _>("output unavailable".into())
            }),
            Err("output unavailable".into())
        );
    }
    assert!(workspace.can_create());
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    let mut close = accept(&workspace, &first);
    assert!(!workspace.can_create());
    assert!(workspace.prepare_new(Actor::Agent).is_err());
    document.shutdown().unwrap();
    close.finish(Ok(())).unwrap();
    assert!(workspace.documents().is_empty());
    assert!(!workspace.can_create());
    assert!(workspace.prepare_new(Actor::Agent).is_err());
}

#[cfg(feature = "mcp")]
#[test]
fn close_failure_is_sticky_and_application_still_drains_the_peer() {
    let directory = tempfile::tempdir().unwrap();
    let mut application = application(directory.path());
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let second = workspace
        .prepare_new(Actor::Agent)
        .unwrap()
        .publish(DocumentVisibility::Headless)
        .unwrap();
    let a = first.document();
    let b = second.document();
    let source = directory.path().join("owned.vmf");
    lock(a.session()).save(Actor::Agent, Some(&source)).unwrap();
    edit(&a);
    edit(&b);
    let broken = directory.path().join("broken-recovery");
    std::fs::write(&broken, b"not a directory").unwrap();
    lock(a.session()).configure_recovery(Ok(broken.clone()));
    let mut close = accept(&workspace, &first);
    let error = a.shutdown().unwrap_err();
    assert_eq!(close.finish(Err(error.clone())), Err(error.clone()));
    std::fs::remove_file(&broken).unwrap();
    std::fs::create_dir(&broken).unwrap();
    assert_eq!(close.finish(Ok(())), Err(error.clone()));
    assert_eq!(a.shutdown(), Err(error.clone()));
    assert_eq!(
        workspace.documents()[0].state(),
        WorkspaceDocumentState::Failed
    );
    assert!(workspace.resolve(Some(first.selector())).is_err());
    assert!(lock(b.session()).save(Actor::Agent, Some(&source)).is_err());
    assert!(application.shutdown().unwrap_err().contains(&error));
    assert_eq!(b.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(
        lock(b.session()).recovery_status()["pending_changes"],
        false
    );
}

#[cfg(feature = "mcp")]
#[test]
fn successful_close_releases_retained_source_bsp_and_lin_with_live_old_handles() {
    let (directory, game, _) = crate::test_assets::game_process(false);
    let application = Application::isolated(Ok(game), directory.path().join("recovery")).unwrap();
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let second = workspace
        .prepare_new(Actor::Agent)
        .unwrap()
        .publish(DocumentVisibility::Headless)
        .unwrap();
    let a = first.document();
    let b = second.document();
    let source = directory.path().join("room.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    lock(a.session()).open(Actor::Agent, &source, true).unwrap();
    let bsp = directory.path().join("successful.bsp");
    let request = lock(a.session())
        .begin_compile(Actor::Agent, Some(&bsp))
        .unwrap();
    let result = request.run();
    assert!(result.as_ref().unwrap().published());
    lock(a.session()).finish_compile(Actor::Agent, &result);
    drop(result);
    let wall = lock(a.session())
        .document()
        .vmf()
        .world
        .solids
        .last()
        .unwrap()
        .id;
    lock(a.session())
        .delete_objects(Actor::Agent, &[map_core::ObjectId::Solid(wall)])
        .unwrap();
    let leaking = directory.path().join("leaking.bsp");
    let request = lock(a.session())
        .begin_compile(Actor::Agent, Some(&leaking))
        .unwrap();
    let result = request.run();
    assert!(result.as_ref().unwrap().output.leak.is_some());
    lock(a.session()).finish_compile(Actor::Agent, &result);
    drop(result);
    let lin = leaking.with_extension("lin");
    let old_server = crate::mcp::EditorServer::for_document(a.clone(), None);
    let mut close = accept(&workspace, &first);
    a.shutdown().unwrap();
    for destination in [&source, &bsp, &lin] {
        assert!(
            lock(b.session())
                .save(Actor::Agent, Some(destination))
                .is_err()
        );
    }
    close.finish(Ok(())).unwrap();
    close
        .finish(Err("must not replace success".into()))
        .unwrap();
    for destination in [&source, &bsp, &lin] {
        lock(b.session())
            .save(Actor::Agent, Some(destination))
            .unwrap();
    }
    assert!(a.admit().is_err());
    assert!(workspace.resolve(Some(first.selector())).is_err());
    assert_eq!(workspace.resolve(None).unwrap().document().id(), b.id());
    let reopened = workspace
        .prepare_open(Actor::Agent, &source)
        .unwrap()
        .publish(DocumentVisibility::Headless)
        .unwrap();
    assert_ne!(reopened.selector(), first.selector());
    assert_ne!(reopened.document().id(), a.id());
    drop(old_server);
}
