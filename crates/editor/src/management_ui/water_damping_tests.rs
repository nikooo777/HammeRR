use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

fn snapshot(shared: &SharedSession) -> (source_formats::vmf::Vmf, String) {
    let session = lock(shared);
    (session.document().vmf().clone(), session.history_snapshot())
}

#[test]
fn water_damping_number_callbacks_preserve_raw_text_and_stage_cancel_apply_remove() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let key = compiler::WATER_DAMPING_KEY;
    let raw = " 5e-3 ";
    let mut session = Session::new(Err("property controls need no assets".into()));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let access = DocumentAccess::unmanaged(shared.clone());
    let window = PropertiesWindow::new().unwrap();
    bind_properties_with_access(&window, &access).unwrap();
    window.set_filter("Liquid pressure damping".into());
    refresh_properties(&window, &lock(&shared));
    assert_eq!(window.get_fields().row_count(), 1);
    let field = window.get_fields().row_data(0).unwrap();
    assert_eq!(field.field.label, "Liquid pressure damping");
    assert_eq!(field.field.control, "number");
    assert_eq!(field.field.type_name, "float");
    assert_eq!(field.field.value, "0.01");
    assert!(field.field.numeric);
    assert!(field.status.contains("No stored override"));
    assert!(!field.can_reset);
    let before = snapshot(&shared);
    window.invoke_edit(key.into(), raw.into());
    let staged = serde_json::to_value(lock(&shared).property_draft()).unwrap();
    let field = window.get_fields().row_data(0).unwrap();
    assert_eq!(field.field.control, "number");
    assert_eq!(field.field.value, raw);
    for invalid in ["0.010000000000000002", "1e-9999", "1e-46"] {
        window.invoke_edit(key.into(), invalid.into());
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
    window.invoke_edit(key.into(), "0.005".into());
    window.set_filter("Water".into());
    refresh_properties(&window, &lock(&shared));
    window.show().unwrap();
    crate::ui::tests::assert_ui_renders(window.window(), "water-pressure-world-settings");
    window.set_filter("Liquid pressure damping".into());
    window.invoke_edit(key.into(), raw.into());
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some(raw)
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    window.invoke_remove(key.into());
    assert_eq!(window.get_fields().row_data(0).unwrap().field.value, "0.01");
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        None
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some(raw)
    );
    lock(&shared).redo(Actor::User).unwrap();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        None
    );
}
