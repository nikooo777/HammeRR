use super::*;

#[test]
fn achromatic_hue_can_be_chosen_before_saturation_and_presets_sync_both_models() {
    i_slint_backend_testing::init_no_event_loop();
    let window = ColorWindow::new().unwrap();
    bind(&window);
    window.set_intensity("123.5".into());
    window.invoke_select_model(1);
    window.set_hue("240".into());
    window.set_saturation("0".into());
    window.set_level("100".into());
    window.invoke_components_changed();
    assert_eq!(read(&window).unwrap().rgb255, [255; 3]);
    assert_eq!(window.get_hue(), "240");
    window.set_saturation("100".into());
    window.invoke_components_changed();
    assert_eq!(read(&window).unwrap().rgb255, [0, 0, 255]);
    assert_eq!(
        [window.get_red(), window.get_green(), window.get_blue()],
        [0, 0, 255]
    );
    window.set_red(0);
    window.set_green(255);
    window.set_blue(0);
    window.invoke_rgb_changed();
    assert_eq!(window.get_hue(), "120");
    window.invoke_select_model(2);
    assert_eq!(window.get_level(), "50");
    assert_eq!(read(&window).unwrap().rgb255, [0, 255, 0]);
    assert_eq!(window.get_intensity(), "123.5");
    window.set_hue("-1".into());
    window.invoke_select_model(0);
    assert_eq!(window.get_model_index(), 2);
    assert!(read(&window).is_err());
    window.set_hue("120".into());
    window.invoke_select_model(0);
    window.set_red(256);
    assert!(read(&window).is_err());
    load(&window, "color1", "0.123456789 0.25 0.75").unwrap();
    window.set_red(255);
    window.invoke_rgb_changed();
    assert_eq!(read(&window).unwrap().rgb1, [1.0, 0.25, 0.75]);
    window.invoke_preset(255, 255, 255);
    assert_eq!(read(&window).unwrap().rgb1, [1.0; 3]);
}
