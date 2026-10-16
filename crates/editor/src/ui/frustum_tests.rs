use super::*;
use source_formats::fgd::Catalog;

#[test]
fn fgd_frustum_panel_shares_evaluated_pitch_default_provenance_and_failures_without_authoring() {
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
@PointClass size(4 4 4) frustum(fov,near,far,tint,0.5) = frustum_guide [
angles(angle) : "Pitch Yaw Roll" : "450 37 22"
fov(float) : "FOV" : 90
near(float) : "NearZ" : 4
far(float) : "FarZ" : 120
tint(color255) : "Guide Color" : "24 210 100 200.5"
]"#,
        )
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "frustum_guide",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    session
        .set_entity_properties(Actor::Agent, id, &[("fov".into(), None)])
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let app = AppWindow::new().unwrap();
    app.set_advanced_properties(true);
    refresh_panel(&app, &mut session);
    let report = session.entity_helper_report(id).unwrap();
    assert_eq!(app.get_entity_helper_status().as_str(), report.status);
    assert!(
        app.get_entity_helper_status()
            .contains("FGD frustum guide: separate; pitch_scale=0.5 (helper_argument)")
    );
    assert!(app.get_entity_helper_status().contains("FGD defaults: fov"));
    assert!(report.frustums[0].pitch_scale_evaluated);
    assert_eq!(
        report.frustums[0].effective_angles,
        Some([-225.0, 37.0, 22.0])
    );
    assert_eq!(report.frustums[0].intensity, Some(200.5));
    assert!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("fov")
            .is_none()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    app.show().unwrap();
    app.window().set_size(slint::PhysicalSize::new(1600, 960));
    super::tests::scroll_inspector_to_control(&app, "Advanced properties");
    super::tests::save_ui_snapshot(app.window(), "fgd-frustum-guide-inspector");
    session
        .set_entity_properties(Actor::Agent, id, &[("fov".into(), Some("179".into()))])
        .unwrap();
    let invalid_source = session.document().vmf().clone();
    let invalid_history = session.document().undo_labels().count();
    refresh_panel(&app, &mut session);
    assert!(
        app.get_entity_helper_status()
            .contains("native clamp branches are not substituted")
    );
    assert!(!session.entity_helper_report(id).unwrap().frustums[0].displayed);
    assert_eq!(session.document().vmf(), &invalid_source);
    assert_eq!(session.document().undo_labels().count(), invalid_history);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    app.hide().unwrap();
}
