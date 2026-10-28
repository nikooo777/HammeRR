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
    form.window.invoke_change_lighting(99);
    assert_eq!(form.window.get_lighting_index(), 1);
    assert!(
        form.window
            .get_message()
            .contains("Unknown compiled detail lighting mode")
    );
    form.window.invoke_change_lighting(2);
    assert_eq!(form.window.get_lighting_index(), 2);
    form.window.set_exposure_ev(0.5);
    form.window.invoke_apply_lighting();
    assert_eq!(
        lock(&shared).inspect_compiled_details(None, 0, 1).unwrap()["lighting"]["exposure_ev"],
        0.5
    );
    form.window.set_lighting_channel_index(1);
    form.window.set_exposure_ev(2.0);
    form.window.invoke_apply_lighting();
    assert_eq!(form.window.get_lighting_index(), 2);
    assert_eq!(form.window.get_lighting_channel_index(), 0);
    assert_eq!(form.window.get_exposure_ev(), 0.5);
    assert!(
        form.window
            .get_message()
            .to_ascii_lowercase()
            .contains("hdr")
    );
    form.window.invoke_show_details();
    assert!(form.window.get_state().contains("displayed true"));
    assert!(form.window.get_state().contains("complete geometry true"));
    form.window.invoke_hide_details();
    assert!(form.window.get_state().contains("displayed false"));
    assert_eq!(form.window.get_lighting_index(), 2);
    assert_eq!(form.window.get_exposure_ev(), 0.5);
    form.window.invoke_show_details();
    std::fs::write(source.with_extension("bsp"), b"replaced output").unwrap();
    form.window.invoke_refresh_inspection();
    assert!(form.window.get_state().contains("unavailable"));
    assert!(form.window.get_rows().is_empty());
    form.window.invoke_clear_details();
    assert!(!form.window.get_has_token());
    assert!(form.window.get_state().contains("not_loaded"));
    assert_eq!(form.window.get_lighting_index(), 0);
    let mut report = lock(&shared).inspect_compiled_details(None, 0, 64).unwrap();
    report["lighting"]["mode"] = serde_json::json!("baked_radiance");
    report["lighting"]["baked_ldr_available"] = serde_json::json!(true);
    report["lighting"]["available_styles"] = serde_json::json!([1]);
    report["lighting"]["base_policy"] = serde_json::json!("per_record_static_contributions");
    report["page"]["total"] = serde_json::json!(1);
    report["page"]["returned"] = serde_json::json!(1);
    report["counts"] = serde_json::json!({"models":0,"cards":1,"cross":0,"tri":0});
    for (channel, rgb) in [("ldr", [0, 64, 0]), ("hdr", [0, 128, 0])] {
        report["lighting"]["channel"] = serde_json::json!(channel);
        let linear = rgb.map(|value| f64::from(value) / 255.0);
        report["page"]["rows"] = serde_json::json!([{
            "ordinal":0,"kind_name":"card","leaf":0,"dictionary_index":0,"origin":[0,0,0],"angles":[0,0,0],"scale":1,"orientation":0,"sway":0,"shape_angle":0,"shape_size":0,"light_styles":0,"light_style_count":2,
            "lighting":{"rgb":[0,0,0],"exponent":0,"static_contribution":{"source":"selected_channel_style_zero","selected_channel":channel,"style_zero":{"rgb":rgb,"exponent":0,"weight":1},"linear":linear},"exposed_linear":[8.0/255.0,linear[1],0.0],"style_extras":[{"style":0,"rgb":rgb,"exponent":0,"effective_weight":1},{"style":1,"rgb":[16,0,0],"exponent":0,"effective_weight":0.5}]}
        }]);
        present(
            &form.window,
            &mut form.state.borrow_mut(),
            report.clone(),
            serde_json::Value::Null,
        )
        .unwrap();
        assert_eq!(
            form.window.get_lighting_channel_index(),
            i32::from(channel == "hdr")
        );
        assert!(
            form.window
                .get_lighting_state()
                .contains(&channel.to_ascii_uppercase())
        );
        assert!(form.window.get_rows().contains(&format!(
            "stored style 0 RGBExp32 {} exponent 0 · editor weight 1",
            serde_json::json!(rgb)
        )));
        assert_eq!(form.state.borrow().styles, [1]);
        assert_eq!(form.state.borrow().selected_style, Some(1));
        form.window.invoke_choose_style(0);
        form.window.set_style_gain(0.5);
        let chosen = chosen_lighting(&form.window, &mut form.state.borrow_mut(), 2).unwrap();
        assert_eq!(
            chosen.style_gains,
            [StyleGain {
                style: 1,
                gain: 0.5
            }]
        );
    }
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
