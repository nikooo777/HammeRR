use super::*;
use crate::recovery;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn recovery_restore_callback_rejects_review_from_before_peer_save_as() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut state = Session::new(Err("no assets needed".into()));
    state.configure_recovery(Ok(directory.path().join("recovery")));
    let request = state.begin_recovery_snapshot(Kind::Manual).unwrap();
    let entry = state
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap();
    let shared = Arc::new(Mutex::new(state));
    let document = crate::test_assets::document(shared.clone());
    let window = RecoveryWindow::new().unwrap();
    bind_with_access(
        &window,
        &shared,
        DocumentAccess::for_document(document.clone()),
    );
    refresh(&window, &lock(&shared));
    window.set_selected_id(entry.id.into());
    window.set_discard_changes(true);
    let review = window.get_map_review();
    let revision = lock(&shared).document().revision();
    let copy = directory.path().join("copy.vmf");
    lock(&shared).save(Actor::Agent, Some(&copy)).unwrap();
    assert_eq!(lock(&shared).document().revision(), revision);
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let saved = std::fs::read(&copy).unwrap();
    window.invoke_restore_snapshot();
    assert!(window.get_message().contains("Map changed"));
    assert_ne!(window.get_map_review(), review);
    assert!(!window.get_discard_changes());
    assert_eq!(lock(&shared).path(), Some(copy.as_path()));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(std::fs::read(&copy).unwrap(), saved);
    window.invoke_restore_snapshot();
    assert!(lock(&shared).path().is_none());
    assert!(lock(&shared).document().is_dirty());
}

#[test]
fn recovery_warning_keeps_error_domains_with_listing_failures_and_skipped_entries() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery");
    let authority = recovery::Authority::configured(Ok(path.clone()));
    let mut state = Session::with_recovery_settings(Err("no assets".into()), authority.clone());
    state.configure_recovery(Ok(path.clone()));
    let mut peer = Session::with_recovery_settings(Err("no assets".into()), authority);
    peer.configure_recovery(Ok(directory.path().join("peer")));
    std::fs::create_dir_all(path.join("settings.json")).unwrap();
    let settings_error = state
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap_err();
    let request = state.begin_recovery_snapshot(Kind::Manual).unwrap();
    state
        .finish_recovery_snapshot(Actor::Agent, &request, Err("snapshot write failed".into()))
        .unwrap_err();
    let source = state.document().vmf().clone();
    let history = state.history_snapshot();
    let shared = Arc::new(Mutex::new(state));
    let window = RecoveryWindow::new().unwrap();
    bind(&window, &shared);
    let moved = directory.path().join("moved");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::write(&path, b"not a directory").unwrap();
    refresh(&window, &lock(&shared));
    assert!(window.get_warning().contains(settings_error.as_str()));
    assert!(window.get_warning().contains("snapshot write failed"));
    assert!(window.get_warning().contains(path.to_str().unwrap()));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&moved, &path).unwrap();
    std::fs::write(
        path.join(format!("{}.json", recovery::unique_id())),
        b"broken",
    )
    .unwrap();
    refresh(&window, &lock(&shared));
    assert!(window.get_warning().contains(settings_error.as_str()));
    assert!(window.get_warning().contains("snapshot write failed"));
    assert!(window.get_warning().contains("incomplete or unreadable"));
    std::fs::remove_dir(path.join("settings.json")).unwrap();
    window.invoke_reload_settings();
    assert!(lock(&shared).recovery_status()["settings_error"].is_null());
    assert!(window.get_warning().contains("snapshot write failed"));
    window.set_interval_seconds(60);
    window.set_settings_dirty(true);
    window.invoke_apply_settings();
    assert!(lock(&shared).recovery_status()["settings_error"].is_null());
    assert!(window.get_warning().contains("snapshot write failed"));
    let peer_source = peer.document().vmf().clone();
    peer.update_recovery_settings(
        Actor::Agent,
        recovery::SettingsUpdate {
            keep_per_map: Some(8),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    window.set_interval_seconds(180);
    window.set_keep_count(12);
    window.set_autosave_enabled(false);
    window.set_settings_dirty(true);
    window.invoke_apply_settings();
    assert!(window.get_message().contains("changed elsewhere"));
    assert!(window.get_settings_dirty());
    assert_eq!(window.get_interval_seconds(), 180);
    assert_eq!(window.get_keep_count(), 12);
    assert_eq!(peer.recovery_settings().keep_per_map, 8);
    std::fs::write(path.join("settings.json"), b"broken").unwrap();
    window.invoke_reload_settings();
    assert!(window.get_settings_dirty());
    assert_eq!(window.get_interval_seconds(), 180);
    assert_eq!(window.get_keep_count(), 12);
    assert!(!window.get_autosave_enabled());
    assert!(window.get_message().contains("could not reload"));
    assert_eq!(
        peer.recovery_status()["settings_error"],
        lock(&shared).recovery_status()["settings_error"]
    );
    assert!(peer.recovery_status()["snapshot_error"].is_null());
    assert!(window.get_warning().contains("snapshot write failed"));
    window.show().unwrap();
    crate::ui::tests::save_ui_snapshot(window.window(), "recovery-settings-reload");
    let replacement = Settings {
        enabled: true,
        interval_seconds: 90,
        keep_per_map: 3,
    };
    std::fs::write(
        path.join("settings.json"),
        serde_json::to_vec(&replacement).unwrap(),
    )
    .unwrap();
    window.invoke_reload_settings();
    assert!(!window.get_settings_dirty());
    assert_eq!(window.get_interval_seconds(), 90);
    assert_eq!(window.get_keep_count(), 3);
    assert!(window.get_autosave_enabled());
    assert!(window.get_warning().contains("snapshot write failed"));
    assert_eq!(peer.recovery_settings(), replacement);
    assert!(peer.recovery_status()["settings_error"].is_null());
    assert_eq!(peer.document().vmf(), &peer_source);
    window.invoke_cancel();
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
}

fn admitted_fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
    RecoveryWindow,
    recovery::Authority,
    String,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let authority = recovery::Authority::configured(Ok(directory.path().join("settings")));
    let mut state =
        Session::with_recovery_settings(Err("no assets required".into()), authority.clone());
    state.configure_recovery(Ok(directory.path().join("recovery")));
    state
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let request = state.begin_recovery_snapshot(Kind::Manual).unwrap();
    let entry = state
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap();
    let shared = Arc::new(Mutex::new(state));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = RecoveryWindow::new().unwrap();
    bind_with_access(&window, &shared, access.clone());
    access
        .with_session(|session| {
            refresh(&window, session);
            Ok(())
        })
        .unwrap();
    (
        directory, document, shared, access, window, authority, entry.id,
    )
}

