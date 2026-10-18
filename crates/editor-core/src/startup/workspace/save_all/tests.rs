use super::*;
use crate::startup::Application;
use glam::DVec3;
use map_core::ObjectId;
use std::fs;
use std::time::{Duration, Instant};

fn fixture(
    directory: &Path,
    named_peer: bool,
) -> (Application, Workspace, WorkspaceDocument, WorkspaceDocument) {
    let application = Application::isolated(
        Err("No content required".into()),
        directory.join("recovery"),
    )
    .unwrap();
    let workspace = application.workspace();
    let first = workspace.resolve(None).unwrap();
    let second = workspace
        .prepare_new(Actor::User)
        .unwrap()
        .publish(DocumentVisibility::Headless)
        .unwrap();
    lock(first.document.session())
        .save(Actor::User, Some(&directory.join("first.vmf")))
        .unwrap();
    if named_peer {
        lock(second.document.session())
            .save(Actor::User, Some(&directory.join("second.vmf")))
            .unwrap();
    }
    (application, workspace, first, second)
}

fn edit(entry: &WorkspaceDocument, x: f64) -> ObjectId {
    lock(entry.document.session())
        .create_block(
            Actor::User,
            DVec3::new(x, 0.0, 0.0),
            DVec3::new(x + 16.0, 16.0, 16.0),
            "test",
            false,
        )
        .unwrap()
}

fn row<'a>(report: &'a Value, entry: &WorkspaceDocument) -> &'a Value {
    report["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["document_id"] == entry.selector())
        .unwrap()
}

#[test]
fn two_named_maps_save_once_and_clean_retry_verifies_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    let (mut application, workspace, first, second) = fixture(directory.path(), true);
    edit(&first, 0.0);
    edit(&second, 64.0);
    let settings = lock(second.document.session()).compile_settings();
    let history = lock(second.document.session()).history_snapshot();
    let report = workspace.prepare_save_all_named(Actor::User).unwrap().run();
    assert_eq!(report["complete"], true);
    assert_eq!(report["saved"], 2);
    for entry in [&first, &second] {
        assert_eq!(row(&report, entry)["status"], "saved");
        assert!(!lock(entry.document.session()).document().is_dirty());
    }
    assert_eq!(lock(second.document.session()).compile_settings(), settings);
    assert_eq!(
        lock(second.document.session())
            .document()
            .undo_labels()
            .count(),
        1
    );
    assert_ne!(lock(second.document.session()).history_snapshot(), history);
    let paths = [
        directory.path().join("first.vmf"),
        directory.path().join("second.vmf"),
    ];
    let bytes = paths
        .iter()
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let stamps = paths
        .iter()
        .map(|path| fs::metadata(path).unwrap().modified().unwrap())
        .collect::<Vec<_>>();
    let report = workspace
        .prepare_save_all_named(Actor::Agent)
        .unwrap()
        .run();
    assert_eq!(report["complete"], true);
    assert_eq!(report["saved"], 0);
    assert_eq!(report["unchanged"], 2);
    for (index, path) in paths.iter().enumerate() {
        assert_eq!(fs::read(path).unwrap(), bytes[index]);
        assert_eq!(
            fs::metadata(path).unwrap().modified().unwrap(),
            stamps[index]
        );
    }
    application.shutdown().unwrap();
}

#[test]
fn partial_failure_keeps_failed_dirty_source_and_successful_peer_with_exact_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let (mut application, workspace, first, second) = fixture(directory.path(), true);
    edit(&first, 0.0);
    edit(&second, 64.0);
    let second_path = directory.path().join("second.vmf");
    let history = lock(second.document.session()).history_snapshot();
    fs::write(&second_path, b"externally changed source").unwrap();
    let report = workspace.prepare_save_all_named(Actor::User).unwrap().run();
    assert_eq!(report["complete"], false);
    assert_eq!(report["status"], "not_complete");
    assert_eq!(report["saved"], 1);
    assert_eq!(row(&report, &first)["status"], "saved");
    assert_eq!(row(&report, &second)["status"], "error");
    assert!(lock(second.document.session()).document().is_dirty());
    assert_eq!(lock(second.document.session()).history_snapshot(), history);
    assert_eq!(
        fs::read(&second_path).unwrap(),
        b"externally changed source"
    );
    assert!(
        lock(first.document.session())
            .save(Actor::Agent, Some(&second_path))
            .unwrap_err()
            .contains("conflicts")
    );
    application.shutdown().unwrap();
}

