use super::*;
use slint::Model;
use std::sync::{Arc, Mutex};

#[test]
fn model_scale_numeric_field_uses_shared_property_callback_undo_and_details() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let Ok(game) = editor_core::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::ZERO),
            BTreeMap::from([(
                "model".into(),
                "models/props_junk/wood_crate001a.mdl".into(),
            )]),
        )
        .unwrap()
    else {
        unreachable!()
    };
    let original = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let app = AppWindow::new().unwrap();
    app.on_property_edited({
        let (app, shared) = (app.as_weak(), shared.clone());
        move |key, value| set_inspected_property(&app.unwrap(), &shared, &key, &value)
    });
    refresh_panel(&app, &mut lock(&shared));
    let rows = app.get_entity_properties();
    let row = rows.iter().find(|row| row.key == "modelscale").unwrap();
    assert_eq!(row.label, "Model scale");
    assert_eq!(row.value, "1");
    assert_eq!(row.control, "number");
    assert!(!row.advanced);
    assert_eq!((row.minimum, row.maximum), (0.015625, 64.0));
    assert_eq!(lock(&shared).document().vmf(), &original);
    app.invoke_property_edited("modelscale".into(), "2".into());
    assert_eq!(
        lock(&shared).inspect_entity_model(id).unwrap()["model_scale"]["effective"],
        2.0
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    let controller = crate::model_ui::Controller::new(&shared).unwrap();
    controller.initialize(&mut lock(&shared), id);
    assert!(controller.window.get_skin_info().contains("Model scale 2"));
    assert!(!controller.window.get_pose_info().contains("scaling remain"));
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    lock(&shared)
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("modelscale".into(), Some("0.5".into()))],
        )
        .unwrap();
    refresh_panel(&app, &mut lock(&shared));
    assert_eq!(
        app.get_entity_properties()
            .iter()
            .find(|row| row.key == "modelscale")
            .unwrap()
            .value,
        "0.5"
    );
    app.invoke_property_edited("modelscale".into(), "0".into());
    refresh_panel(&app, &mut lock(&shared));
    assert_eq!(
        app.get_entity_properties()
            .iter()
            .find(|row| row.key == "modelscale")
            .unwrap()
            .control,
        "text"
    );
    assert_eq!(
        lock(&shared).inspect_entity_model(id).unwrap()["helper"],
        true
    );
    controller.window.hide().unwrap();
}
