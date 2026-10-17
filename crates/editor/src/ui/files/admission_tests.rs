use super::*;
use crate::runtime::DocumentLifecycle;
use std::sync::{Arc, Mutex};

fn software_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

fn fixture() -> (
    tempfile::TempDir,
    DocumentHandle,
    SharedSession,
    DocumentAccess,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    (directory, document, shared, access)
}

#[test]
fn file_completion_abandons_missing_window_while_document_and_peer_remain_live() {
    software_backend();
    let (directory, document, shared, access) = fixture();
    let (_peer_directory, peer, _peer_shared, _peer_access) = fixture();
    let app = AppWindow::new().unwrap();
    let weak = app.as_weak();
    let continuation = access.downgrade();
    let generation = lock(&shared).document_generation();
    let path = directory.path().join("abandoned.vmf");
    let before = lock(&shared).history_snapshot();
    drop(app);
    complete_file_action(&weak, &continuation, |session| {
        session
            .save_for_generation(Actor::User, Some(&path), Some(generation))
            .map(|path| path.display().to_string())
    });
    assert!(!path.exists());
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
    peer.shutdown().unwrap();
}

#[test]
fn retained_file_completions_and_compile_reject_terminal_document_without_retargeting_peer() {
    software_backend();
    let (directory, document, shared, access) = fixture();
    let (_peer_directory, peer, peer_shared, _peer_access) = fixture();
    let peer_before = lock(&peer_shared).history_snapshot();
    let app = AppWindow::new().unwrap();
    let continuation = access.downgrade();
    let generation = lock(&shared).document_generation();
    let review = lock(&shared).buffer_replacement_review();
    let path = directory.path().join("late.vmf");
    document.shutdown().unwrap();
    let before = lock(&shared).history_snapshot();
    complete_file_action(&app.as_weak(), &continuation, |session| {
        session
            .save_for_generation(Actor::User, Some(&path), Some(generation))
            .map(|path| path.display().to_string())
    });
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!path.exists());
    complete_file_action(&app.as_weak(), &continuation, |session| {
        session
            .new_map_reviewed(Actor::User, true, &review)
            .map(|()| "New map".into())
    });
    assert_eq!(app.get_message(), "the document is closed");
    start_compile(
        &app,
        &access,
        Some(&directory.path().join("late.bsp")),
        false,
        generation,
    );
    assert_eq!(app.get_message(), "the document is closed");
    assert!(!lock(&shared).compiling());
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(lock(&peer_shared).history_snapshot(), peer_before);
    assert_eq!(peer.lifecycle(), DocumentLifecycle::Live);
    peer.shutdown().unwrap();
}

#[test]
fn recent_open_and_recovery_restore_callbacks_reject_matching_reviews_after_drain() {
    software_backend();
    let (directory, document, shared, access) = fixture();
    let source = directory.path().join("source.vmf");
    access
        .with_session(|session| session.save(Actor::User, Some(&source)))
        .unwrap();
    let entry = recovery::capture(&shared, Actor::User, recovery::Kind::Manual).unwrap();
    let recent = RecentMapsWindow::new().unwrap();
    recent_maps_ui::bind_with_access(&recent, &shared, access.clone());
    recent_maps_ui::refresh(&recent, &lock(&shared));
    recent.invoke_select_map(source.to_string_lossy().to_string().into());
    recent.set_discard_changes(true);
    let restore = RecoveryWindow::new().unwrap();
    recovery_ui::bind_with_access(&restore, &shared, access);
    recovery_ui::refresh(&restore, &lock(&shared));
    restore.invoke_select_snapshot(entry.id.into());
    restore.set_discard_changes(true);
    let old_recent_review = recent.get_map_guard();
    let old_restore_review = restore.get_map_review();
    let bytes = std::fs::read(&source).unwrap();
    document.shutdown().unwrap();
    let before = lock(&shared).history_snapshot();
    assert_eq!(recent.get_map_guard(), old_recent_review);
    assert_eq!(restore.get_map_review(), old_restore_review);
    recent.invoke_open_selected();
    restore.invoke_restore_snapshot();
    assert_eq!(recent.get_message(), "the document is closed");
    assert_eq!(restore.get_message(), "the document is closed");
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

#[test]
fn explicit_save_completion_repairs_stale_source_and_cancel_preserves_state() {
    software_backend();
    let (directory, document, shared, access) = fixture();
    let source = directory.path().join("source.vmf");
    access
        .with_session(|session| session.save(Actor::User, Some(&source)))
        .unwrap();
    access
        .with_session(|session| {
            session.create_block(
                Actor::User,
                DVec3::splat(32.0),
                DVec3::splat(48.0),
                "test",
                false,
            )
        })
        .unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, b"external replacement").unwrap();
    assert!(
        access
            .with_session(|session| session.save(Actor::User, None))
            .is_err()
    );
    let app = AppWindow::new().unwrap();
    app.set_message("Choose a destination".into());
    let continuation = access.downgrade();
    let generation = lock(&shared).document_generation();
    let before = lock(&shared).history_snapshot();
    complete_save_dialog(&app.as_weak(), &continuation, None, generation);
    assert_eq!(app.get_message(), "Choose a destination");
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(lock(&shared).path(), Some(source.as_path()));
    assert!(lock(&shared).document().is_dirty());
    let destination = directory.path().join("repaired.vmf");
    complete_save_dialog(
        &app.as_weak(),
        &continuation,
        Some(&destination),
        generation,
    );
    assert_eq!(lock(&shared).path(), Some(destination.as_path()));
    assert!(!lock(&shared).document().is_dirty());
    assert!(destination.is_file());
    assert_eq!(std::fs::read(&source).unwrap(), b"external replacement");
    access
        .with_session(|session| session.save(Actor::User, None))
        .unwrap();

    access
        .with_session(|session| session.new_map(Actor::User, true))
        .unwrap();
    let replacement = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let stale = directory.path().join("stale-picker.vmf");
    complete_save_dialog(&app.as_weak(), &continuation, Some(&stale), generation);
    assert!(!stale.exists());
    assert!(app.get_message().contains("map was replaced"));
    assert_eq!(lock(&shared).document().vmf(), &replacement);
    assert_eq!(lock(&shared).history_snapshot(), history);
    document.shutdown().unwrap();
}

