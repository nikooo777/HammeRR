use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use serde_json::{Value, json};
use slint::Model;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    shared: SharedSession,
    access: DocumentAccess,
    parent: PropertiesWindow,
    color: ColorWindow,
    material: MaterialWindow,
    sky: Rc<RefCell<Option<crate::sky_assets_ui::Controller>>>,
    _directory: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"
@PointClass = fixture [
    tint(color1) : "Tint" : "0.5 0.25 0.75"
    material(material) : "Material" : "test/checker"
    vector(vector) : "Vector" : "0 0 0"
    count(integer) : "Count" : 1
    spawnflags(flags) = [ 1 : "Enabled" : 0 ]
]
"#,
        )
        .unwrap(),
    );
    let map_core::ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "fixture",
            Some(glam::DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    session
        .begin_property_edit(PropertyTarget::Entities(vec![id]))
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let parent = PropertiesWindow::new().unwrap();
    bind_properties_with_access(&parent, &access).unwrap();
    let color = bind_color(&parent, &access).unwrap();
    let material = bind_material(&parent, &access).unwrap();
    let sky = crate::sky_assets_ui::bind_with_access(&parent, &access);
    refresh_properties(&parent, &lock(&shared));
    Fixture {
        document,
        shared,
        access,
        parent,
        color,
        material,
        sky,
        _directory: directory,
    }
}

#[test]
fn hsv_hsl_property_picker_keeps_local_edits_until_apply_and_commits_one_undo() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let f = fixture();
    f.parent.invoke_pick_color("tint".into());
    let before = state(&f.shared);
    for model in [1, 2, 0, 1] {
        f.color.invoke_select_model(model);
        assert_eq!(
            crate::color_ui::read(&f.color).unwrap().rgb1,
            [0.5, 0.25, 0.75]
        );
        assert_eq!(state(&f.shared), before);
    }
    f.color.set_hue("240".into());
    f.color.set_saturation("100".into());
    f.color.set_level("100".into());
    f.color.invoke_components_changed();
    assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb255, [0, 0, 255]);
    f.color.invoke_cancel();
    assert_eq!(state(&f.shared), before);
    f.parent.invoke_pick_color("tint".into());
    f.color.invoke_select_model(2);
    f.color.set_hue("NaN".into());
    f.color.invoke_apply();
    assert!(f.color.get_message().contains("finite"));
    assert_eq!(state(&f.shared), before);
    f.color.set_hue("120".into());
    f.color.set_saturation("100".into());
    f.color.set_level("50".into());
    f.color.invoke_components_changed();
    assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb255, [0, 255, 0]);
    crate::ui::tests::assert_dialog_button_visible(&f.color, "Apply color");
    crate::ui::tests::assert_dialog_button_visible(&f.color, "Hue in degrees");
    crate::ui::tests::assert_dialog_button_visible(&f.color, "Saturation in percent");
    crate::ui::tests::assert_dialog_button_visible(&f.color, "Lightness in percent");
    crate::ui::tests::save_ui_snapshot(f.color.window(), "property-color-hsl");
    let source = lock(&f.shared).document().vmf().clone();
    let history = lock(&f.shared).document().undo_labels().count();
    f.color.invoke_apply();
    assert_eq!(lock(&f.shared).document().vmf(), &source);
    let preview = lock(&f.shared).preview_property_edit().unwrap();
    assert_eq!(
        preview
            .properties
            .iter()
            .find(|property| property.field.key == "tint")
            .unwrap()
            .field
            .value,
        "0 1 0"
    );
    f.parent.invoke_apply();
    assert_eq!(
        lock(&f.shared).document().undo_labels().count(),
        history + 1
    );
    lock(&f.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&f.shared).document().vmf(), &source);
}

