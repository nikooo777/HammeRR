use super::*;
use crate::session::{Actor, PropertyChange, PropertyTarget};
use std::sync::{Arc, Mutex};

#[test]
fn world_sky_inspection_button_refresh_and_close_preserve_applied_map_and_pending_edits() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (root, game) = crate::test_assets::game();
    let skybox = root.path().join("materials/skybox");
    std::fs::create_dir_all(&skybox).unwrap();
    let material = "Sky { $basetexture test/checker $hdrbasetexture skybox/fixture_hdr }";
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        std::fs::write(skybox.join(format!("fixture{suffix}.vmt")), material).unwrap();
    }
    let mut hdr = std::fs::read(root.path().join("materials/test/checker.vtf")).unwrap();
    hdr.truncate(80);
    hdr[16..18].copy_from_slice(&2u16.to_le_bytes());
    hdr[18..20].copy_from_slice(&2u16.to_le_bytes());
    hdr[52..56].copy_from_slice(&24u32.to_le_bytes());
    hdr.resize(80 + 2 * 2 * 8, 0);
    std::fs::write(skybox.join("fixture_hdr.vtf"), hdr).unwrap();
    let mut session = Session::new(Ok(game));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "fixture".into(),
            },
        )
        .unwrap();
    session.commit_property_edit(Actor::User, token).unwrap();
    let brush = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[brush],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_family".into(),
            },
        )
        .unwrap();
    let document = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let draft = serde_json::to_value(session.property_draft()).unwrap();
    let management = session.management_serial();
    let shared = Arc::new(Mutex::new(session));
    let properties = PropertiesWindow::new().unwrap();
    let inspector = bind(&properties, shared.clone());
    crate::management_ui::refresh_properties(&properties, &lock(&shared));
    properties.show().unwrap();
    properties.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &properties,
        "Inspect applied sky…",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    let controller = inspector.borrow();
    let window = &controller.as_ref().unwrap().window;
    assert!(window.window().is_visible());
    assert_eq!(window.get_family(), "Applied skyname: \"fixture\"");
    assert!(window.get_summary().contains("LDR assets: ready"));
    assert!(window.get_summary().contains("HDR assets: ready"));
    assert!(window.get_report().contains("skybox/fixturert"));
    assert!(
        window
            .get_report()
            .contains("$basetexture = \"test/checker\"")
    );
    assert!(
        window
            .get_report()
            .contains(&root.path().display().to_string())
    );
    assert!(!window.get_report().contains("pending_family"));
    crate::ui::tests::save_ui_snapshot(window.window(), "sky-assets");
    for label in ["Refresh sky assets", "Close"] {
        crate::ui::tests::assert_dialog_button_visible(window, label);
    }
    std::fs::remove_file(skybox.join("fixturert.vmt")).unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, "Refresh sky assets")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(window.get_summary().contains("LDR assets: unavailable"));
    assert!(window.get_report().contains("Missing sky material"));
    std::fs::write(skybox.join("fixturert.vmt"), material).unwrap();
    window.invoke_refresh_assets();
    assert!(window.get_summary().contains("LDR assets: ready"));
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(window, "Close")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!window.window().is_visible());
    assert!(properties.window().is_visible());
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(
        serde_json::to_value(session.property_draft()).unwrap(),
        draft
    );
    assert_eq!(session.management_serial(), management);
    properties.hide().unwrap();
}