#[test]
fn two_toolbar_rows_keep_controls_reachable_and_invoke_save_as() {
    use i_slint_backend_testing::ElementHandle;
    use std::cell::Cell;

    software_backend();
    let app = AppWindow::new().unwrap();
    app.set_can_undo(true);
    app.set_can_redo(true);
    app.set_has_selection(true);
    app.set_game_running(true);
    app.set_projection_index(0);
    app.set_mcp_status("MCP · http://127.0.0.1:29082/mcp".into());
    let saves = Rc::new(Cell::new(0));
    app.on_save_map_as({
        let saves = saves.clone();
        move || saves.set(saves.get() + 1)
    });
    app.show().unwrap();
    let rows: [&[&str]; 2] = [
        &[
            "New",
            "Open…",
            "Recent…",
            "Save",
            "Save as…",
            "Build settings…",
            "Compile region…",
            "Compile",
            "Run Map",
            "Game Log",
            "Stop Map",
        ],
        &[
            "Undo",
            "Redo",
            "History…",
            "Add brush",
            "Delete",
            "Working region…",
            "Tool textures",
            "Leak path",
        ],
    ];
    for width in [1280, 1600] {
        app.window().set_size(slint::PhysicalSize::new(width, 960));
        app.window().take_snapshot().unwrap();
        assert_eq!(app.window().size().width, width);
        let mut second_row_end = 0.0;
        for (row, labels) in rows.iter().enumerate() {
            let top = row as f32 * 44.0;
            let mut previous_right = 0.0;
            for label in *labels {
                let control = ElementHandle::find_by_accessible_label(&app, label)
                    .next()
                    .unwrap_or_else(|| panic!("Missing toolbar control {label}"));
                let position = control.absolute_position();
                let size = control.size();
                assert!(size.width > 0.0 && size.height > 0.0, "{label}");
                assert!(control.computed_opacity() > 0.0, "{label}");
                assert!(
                    position.x >= previous_right - 0.5,
                    "{label} overlaps at {width}"
                );
                assert!(
                    position.x + size.width <= width as f32 + 0.5,
                    "{label} clips at {width}"
                );
                assert!(position.y >= top - 0.5, "{label} is in the wrong row");
                assert!(
                    position.y + size.height <= top + 44.5,
                    "{label} clips vertically"
                );
                previous_right = position.x + size.width;
            }
            second_row_end = previous_right;
        }
        let status = ElementHandle::find_by_element_id(&app, "AppWindow::toolbar-mcp-status")
            .next()
            .unwrap();
        let position = status.absolute_position();
        let size = status.size();
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(position.x >= second_row_end);
        assert!(position.x + size.width <= width as f32 + 0.5);
        assert!(position.y >= 44.0 && position.y + size.height <= 88.5);
        ElementHandle::find_by_accessible_label(&app, "Save as…")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
    }
    assert_eq!(saves.get(), 2);
    crate::ui::tests::save_ui_snapshot(app.window(), "save-as-controls");
    app.hide().unwrap();
}