fn state(shared: &SharedSession) -> Value {
    let session = lock(shared);
    let favorites = session.material_favorites_snapshot();
    json!({"map":session.document().vmf().write(&Default::default()),"draft":session.property_draft(),"history":session.history_snapshot(),"camera":format!("{:?}",session.camera()),"view":session.view_serial(),"management":session.management_serial(),"material_generation":session.materials().generation(),"favorites":favorites.materials,"favorite_status":favorites.status,"favorite_serial":favorites.serial,"selection":session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>()})
}

fn fields(f: &Fixture) -> Value {
    let sky = f.sky.borrow();
    let sky=sky.as_ref().map(|sky|json!({"family":sky.window.get_family().to_string(),"summary":sky.window.get_summary().to_string(),"scope":sky.window.get_scope().to_string(),"read":sky.window.get_read_summary().to_string(),"face":sky.window.get_selected_face(),"rows":sky.window.get_faces().iter().map(|row|json!({"label":row.label.to_string(),"detail":row.detail.to_string(),"ldr":row.ldr_status.to_string(),"hdr":row.hdr_status.to_string()})).collect::<Vec<_>>()}));
    json!({"parent":{"token":f.parent.get_draft_token().to_string(),"heading":f.parent.get_heading().to_string(),"summary":f.parent.get_summary().to_string(),"apply":f.parent.get_can_apply(),"world":f.parent.get_world_settings(),"rows":f.parent.get_fields().iter().map(|row|json!({"key":row.field.key.to_string(),"value":row.field.value.to_string(),"status":row.status.to_string(),"reset":row.can_reset})).collect::<Vec<_>>()},"color":{"r":f.color.get_red(),"g":f.color.get_green(),"b":f.color.get_blue(),"label":f.color.get_field_label().to_string()},"material":{"name":f.material.get_material().to_string(),"filter":f.material.get_filter().to_string(),"group":f.material.get_group().to_string(),"offset":f.material.get_offset(),"favorite":f.material.get_favorite(),"generation":f.material.get_asset_generation().to_string()},"sky":sky})
}

fn reject_actions(f: &Fixture, expected: &Value, error: &str) {
    let expected_state = state(&f.shared);
    macro_rules! rejected {
        ($action:expr, $message:expr) => {{
            $action;
            assert_eq!($message, error);
            assert_eq!(fields(f), *expected);
            assert_eq!(state(&f.shared), expected_state);
        }};
    }
    rejected!(f.parent.invoke_search(), f.parent.get_message());
    rejected!(
        f.parent.invoke_edit("skyname".into(), "changed".into()),
        f.parent.get_message()
    );
    rejected!(
        f.parent.invoke_reset("skyname".into()),
        f.parent.get_message()
    );
    rejected!(
        f.parent.invoke_remove("skyname".into()),
        f.parent.get_message()
    );
    rejected!(
        f.parent
            .invoke_flag_edit("spawnflags".into(), "bad".into(), true),
        f.parent.get_message()
    );
    rejected!(
        f.parent
            .invoke_component_edit("tint".into(), 0, "bad".into()),
        f.parent.get_message()
    );
    rejected!(
        f.parent.invoke_pick_color("tint".into()),
        f.parent.get_message()
    );
    rejected!(
        f.parent.invoke_pick_material("material".into()),
        f.parent.get_message()
    );
    rejected!(f.parent.invoke_inspect_sky_assets(), f.parent.get_message());
    rejected!(f.parent.invoke_apply(), f.parent.get_message());
    rejected!(f.color.invoke_apply(), f.color.get_message());
    rejected!(f.material.invoke_search(), f.material.get_detail());
    rejected!(
        f.material.invoke_preview_material(),
        f.material.get_detail()
    );
    rejected!(f.material.invoke_toggle_favorite(), f.material.get_detail());
    rejected!(
        f.material.invoke_reload_material_assets(),
        f.material.get_detail()
    );
    rejected!(f.material.invoke_use_material(), f.material.get_detail());
    if let Some(sky) = f.sky.borrow().as_ref() {
        rejected!(sky.window.invoke_refresh_assets(), sky.window.get_issues());
    }
}

