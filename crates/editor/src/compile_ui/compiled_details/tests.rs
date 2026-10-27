use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::*;
use crate::compile_ui::Controller;

#[test]
fn compiled_detail_controls_share_background_prepare_show_clear_and_output_invalidation() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (_directory, session, source) =
        editor_core::session::compiled_world_lights::test_support::compiled(false);
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.history_snapshot(),
        std::fs::read(&source).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    controller.window.invoke_inspect_compiled_details();
    let form = &controller.compiled_details;
    assert!(form.window.window().is_visible());
    assert!(!form.window.get_has_token());
    form.window.invoke_prepare_details();
    let started = Instant::now();
    loop {
        let report = lock(&shared).inspect_compiled_details(None, 0, 1).unwrap();
        if report["worker_active"] == false {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Detail worker did not drain"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    controller.refresh_visible(&lock(&shared));
    assert!(form.window.get_has_token());
    form.window.invoke_refresh_inspection();
    assert!(form.window.get_state().contains("prepared"));
    assert_eq!(form.window.get_lighting_index(), 0);
    form.window.invoke_change_lighting(1);
    assert_eq!(form.window.get_lighting_index(), 1);
    assert_eq!(
        lock(&shared).inspect_compiled_details(None, 0, 1).unwrap()["lighting"]["mode"],
        "baked_ldr"
    );
    form.window.invoke_change_lighting(2);
    assert_eq!(form.window.get_lighting_index(), 1);
    assert!(
        form.window
            .get_message()
            .contains("Unknown compiled detail lighting mode")
    );
    form.window.invoke_show_details();
    assert!(form.window.get_state().contains("displayed true"));
    assert!(form.window.get_state().contains("complete geometry true"));
    form.window.invoke_hide_details();
    assert!(form.window.get_state().contains("displayed false"));
    assert_eq!(form.window.get_lighting_index(), 1);
    form.window.invoke_show_details();
    std::fs::write(source.with_extension("bsp"), b"replaced output").unwrap();
    form.window.invoke_refresh_inspection();
    assert!(form.window.get_state().contains("unavailable"));
    assert!(form.window.get_rows().is_empty());
    form.window.invoke_clear_details();
    assert!(!form.window.get_has_token());
    assert!(form.window.get_state().contains("not_loaded"));
    assert_eq!(form.window.get_lighting_index(), 0);
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.camera(),
            session.history_snapshot(),
            std::fs::read(source).unwrap()
        ),
        before
    );
    form.window.invoke_close_dialog();
    assert!(!form.is_visible());
}