fn form(window: &RecoveryWindow) -> serde_json::Value {
    use slint::Model;
    let rows = window.get_rows();
    serde_json::json!({
        "selected":window.get_selected_id().to_string(), "detail":window.get_selected_detail().to_string(),
        "review":window.get_map_review().to_string(), "discard":window.get_discard_changes(),
        "confirm":window.get_confirm_delete(), "summary":window.get_summary().to_string(),
        "revision":window.get_settings_revision().to_string(), "dirty":window.get_settings_dirty(),
        "enabled":window.get_autosave_enabled(), "interval":window.get_interval_seconds(), "keep":window.get_keep_count(),
        "page":window.get_page(), "rows":(0..rows.row_count()).map(|index|rows.row_data(index).unwrap().id.to_string()).collect::<Vec<_>>()
    })
}

fn terminal_actions(window: &RecoveryWindow) {
    window.invoke_delete_snapshot();
    window.invoke_select_snapshot("never-restored".into());
    window.invoke_apply_settings();
    window.invoke_reload_settings();
    window.invoke_snapshot_now();
    window.invoke_refresh();
    window.invoke_restore_snapshot();
}

fn snapshots(store: &recovery::Store) -> Vec<(recovery::Entry, String)> {
    store
        .list()
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| {
            let text = store.read(&entry.id).unwrap().1;
            (entry, text)
        })
        .collect()
}