#[test]
fn properties_and_owned_pickers_act_live_then_reject_drain_and_preserve_peer() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let original = lock(&f.shared).document().vmf().clone();
    f.parent.invoke_edit("count".into(), "3".into());
    f.parent.invoke_reset("count".into());
    f.parent.invoke_remove("count".into());
    f.parent
        .invoke_flag_edit("spawnflags".into(), "1".into(), true);
    f.parent
        .invoke_component_edit("vector".into(), 1, "32".into());
    f.parent.invoke_pick_color("tint".into());
    assert!(f.color.window().is_visible());
    f.color.set_red(255);
    f.color.set_green(0);
    f.color.set_blue(0);
    f.color.invoke_apply();
    assert!(!f.color.window().is_visible());
    assert!(lock(&f.shared).property_draft().unwrap().changes.iter().any(|change|matches!(change,PropertyChange::Component{key,index:0,value} if key=="tint" && *value==1.0)));
    f.parent.invoke_pick_material("material".into());
    assert!(f.material.window().is_visible());
    f.material.invoke_use_material();
    assert!(!f.material.window().is_visible());
    assert_eq!(lock(&f.shared).document().vmf(), &original);
    f.parent.invoke_apply();
    assert_ne!(lock(&f.shared).document().vmf(), &original);
    lock(&f.shared).undo(Actor::User).unwrap();
    lock(&f.shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&f.parent, &lock(&f.shared));
    f.parent.invoke_edit("skyname".into(), "pending".into());
    f.parent.invoke_inspect_sky_assets();
    let sky = f.sky.borrow().as_ref().unwrap().window.clone_strong();
    assert!(sky.window().is_visible());
    sky.invoke_refresh_assets();
    let peer = fixture();
    peer.parent.invoke_edit("count".into(), "7".into());
    let peer_state = state(&peer.shared);
    let peer_fields = fields(&peer);
    let expected = fields(&f);
    let gate = f.document.admit().unwrap();
    let draining = f.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while f.document.lifecycle() == DocumentLifecycle::Live
        || lock(&f.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let unchanged = state(&f.shared);
    reject_actions(&f, &expected, "the editor is closing");
    assert_eq!(state(&f.shared), unchanged);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    reject_actions(&f, &expected, "the document is closed");
    f.parent.show().unwrap();
    f.parent.invoke_cancel();
    assert!(!f.parent.window().is_visible());
    f.parent.show().unwrap();
    f.parent
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!f.parent.window().is_visible());
    f.color.show().unwrap();
    f.color.invoke_cancel();
    assert!(!f.color.window().is_visible());
    f.material.show().unwrap();
    f.material.invoke_cancel();
    assert!(!f.material.window().is_visible());
    sky.invoke_close_dialog();
    assert!(!sky.window().is_visible());
    assert_eq!(fields(&f), expected);
    assert_eq!(state(&f.shared), unchanged);
    assert_eq!(state(&peer.shared), peer_state);
    assert_eq!(fields(&peer), peer_fields);
    peer.parent.invoke_apply();
    assert_ne!(state(&peer.shared), peer_state);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let store = crate::recovery::Store {
        directory: f._directory.path().join("recovery"),
    };
    assert_eq!(store.list().unwrap().entries.len(), 1);
}

#[test]
fn property_picker_old_tokens_preserve_replaced_draft_and_parent_cleanup_is_exact() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    f.parent.invoke_pick_color("tint".into());
    f.parent.invoke_pick_material("material".into());
    lock(&f.shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    let foreign = state(&f.shared);
    f.color.invoke_apply();
    assert!(f.color.get_message().contains("replaced"));
    f.material.invoke_use_material();
    assert!(f.material.get_detail().contains("replaced"));
    f.parent.invoke_cancel();
    assert_eq!(state(&f.shared), foreign);
    f.parent.show().unwrap();
    f.parent
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!f.parent.window().is_visible());
    assert_eq!(state(&f.shared), foreign);
}

