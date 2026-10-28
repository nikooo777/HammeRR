use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn material_emission_text_callbacks_share_world_draft_cancel_apply_remove_and_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let key = compiler::MATERIAL_EMISSION_FILE_KEY;
    let raw = "  lighting/room.rad  ";
    let mut session = Session::new(Err("property controls need no assets".into()));
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(&format!(
            "@SolidClass = worldspawn [ {key}(vector) readonly : \"Locked\" : \"1 2 3\" ]"
        ))
        .unwrap(),
    );
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let access = DocumentAccess::unmanaged(shared.clone());
    let window = PropertiesWindow::new().unwrap();
    bind_properties_with_access(&window, &access).unwrap();
    window.set_filter("Emission file".into());
    refresh_properties(&window, &lock(&shared));
    assert_eq!(window.get_heading(), "World settings");
    assert_eq!(window.get_fields().row_count(), 1);
    let row = window.get_fields().row_data(0).unwrap();
    assert_eq!(row.field.label, "Emission file");
    assert_eq!(row.field.control, "text");
    assert_eq!(row.field.type_name, "string");
    assert_eq!(row.field.value, "");
    assert!(!row.field.read_only);
    assert!(!row.can_reset);
    window.invoke_edit(key.into(), raw.into());
    let staged = lock(&shared).property_edit_report().unwrap();
    assert_eq!(window.get_fields().row_data(0).unwrap().field.value, raw);
    window.invoke_edit(key.into(), "../bad.rad".into());
    assert!(window.get_message().contains("Emission file"));
    assert_eq!(lock(&shared).property_edit_report().unwrap(), staged);
    assert_eq!(lock(&shared).document().vmf(), &source);
    window.invoke_cancel();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    window.invoke_edit(key.into(), raw.into());
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        Some(raw)
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    lock(&shared).redo(Actor::User).unwrap();
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&window, &lock(&shared));
    window.invoke_remove(key.into());
    assert_eq!(window.get_fields().row_data(0).unwrap().field.value, "");
    window.invoke_apply();
    assert_eq!(
        lock(&shared).document().vmf().world.properties.get(key),
        None
    );
}
