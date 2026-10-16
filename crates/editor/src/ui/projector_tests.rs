use super::*;
use source_formats::fgd::Catalog;

#[test]
fn source_projector_panel_shares_orientation_diagnostics_and_four_component_color() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(
            r#"
@PointClass size(4 4 4) frustum(lightfov,nearz,farz,lightcolor,-1) = env_projectedtexture [
angles(angle) : "Pitch Yaw Roll" : "30 40 25"
lightfov(float) : "FOV" : 90
nearz(float) : "NearZ" : 4
farz(float) : "FarZ" : 750
lightcolor(color255) : "Light Color" : "24 210 100 200.5"
target(target_destination) : "target"
parentname(target_destination) : "Parent"
cameraspace(integer) : "Camera Space" : 0
]"#,
        )
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "env_projectedtexture",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let app = AppWindow::new().unwrap();
    app.set_advanced_properties(true);
    refresh_panel(&app, &mut session);
    assert_eq!(
        app.get_entity_helper_status().as_str(),
        session.entity_helper_report(id).unwrap().status
    );
    assert!(
        app.get_entity_helper_status()
            .contains("Source projector guide: displayed")
    );
    assert!(
        app.get_entity_helper_status()
            .contains("positive pitch down")
    );
    assert!(
        app.get_entity_helper_status()
            .contains("pitch_scale is not evaluated (declared: -1)")
    );
    let entity = session.document().entity(id).unwrap();
    let color = entity_property_rows(&session, entity)
        .into_iter()
        .find(|row| row.key == "lightcolor")
        .unwrap();
    assert!(color.has_intensity);
    assert_eq!(color.value.as_str(), "24 210 100 200.5");
    assert_eq!(color.intensity.as_str(), "200.5");
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    session
        .set_entity_properties(Actor::Agent, id, &[("lightfov".into(), None)])
        .unwrap();
    let default_source = session.document().vmf().clone();
    let default_history = session.document().undo_labels().count();
    refresh_panel(&app, &mut session);
    assert!(app.get_advanced_properties());
    assert!(
        app.get_entity_helper_status()
            .contains("FGD defaults: lightfov=90; engine defaults may differ")
    );
    assert!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("lightfov")
            .is_none()
    );
    assert_eq!(session.document().vmf(), &default_source);
    assert_eq!(session.document().undo_labels().count(), default_history);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    super::tests::save_ui_snapshot(app.window(), "source-projector-guide-inspector");
    session.undo(Actor::Agent).unwrap();
    refresh_panel(&app, &mut session);
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("cameraspace".into(), Some("1".into()))],
        )
        .unwrap();
    refresh_panel(&app, &mut session);
    assert!(
        app.get_entity_helper_status()
            .contains("cameraspace branches are not evaluated")
    );
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    app.hide().unwrap();
}