#[test]
fn property_children_keep_no_idle_document_owner_and_missing_parent_prevents_forwarding() {
    i_slint_backend_testing::init_no_event_loop();
    let Fixture {
        document,
        shared,
        access,
        parent,
        color,
        material,
        sky,
        _directory,
    } = fixture();
    parent.invoke_pick_color("tint".into());
    parent.invoke_pick_material("material".into());
    let target = lock(&shared).property_draft().unwrap().target.clone();
    lock(&shared)
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    refresh_properties(&parent, &lock(&shared));
    parent.invoke_inspect_sky_assets();
    let sky_window = sky.borrow().as_ref().unwrap().window.clone_strong();
    let sky_family = sky_window.get_family();
    let sky_read = sky_window.get_read_summary();
    lock(&shared).begin_property_edit(target).unwrap();
    refresh_properties(&parent, &lock(&shared));
    parent.invoke_pick_color("tint".into());
    parent.invoke_pick_material("material".into());
    let expected = state(&shared);
    parent.hide().unwrap();
    let weak = parent.as_weak();
    drop(parent);
    assert!(
        with_properties(&weak, &access.downgrade(), |_, _| panic!(
            "vanished parent accessed Session"
        ))
        .is_none()
    );
    color.invoke_apply();
    material.invoke_use_material();
    assert_eq!(state(&shared), expected);
    let weak_session = Arc::downgrade(&shared);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak_session.upgrade().is_none());
    color.invoke_apply();
    assert_eq!(color.get_message(), "the document is closed");
    material.invoke_search();
    assert_eq!(material.get_detail(), "the document is closed");
    sky_window.invoke_refresh_assets();
    assert_eq!(sky_window.get_issues(), "the document is closed");
    assert_eq!(sky_window.get_family(), sky_family);
    assert_eq!(sky_window.get_read_summary(), sky_read);
    sky_window.invoke_close_dialog();
    assert!(!sky_window.window().is_visible());
    color.invoke_cancel();
    material.invoke_cancel();
    assert!(!color.window().is_visible());
    assert!(!material.window().is_visible());
    assert!(sky.borrow().is_some());
    drop(_directory);
}

#[test]
fn property_stage_keeps_its_admission_through_real_shutdown_and_borrowed_refresh() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = with_properties(
        &f.parent.as_weak(),
        &f.access.downgrade(),
        |window, session| {
            let document = f.document.clone();
            let drain = std::thread::spawn(move || done.send(document.shutdown()).unwrap());
            let deadline = Instant::now() + TIMEOUT;
            while f.document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            stage(
                window,
                session,
                PropertyChange::Set {
                    key: "count".into(),
                    value: "9".into(),
                },
            );
            assert!(window.get_can_apply());
            assert!(completed.try_recv().is_err());
            drain
        },
    )
    .unwrap();
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(lock(&f.shared).property_draft().unwrap().changes.len(), 1);
}

