use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn reload_recipe_callback_repairs_storage_and_preserves_owned_review_and_options() {
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
    session.configure_build_recipes(Ok(directory.path().to_owned()));
    session
        .save_build_recipe(Actor::User, "Original", None, None)
        .unwrap();
    let path = std::path::PathBuf::from(session.build_recipes()["path"].as_str().unwrap());
    session
        .preview_measurement(
            Actor::Agent,
            editor_core::session::measurements::Request {
                id: None,
                name: "Owned clearance".into(),
                start: glam::DVec3::ZERO,
                end: glam::DVec3::X,
            },
            &session.measurement_stamp(),
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let draft = session.measurement_state()["draft"].clone();
    let options = session.compile_options();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.refresh(&lock(&shared));
    controller.window.show().unwrap();
    crate::ui::tests::assert_dialog_button_visible(&controller.window, "Reload recipes");
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["recipes"][0]["name"] = "External rename".into();
    let valid = serde_json::to_vec(&file).unwrap();
    std::fs::write(&path, &valid).unwrap();
    controller.window.invoke_reload_recipes();
    assert_eq!(controller.window.get_selected_recipe(), "External rename");
    assert!(
        controller
            .window
            .get_recipe_status()
            .contains("Current recipe: External rename")
    );
    let id = lock(&shared).build_recipes()["active"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    lock(&shared)
        .rename_build_recipe(Actor::Agent, &id, "Newer revision", None)
        .unwrap();
    controller.window.invoke_reload_recipes();
    assert!(controller.window.get_message().contains("changed"));
    std::fs::write(&path, b"broken").unwrap();
    controller.window.invoke_reload_recipes();
    assert!(!controller.window.get_custom_recipes_enabled());
    assert!(controller.window.get_recipes_reload_enabled());
    assert!(
        controller
            .window
            .get_recipe_status()
            .contains("could not reload")
    );
    assert_eq!(controller.window.get_selected_recipe(), "Newer revision");
    crate::ui::tests::save_ui_snapshot(controller.window.window(), "build-recipes-reload");
    std::fs::write(&path, valid).unwrap();
    controller.window.invoke_reload_recipes();
    assert!(controller.window.get_custom_recipes_enabled());
    assert!(controller.window.get_message().contains("reloaded"));
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(session.compile_options(), options);
    assert_eq!(session.camera(), camera);
    drop(session);
    controller.window.invoke_close_dialog();
}
