use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::DVec3;
use map_core::ObjectId;
use map_core::{edit::Transform, query::Ray};
use source_formats::fgd::Catalog;

const FGD: &str = r#"@BaseClass = values [
angles(angle) : "Angles" : "450 37 22"
lightfov(float) : "FOV" : 90
nearz(float) : "NearZ" : 4
farz(float) : "FarZ" : 120
lightcolor(color255) : "Light Color" : "12 200 80 200.5"
target(target_destination) : "target"
parentname(target_destination) : "Parent"
cameraspace(integer) : "Camera Space" : 0
]
@PointClass base(values) size(4 4 4) frustum(lightfov,nearz,farz,lightcolor,0.5) = guide []
@PointClass base(values) size(4 4 4) frustum(lightfov,nearz,farz,lightcolor,-1) = env_projectedtexture []"#;

fn fixture(actor: Actor, class: &str) -> (Session, u32) {
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(actor, class, Some(DVec3::ZERO), BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    (session, id)
}

fn vertices(mesh: &render::SceneMesh) -> Vec<[f32; 3]> {
    mesh.lines
        .iter()
        .filter(|v| v.color == [12, 200, 80, 255])
        .map(|v| v.position)
        .collect()
}

#[test]
fn fgd_frustum_inspection_scene_preview_and_ordinary_bounds_share_read_only_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, id) = fixture(actor, "guide");
        let object = ObjectId::Entity(id);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session.object_bounds_in(session.document(), &[object]);
        let report = session.entity_helper_report(id).unwrap();
        assert!(!report.unsupported.contains(&"frustum".into()));
        let row = &report.frustums[0];
        assert_eq!(
            (row.state, row.presentation, row.displayed),
            ("ready", "separate", true)
        );
        assert_eq!(row.pitch_scale, Some(0.5));
        assert!(row.pitch_scale_evaluated);
        assert_eq!(row.effective_angles, Some([-225.0, 37.0, 22.0]));
        let scene = session.build_scene();
        assert_eq!(vertices(&scene).len(), 24);
        assert_eq!(
            vertices(&scene)[0],
            DVec3::from_array(row.corners.unwrap()[0])
                .as_vec3()
                .to_array()
        );
        assert_eq!(
            session
                .pick(&Ray {
                    origin: DVec3::new(0.0, -100.0, 0.0),
                    direction: DVec3::Y
                })
                .unwrap()
                .object,
            object
        );
        session
            .preview_transform(
                actor,
                &[object],
                Transform {
                    translation: DVec3::new(16.0, 24.0, 32.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let preview = session.entity_helper_report(id).unwrap().frustums.remove(0);
        assert_eq!(preview.origin, Some([16.0, 24.0, 32.0]));
        assert_eq!(
            vertices(&session.build_scene())[0],
            DVec3::from_array(preview.corners.unwrap()[0])
                .as_vec3()
                .to_array()
        );
        session.cancel_transform_preview();
        session.select(&[], SelectMode::Replace).unwrap();
        let plain = session.build_scene();
        assert!(vertices(&plain).is_empty());
        assert_eq!((scene.mins, scene.maxs), (plain.mins, plain.maxs));
        assert_eq!(
            session.entity_helper_report(id).unwrap().frustums[0].suppression_reason,
            Some("not_selected")
        );
        assert_eq!(
            session.object_bounds_in(session.document(), &[object]),
            bounds
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn fgd_frustum_stock_source_geometry_is_shared_and_custom_or_unavailable_branches_cannot_bypass_precedence()
 {
    let (mut session, id) = fixture(Actor::Agent, "env_projectedtexture");
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let report = session.entity_helper_report(id).unwrap();
    let source = report.source_projector.unwrap();
    assert!(source.displayed && !source.helper_pitch_scale_evaluated);
    assert_eq!(report.frustums[0].corners, source.corners);
    assert_eq!(
        report.frustums[0].presentation,
        "shared_with_source_projector"
    );
    assert!(report.frustums[0].displayed && report.frustums[0].pitch_scale_evaluated);
    assert_eq!(vertices(&session.build_scene()).len(), 24);
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&FGD.replace(",-1)", ",0.5)")).unwrap(),
    );
    let report = session.entity_helper_report(id).unwrap();
    assert!(report.source_projector.unwrap().displayed);
    assert_eq!(report.frustums[0].state, "ready");
    assert_eq!(
        report.frustums[0].suppression_reason,
        Some("source_projector_precedence")
    );
    assert_eq!(vertices(&session.build_scene()).len(), 24);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    session
        .set_entity_properties(Actor::Agent, id, &[("target".into(), Some("aim".into()))])
        .unwrap();
    let report = session.entity_helper_report(id).unwrap();
    assert!(!report.source_projector.unwrap().displayed);
    assert_eq!(
        report.frustums[0].suppression_reason,
        Some("source_projector_precedence")
    );
    assert!(vertices(&session.build_scene()).is_empty());
}

#[test]
fn fgd_frustum_defaults_failures_visibility_and_model_precedence_are_explicit() {
    let (mut session, id) = fixture(Actor::Agent, "guide");
    session
        .set_entity_properties(Actor::Agent, id, &[("lightfov".into(), None)])
        .unwrap();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let report = session.entity_helper_report(id).unwrap();
    assert_eq!(report.frustums[0].inputs[2].value_source, "fgd_default");
    assert!(report.status.contains("FGD defaults: lightfov"));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&FGD.replace("size(4 4 4)", "size(4 4 4) studio(missing.mdl)")).unwrap(),
    );
    assert_eq!(
        session.entity_helper_report(id).unwrap().frustums[0].suppression_reason,
        Some("model_or_sprite_helper")
    );
    assert!(vertices(&session.build_scene()).is_empty());
    session.set_definitions(Actor::Agent, Catalog::parse(FGD).unwrap());
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [DVec3::splat(80.0), DVec3::splat(144.0)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        session.entity_helper_report(id).unwrap().frustums[0].suppression_reason,
        Some("outside_working_region")
    );
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    session
        .edit(Actor::Agent, "hide guide", |tx| {
            tx.entity_mut(id)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        session.entity_helper_report(id).unwrap().frustums[0].suppression_reason,
        Some("authored_hidden")
    );
    session.undo(Actor::Agent).unwrap();
    session
        .set_entity_properties(Actor::Agent, id, &[("lightfov".into(), Some("179".into()))])
        .unwrap();
    let report = session.entity_helper_report(id).unwrap();
    assert_eq!(report.frustums[0].state, "invalid");
    assert!(
        report.frustums[0]
            .error
            .as_ref()
            .unwrap()
            .contains("not substituted")
    );
    assert!(vertices(&session.build_scene()).is_empty());
}

#[test]
fn fgd_frustum_declaration_and_scene_budgets_report_omissions_without_expanding_bounds() {
    let (mut session, id) = fixture(Actor::Agent, "guide");
    let declarations = (0..=frustums::MAX_DECLARATIONS)
        .map(|scale| format!("frustum(lightfov,nearz,farz,lightcolor,{scale}) "))
        .collect::<String>();
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&FGD.replace("frustum(lightfov,nearz,farz,lightcolor,0.5)", &declarations))
            .unwrap(),
    );
    let report = session.entity_helper_report(id).unwrap();
    assert_eq!(report.frustums.len(), 16);
    assert_eq!(report.omitted_frustum_declarations, 1);
    assert_eq!(vertices(&session.build_scene()).len(), 16 * 24);
    session.set_definitions(Actor::Agent, Catalog::parse(FGD).unwrap());
    let mut ids = vec![ObjectId::Entity(id)];
    for index in 0..MAX_DISPLAYED_FRUSTUMS {
        ids.push(
            session
                .create_entity(
                    Actor::Agent,
                    "guide",
                    Some(DVec3::X * index as f64),
                    BTreeMap::new(),
                )
                .unwrap(),
        );
    }
    session.select(&ids, SelectMode::Replace).unwrap();
    assert_eq!(
        vertices(&session.build_scene()).len(),
        MAX_DISPLAYED_FRUSTUMS * 24
    );
    let ObjectId::Entity(last) = *ids.last().unwrap() else {
        panic!()
    };
    assert_eq!(
        session.entity_helper_report(last).unwrap().frustums[0].suppression_reason,
        Some("scene_guide_limit")
    );
}
