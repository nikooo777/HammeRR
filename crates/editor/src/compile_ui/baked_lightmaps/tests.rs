use std::sync::{Arc, Mutex};

use super::*;
use crate::compile_ui::Controller;

#[test]
fn baked_lightmap_controls_inspect_successful_build_and_clear_changed_output() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let mut vmf = crate::test_assets::room();
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            side.material = "test/checker".into();
        }
    }
    let mut light = source_formats::vmf::Entity::new(9000, "light");
    light.properties.set("origin", "-160 -100 96");
    light.properties.set("_light", "255 160 60 600");
    light.properties.set("_lightHDR", "80 160 255 1200");
    vmf.entities.push(light);
    let source = directory.path().join("lightmaps.vmf");
    let output = directory.path().join("lightmaps.bsp");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session
        .open(super::super::Actor::User, &source, false)
        .unwrap();
    session
        .update_compile_settings(
            super::super::Actor::User,
            Some("direct_ldr_hdr"),
            Some("none"),
        )
        .unwrap();
    let request = session
        .begin_compile(super::super::Actor::User, Some(&output))
        .unwrap();
    let result = request.run();
    session.finish_compile(super::super::Actor::User, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.window.invoke_inspect_baked_lightmaps();
    let window = &controller.baked_lightmaps.window;
    assert!(window.window().is_visible());
    assert!(window.get_has_preview());
    assert!(window.get_build_state().contains("current"));
    assert!(window.get_selected().contains("Linear RGB intensity"));
    window.set_channel("HDR".into());
    window.invoke_inspect_face();
    assert!(window.get_has_preview());
    assert!(window.get_selected().contains("HDR"));
    crate::ui::tests::save_ui_snapshot(window.window(), "baked-lightmaps");
    window.set_style_index(3);
    window.invoke_query_changed();
    assert!(!window.get_has_preview());
    window.invoke_inspect_face();
    assert!(!window.get_has_preview());
    assert!(window.get_diagnostics().contains("absent"));
    window.set_style_index(0);
    window.invoke_inspect_face();
    assert!(window.get_has_preview());
    std::fs::write(&output, b"external replacement").unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_has_preview());
    assert!(window.get_build_state().contains("unavailable"));
    window.invoke_inspect_face();
    assert!(!window.get_has_preview());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}
