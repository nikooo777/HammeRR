use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn recipe_callbacks_preserve_typed_names_and_show_current_values_before_retry() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let authority = editor_core::build_recipes::Authority::default();
    let mut peer = Session::with_build_recipes(Err("no assets".into()), authority.clone());
    let shared = Arc::new(Mutex::new(Session::with_build_recipes(
        Err("no assets".into()),
        authority,
    )));
    let controller = Controller::new(&shared).unwrap();
    controller.refresh(&lock(&shared));
    controller
        .window
        .set_recipe_name("Typed before settings".into());
    controller.window.invoke_change_physics(1);
    assert_eq!(controller.window.get_recipe_name(), "Typed before settings");
    controller.window.invoke_save_recipe();
    assert_eq!(
        lock(&shared).build_recipes()["recipes"][2]["settings"]["physics"],
        "engine"
    );
    controller
        .window
        .set_recipe_name("Typed before Agent edit".into());
    lock(&shared)
        .update_compile_settings(Actor::Agent, Some("direct_ldr"), Some("none"))
        .unwrap();
    controller.window.invoke_save_recipe();
    assert!(controller.window.get_message().contains("changed"));
    assert_eq!(
        lock(&shared).build_recipes()["recipes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        controller.window.get_selected_physics(),
        "No physics bodies"
    );
    assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
    assert_eq!(
        controller.window.get_recipe_name(),
        "Typed before Agent edit"
    );
    controller.window.invoke_save_recipe();
    assert_eq!(
        lock(&shared).build_recipes()["recipes"][3]["settings"]["lighting"],
        "direct_ldr"
    );
    let id = lock(&shared).build_recipes()["active"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    peer.rename_build_recipe(Actor::Agent, &id, "Peer renamed", None)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert_eq!(controller.window.get_selected_recipe(), "Peer renamed");
    assert!(
        controller
            .window
            .get_recipe_status()
            .contains("Current recipe: Peer renamed")
    );
    assert_eq!(peer.compile_settings()["lighting"], "fullbright");
}