#[test]
fn recovery_dialog_keeps_pending_form_settings_snapshots_and_peer_through_real_drain() {
    let (directory, document, shared, access, window, authority, id) = admitted_fixture();
    window.invoke_snapshot_now();
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock(&shared).recovery_busy() {
        assert!(Instant::now() < deadline, "manual snapshot did not retire");
        std::thread::yield_now();
    }
    assert!(lock(&shared).recovery_entries().unwrap().entries.len() >= 2);
    window.invoke_refresh();
    window.set_interval_seconds(90);
    window.set_settings_dirty(true);
    window.invoke_apply_settings();
    assert!(!window.get_settings_dirty());
    assert_eq!(lock(&shared).recovery_settings().interval_seconds, 90);
    let mut peer =
        Session::with_recovery_settings(Err("peer assets unavailable".into()), authority);
    peer.configure_recovery(Ok(directory.path().join("peer-recovery")));
    peer.create_block(
        Actor::User,
        glam::DVec3::ZERO,
        glam::DVec3::splat(16.0),
        "test",
        false,
    )
    .unwrap();
    let peer_shared = Arc::new(Mutex::new(peer));
    let peer_document = crate::test_assets::document(peer_shared.clone());
    let peer_access = DocumentAccess::for_document(peer_document.clone());
    let peer_window = RecoveryWindow::new().unwrap();
    bind_with_access(&peer_window, &peer_shared, peer_access.clone());
    peer_access
        .with_session(|session| {
            refresh(&peer_window, session);
            Ok(())
        })
        .unwrap();
    let peer_source = lock(&peer_shared).document().vmf().clone();
    let peer_history = lock(&peer_shared).history_snapshot();
    window.set_current_map_only(false);
    window.invoke_select_snapshot(id.into());
    window.set_confirm_delete(true);
    window.set_discard_changes(true);
    window.set_interval_seconds(180);
    window.set_keep_count(12);
    window.set_autosave_enabled(false);
    window.set_settings_dirty(true);
    let expected_form = form(&window);
    let settings = lock(&shared).recovery_settings_snapshot();
    let settings_path = directory.path().join("settings/settings.json");
    let saved = std::fs::read(&settings_path).unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let store = recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let before = snapshots(&store);
    let gate = document.admit().unwrap();
    let draining = document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while document.lifecycle() == DocumentLifecycle::Live
        || lock(&shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline, "document did not begin draining");
        std::thread::yield_now();
    }
    window.show().unwrap();
    terminal_actions(&window);
    assert_eq!(window.get_message(), "the editor is closing");
    assert_eq!(form(&window), expected_form);
    assert!(window.window().is_visible());
    assert_eq!(snapshots(&store), before);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    let finished = snapshots(&store);
    terminal_actions(&window);
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(form(&window), expected_form);
    assert!(window.window().is_visible());
    assert_eq!(lock(&shared).recovery_settings_snapshot(), settings);
    assert_eq!(std::fs::read(&settings_path).unwrap(), saved);
    assert_eq!(snapshots(&store), finished);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(lock(&peer_shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer_shared).history_snapshot(), peer_history);
    peer_window.set_interval_seconds(150);
    peer_window.set_settings_dirty(true);
    peer_window.invoke_apply_settings();
    assert_eq!(lock(&peer_shared).recovery_settings().interval_seconds, 150);
    assert_eq!(peer_document.lifecycle(), DocumentLifecycle::Live);
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    window.show().unwrap();
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!window.window().is_visible());
    drop(access);
}

#[test]
fn retained_recovery_window_does_not_retain_session_or_runtime_through_restore_callback() {
    let (_directory, document, shared, access, window, _authority, _id) = admitted_fixture();
    let weak = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    window.invoke_restore_snapshot();
    window.invoke_snapshot_now();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_cancel();
    assert!(!window.window().is_visible());
}

#[test]
fn recovery_failed_terminal_preserves_pending_form_snapshot_bytes_and_exact_error() {
    let (directory, document, shared, _access, window, _authority, id) = admitted_fixture();
    window.set_current_map_only(false);
    window.invoke_select_snapshot(id.into());
    window.set_confirm_delete(true);
    window.set_discard_changes(true);
    window.set_interval_seconds(180);
    window.set_settings_dirty(true);
    let expected_form = form(&window);
    let settings = lock(&shared).recovery_settings_snapshot();
    let recovery = directory.path().join("recovery");
    let moved = directory.path().join("retained-recovery");
    let retained = recovery::Store {
        directory: moved.clone(),
    };
    std::fs::rename(&recovery, &moved).unwrap();
    let before = snapshots(&retained);
    std::fs::write(&recovery, b"not a directory").unwrap();
    lock(&shared)
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(32.0),
            glam::DVec3::splat(48.0),
            "changed",
            false,
        )
        .unwrap();
    let error = document.shutdown().unwrap_err();
    terminal_actions(&window);
    assert_eq!(
        window.get_message(),
        format!("document shutdown failed: {error}")
    );
    assert_eq!(form(&window), expected_form);
    assert_eq!(lock(&shared).recovery_settings_snapshot(), settings);
    assert_eq!(snapshots(&retained), before);
    window.invoke_cancel();
    std::fs::remove_file(&recovery).unwrap();
    std::fs::rename(&moved, &recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(
        snapshots(&recovery::Store {
            directory: recovery
        }),
        before
    );
}