#[test]
fn failed_property_drain_keeps_exact_error_and_local_close_without_retry() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    f.parent.invoke_edit("count".into(), "4".into());
    let expected = fields(&f);
    let recovery = f._directory.path().join("recovery");
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = f.document.shutdown().unwrap_err();
    let expected_state = state(&f.shared);
    reject_actions(&f, &expected, &format!("document shutdown failed: {error}"));
    f.parent.show().unwrap();
    f.parent.invoke_cancel();
    assert!(!f.parent.window().is_visible());
    assert_eq!(state(&f.shared), expected_state);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(f.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn saved_palette_picker_buttons_preserve_precision_brightness_and_draft_cancel() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let f = fixture();
    f.parent.invoke_pick_color("tint".into());
    let before = state(&f.shared);
    let precise: [f64; 3] = [0.12345678901234566, 0.25000000000000006, 0.9876543210987654];
    crate::color_ui::load(
        &f.color,
        "color1",
        &precise.map(|value| value.to_string()).join(" "),
    )
    .unwrap();
    f.color.set_intensity("123.5".into());
    f.color.set_palette_name("Choose a saved color".into());
    fn press(window: &ColorWindow, label: &str) {
        crate::ui::tests::assert_dialog_button_visible(window, label);
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, label)
            .next()
            .unwrap()
            .invoke_accessible_default_action();
    }
    press(&f.color, "Save new color");
    assert_eq!(f.color.get_palette_index(), 1);
    crate::color_ui::refresh_palette(&f.color, &lock(&f.shared));
    assert_eq!(f.color.get_palette_index(), 1);
    assert_eq!(
        f.color.get_palette_names().row_data(1).unwrap(),
        "Choose a saved color"
    );
    assert_eq!(
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &f.color,
            "Use saved color"
        )
        .next()
        .unwrap()
        .accessible_enabled(),
        Some(true)
    );
    assert_eq!(
        lock(&f.shared).color_palette_state()["entries"][0]["rgb1"],
        json!(precise)
    );
    assert_eq!(state(&f.shared), before);
    let palette_path = std::path::PathBuf::from(
        lock(&f.shared).color_palette_state()["path"]
            .as_str()
            .unwrap(),
    );
    let bytes = std::fs::read(&palette_path).unwrap();
    f.color.invoke_select_model(1);
    f.color.set_hue("incomplete".into());
    press(&f.color, "Replace saved color");
    assert!(f.color.get_message().contains("valid color numbers"));
    assert_eq!(f.color.get_hue(), "incomplete");
    assert_eq!(std::fs::read(&palette_path).unwrap(), bytes);
    f.color.set_palette_name("Rejected invalid input".into());
    press(&f.color, "Save new color");
    assert_eq!(std::fs::read(&palette_path).unwrap(), bytes);
    press(&f.color, "Use saved color");
    assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb1, precise);
    assert_eq!(f.color.get_intensity(), "123.5");
    f.color.invoke_preset(0, 255, 0);
    press(&f.color, "Replace saved color");
    assert_eq!(
        lock(&f.shared).color_palette_state()["entries"][0]["rgb1"],
        json!([0.0, 1.0, 0.0])
    );
    crate::color_ui::load(
        &f.color,
        "color1",
        &precise.map(|value| value.to_string()).join(" "),
    )
    .unwrap();
    press(&f.color, "Replace saved color");
    for index in 1..16 {
        let mut session = lock(&f.shared);
        let snapshot = session.color_palette_state()["snapshot"]
            .as_str()
            .unwrap()
            .to_owned();
        session
            .change_color_palette(
                "save",
                &snapshot,
                Some(&format!("Color {index:02}")),
                Some(editor_core::color::Input {
                    space: editor_core::color::Space::Rgb1,
                    channels: precise,
                }),
            )
            .unwrap();
    }
    crate::color_ui::refresh_palette(&f.color, &lock(&f.shared));
    assert_eq!(f.color.get_palette_names().row_count(), 17);
    for index in 1..=16 {
        f.color.set_palette_index(index);
        f.color.invoke_palette_choice();
        press(&f.color, "Use saved color");
        assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb1, precise);
    }
    assert_eq!(f.color.get_intensity(), "123.5");
    f.color.set_palette_name("Choose a saved color".into());
    f.color.invoke_select_model(2);
    crate::ui::tests::save_ui_snapshot(f.color.window(), "entity-color-palette");
    press(&f.color, "Reload palette");
    press(&f.color, "Delete saved color");
    assert_eq!(
        lock(&f.shared).color_palette_state()["entries"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
    assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb1, precise);
    let repaired = std::fs::read(&palette_path).unwrap();
    std::fs::write(&palette_path, b"invalid").unwrap();
    press(&f.color, "Reload palette");
    assert!(!f.color.get_palette_ready());
    assert!(f.color.get_palette_status().contains("Palette unavailable"));
    assert!(f.color.get_palette_status().contains("could not load"));
    assert_eq!(crate::color_ui::read(&f.color).unwrap().rgb1, precise);
    assert_eq!(
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &f.color,
            "Save new color"
        )
        .next()
        .unwrap()
        .accessible_enabled(),
        Some(false)
    );
    std::fs::write(&palette_path, repaired).unwrap();
    press(&f.color, "Reload palette");
    assert!(f.color.get_palette_ready());
    f.color.invoke_cancel();
    assert_eq!(state(&f.shared), before);
    let mut restarted = Session::new(Err("no assets needed".into()));
    restarted.configure_recovery(Ok(palette_path.parent().unwrap().into()));
    assert_eq!(
        restarted.color_palette_state()["entries"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
    assert_eq!(restarted.color_palette_state()["ready"], true);
}

#[test]
fn rename_saved_color_button_preserves_invalid_local_draft_and_selects_sorted_name() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let f = fixture();
    f.parent.invoke_pick_color("tint".into());
    let before = state(&f.shared);
    let precise: [f64; 3] = [0.12345678901234566, 0.25000000000000006, 0.9876543210987654];
    {
        let mut session = lock(&f.shared);
        for name in ["Middle", "Last"] {
            let snapshot = session.color_palette_state()["snapshot"]
                .as_str()
                .unwrap()
                .to_owned();
            session
                .change_color_palette(
                    "save",
                    &snapshot,
                    Some(name),
                    Some(editor_core::color::Input {
                        space: editor_core::color::Space::Rgb1,
                        channels: precise,
                    }),
                )
                .unwrap();
        }
    }
    crate::color_ui::refresh_palette(&f.color, &lock(&f.shared));
    f.color.set_palette_index(2);
    f.color.invoke_palette_choice();
    f.color.invoke_select_model(1);
    f.color.set_hue("unfinished".into());
    f.color.set_intensity("450.5".into());
    f.color.set_palette_name("Choose a saved color".into());
    fn rename(window: &ColorWindow) {
        crate::ui::tests::assert_dialog_button_visible(window, "Rename saved color");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            window,
            "Rename saved color",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    }
    rename(&f.color);
    assert_eq!(f.color.get_palette_index(), 1);
    assert_eq!(
        f.color.get_palette_names().row_data(1).unwrap(),
        "Choose a saved color"
    );
    assert_eq!(f.color.get_hue(), "unfinished");
    assert!(crate::color_ui::read(&f.color).is_err());
    assert_eq!(f.color.get_intensity(), "450.5");
    let renamed = lock(&f.shared).color_palette_state();
    assert_eq!(renamed["entries"][0]["rgb1"], json!(precise));
    assert_eq!(state(&f.shared), before);
    let path = std::path::PathBuf::from(renamed["path"].as_str().unwrap());
    let bytes = std::fs::read(&path).unwrap();
    for invalid in ["Last", " invalid "] {
        f.color.set_palette_name(invalid.into());
        rename(&f.color);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(f.color.get_palette_index(), 1);
        assert_eq!(f.color.get_hue(), "unfinished");
        assert_eq!(f.color.get_intensity(), "450.5");
    }
    f.color.set_palette_name("Zulu".into());
    rename(&f.color);
    assert_eq!(f.color.get_palette_index(), 2);
    crate::color_ui::refresh_palette(&f.color, &lock(&f.shared));
    assert_eq!(f.color.get_palette_index(), 2);
    assert_eq!(f.color.get_palette_names().row_data(2).unwrap(), "Zulu");
    assert_eq!(f.color.get_hue(), "unfinished");
    assert_eq!(state(&f.shared), before);
    f.color.invoke_cancel();
    assert_eq!(state(&f.shared), before);
    let mut restarted = Session::new(Err("no assets needed".into()));
    restarted.configure_recovery(Ok(path.parent().unwrap().into()));
    assert_eq!(
        restarted.color_palette_state()["entries"][1]["name"],
        "Zulu"
    );
    assert_eq!(
        restarted.color_palette_state()["entries"][1]["rgb1"],
        json!(precise)
    );
}
