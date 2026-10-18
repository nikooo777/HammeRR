use super::*;
use crate::runtime::{DocumentAccess, lock};
use glam::DVec3;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

#[test]
fn admitted_editor_visibility_controls_hide_and_recover_in_faces_without_source_edits() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let selected = session
        .create_block(
            Actor::User,
            DVec3::splat(-16.0),
            DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let app = AppWindow::new().unwrap();
    bind(&app, &access);
    refresh(&app, &lock(&shared));
    assert!(app.get_can_hide_editor_selection());
    app.window().set_size(slint::PhysicalSize::new(1100, 1000));
    app.show().unwrap();
    crate::ui::tests::scroll_inspector_to_control(&app, "Hide selected in editor")
        .invoke_accessible_default_action();
    assert_eq!(lock(&shared).editor_hidden_count(), 1);
    assert_eq!(lock(&shared).selected_ids(), [selected]);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(app.get_editor_hidden_count(), 1);
    lock(&shared).set_face_mode(true);
    lock(&shared)
        .select_faces(&[], crate::session::SelectMode::Replace)
        .unwrap();
    lock(&shared)
        .set_selection_filters(
            Actor::Agent,
            crate::session::selection_filters::SelectionFilters {
                world_brushes: false,
                brush_entities: false,
                point_entities: false,
            },
        )
        .unwrap();
    app.set_face_mode(true);
    crate::ui::selection_filters::refresh(&app, &lock(&shared));
    refresh(&app, &lock(&shared));
    assert!(!app.get_can_hide_editor_selection());
    crate::ui::tests::scroll_inspector_to_control(&app, "Show all editor-hidden");
    crate::ui::tests::assert_dialog_button_visible(&app, "Show all editor-hidden");
    crate::ui::tests::save_ui_snapshot(app.window(), "editor-visibility");
    crate::ui::tests::scroll_inspector_to_control(&app, "Show all editor-hidden")
        .invoke_accessible_default_action();
    assert_eq!(lock(&shared).editor_hidden_count(), 0);
    assert!(lock(&shared).face_mode());
    assert!(!lock(&shared).selection_filters().world_brushes);
    {
        let mut session = lock(&shared);
        let snapshot = session.working_region_stamp();
        session
            .set_editor_visibility(Actor::Agent, Action::Hide(vec![selected]), &snapshot)
            .unwrap();
    }
    document.shutdown().unwrap();
    let before = lock(&shared).editor_hidden_count();
    app.invoke_show_all_editor_hidden();
    assert_eq!(app.get_message(), "the document is closed");
    assert_eq!(lock(&shared).editor_hidden_count(), before);
    app.hide().unwrap();
}
