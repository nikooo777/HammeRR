use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, String) {
    let session = lock(shared);
    (session.document().vmf().clone(), session.history_snapshot())
}

#[test]
fn water_current_numeric_world_callbacks_stage_cancel_apply_and_remove_without_normalization() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let key = compiler::WATER_CURRENT_VELOCITY_KEY;
    let mut session = Session::new(Err("property controls need no assets".into()));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let access = DocumentAccess::unmanaged(shared.clone());
    let window = PropertiesWindow::new().unwrap();
    bind_properties_with_access(&window, &access).unwrap();
    window.set_filter("Liquid current".into());
    refresh_properties(&window, &lock(&shared));
    assert_eq!(window.get_fields().row_count(), 1);
    let field = window.get_fields().row_data(0).unwrap();
    assert_eq!(field.field.label, "Liquid current · world units/s");
    assert_eq!(field.field.control, "vector");
    assert_eq!(field.field.components.row_data(0).unwrap(), "0");
    assert_eq!(field.field.component_labels.row_data(2).unwrap(), "Z");
    assert!(field.status.contains("No stored override"));
    assert!(!field.can_reset);
    let before = snapshot(&shared);
    window.invoke_component_edit(key.into(), 0, "64".into());
    window.invoke_component_edit(key.into(), 1, "-16".into());
    let staged = serde_json::to_value(lock(&shared).property_draft()).unwrap();
    for (index, value) in [(1, "120"), (0, "1e-9999")] {
        window.invoke_component_edit(key.into(), index, value.into());
        assert!(window.get_message().contains(key));
        assert_eq!(
            serde_json::to_value(lock(&shared).property_draft()).unwrap(),
            staged
        );
    }
    assert_eq!(snapshot(&shared), before);
    window.invoke_cancel();
    assert_eq!(snapshot(&shared), before);
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    window.invoke_component_edit(key.into(), 0, "64".into());
    window.invoke_component_edit(key.into(), 1, "-16".into());
    window.show().unwrap();
    crate::ui::tests::assert_ui_renders(window.window(), "water-current-world-settings");
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some("64 -16 0")
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    window.invoke_remove(key.into());
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        None
    );
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    assert_eq!(
        window
            .get_fields()
            .row_data(0)
            .unwrap()
            .field
            .components
            .row_data(0)
            .unwrap(),
        "0"
    );
    window.invoke_cancel();
}