#[test]
fn unnamed_dirty_and_pending_property_and_geometry_reviews_remain_visible_and_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let (mut application, workspace, first, second) = fixture(directory.path(), false);
    let block = edit(&first, 0.0);
    edit(&second, 64.0);
    {
        let mut session = lock(first.document.session());
        session
            .preview_transform(
                Actor::User,
                &[block],
                map_core::edit::Transform {
                    translation: DVec3::X * 32.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
    }
    let source = lock(first.document.session()).document().vmf().clone();
    let preview = lock(first.document.session())
        .render_document()
        .vmf()
        .clone();
    let draft = json!(lock(first.document.session()).property_draft());
    let original_file = fs::read(directory.path().join("first.vmf")).unwrap();
    let report = workspace.prepare_save_all_named(Actor::User).unwrap().run();
    assert_eq!(report["complete"], false);
    assert_eq!(report["saved"], 0);
    assert_eq!(row(&report, &first)["status"], "pending_review");
    assert!(
        row(&report, &first)["pending_reviews"]
            .as_array()
            .unwrap()
            .contains(&json!("properties"))
    );
    assert!(
        row(&report, &first)["pending_reviews"]
            .as_array()
            .unwrap()
            .contains(&json!("transform"))
    );
    assert_eq!(row(&report, &second)["status"], "needs_save_as");
    assert_eq!(row(&report, &second)["needs_save_as"], true);
    assert_eq!(lock(first.document.session()).document().vmf(), &source);
    assert_eq!(
        lock(first.document.session()).render_document().vmf(),
        &preview
    );
    assert_eq!(
        json!(lock(first.document.session()).property_draft()),
        draft
    );
    assert_eq!(
        fs::read(directory.path().join("first.vmf")).unwrap(),
        original_file
    );
    assert!(!directory.path().join("second.vmf").exists());
    application.shutdown().unwrap();
}

#[test]
fn fixed_capture_rejects_replacement_and_busy_peer_without_rerouting_or_retry() {
    let directory = tempfile::tempdir().unwrap();
    let (mut application, workspace, first, second) = fixture(directory.path(), true);
    edit(&first, 0.0);
    edit(&second, 64.0);
    let request = workspace.prepare_save_all_named(Actor::User).unwrap();
    assert!(workspace.busy());
    assert_eq!(first.document.active_operations(), 1);
    assert!(workspace.reserve_close(first.selector()).is_err());
    lock(second.document.session())
        .new_map(Actor::User, true)
        .unwrap();
    let report = request.run();
    assert_eq!(row(&report, &first)["status"], "saved");
    assert_eq!(row(&report, &second)["status"], "stale");
    assert!(lock(second.document.session()).path().is_none());
    assert!(!workspace.busy());
    assert_eq!(first.document.active_operations(), 0);
    let peer_lock = lock(second.document.session());
    let request = workspace.prepare_save_all_named(Actor::Agent).unwrap();
    let report = request.run();
    assert_eq!(row(&report, &second)["status"], "busy");
    assert_eq!(report["complete"], false);
    drop(peer_lock);
    let report = workspace.prepare_save_all_named(Actor::User).unwrap().run();
    assert_eq!(row(&report, &second)["status"], "unnamed_clean");
    assert_eq!(report["complete"], true);
    assert_eq!(report["saved"], 0);
    application.shutdown().unwrap();
}

#[test]
fn clean_external_change_is_an_error_and_raw_shutdown_waits_for_request_retirement() {
    let directory = tempfile::tempdir().unwrap();
    let (mut application, workspace, first, second) = fixture(directory.path(), true);
    fs::write(directory.path().join("first.vmf"), b"external change").unwrap();
    let report = workspace.prepare_save_all_named(Actor::User).unwrap().run();
    assert_eq!(row(&report, &first)["status"], "error");
    assert_eq!(row(&report, &second)["status"], "unchanged");
    let request = workspace.prepare_save_all_named(Actor::User).unwrap();
    let closing = second.document();
    let shutdown = std::thread::spawn(move || closing.shutdown());
    let deadline = Instant::now() + Duration::from_secs(5);
    while second.document.lifecycle() == DocumentLifecycle::Live
        || lock(second.document.session())
            .document_close_review()
            .inspection()["closing"]
            != true
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let report = request.run();
    assert_eq!(row(&report, &second)["status"], "closing");
    shutdown.join().unwrap().unwrap();
    assert_eq!(second.document.active_operations(), 0);
    application.shutdown().unwrap();
}
