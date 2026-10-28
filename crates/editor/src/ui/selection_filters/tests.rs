use super::*;
use crate::runtime::{DocumentAccess, lock};
use glam::DVec3;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

#[test]
fn bound_selection_filter_controls_share_the_session_without_pruning_and_reject_closed_calls() {
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
    assert!(
        app.get_pick_world_brushes()
            && app.get_pick_brush_entities()
            && app.get_pick_point_entities()
    );
    app.window().set_size(slint::PhysicalSize::new(1100, 1000));
    app.show().unwrap();
    crate::ui::tests::scroll_inspector_to_control(&app, "World brushes")
        .invoke_accessible_default_action();
    assert!(!lock(&shared).selection_filters().world_brushes);
    assert_eq!(lock(&shared).selected_ids(), [selected]);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    lock(&shared)
        .set_selection_filters(
            Actor::Agent,
            SelectionFilters {
                world_brushes: true,
                brush_entities: false,
                point_entities: false,
            },
        )
        .unwrap();
    refresh(&app, &lock(&shared));
    assert!(app.get_pick_world_brushes());
    assert!(!app.get_pick_brush_entities() && !app.get_pick_point_entities());
    crate::ui::tests::scroll_inspector_to_control(&app, "Point entities & models");
    for label in ["World brushes", "Brush entities", "Point entities & models"] {
        crate::ui::tests::assert_dialog_button_visible(&app, label);
    }
    crate::ui::tests::assert_ui_renders(app.window(), "selection-filters");
    document.shutdown().unwrap();
    let before = lock(&shared).selection_filter_state();
    app.set_pick_world_brushes(false);
    app.invoke_selection_filters_changed();
    assert_eq!(app.get_message(), "the document is closed");
    assert_eq!(lock(&shared).selection_filter_state(), before);
    app.hide().unwrap();
}
