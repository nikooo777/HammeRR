use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::DVec3;
use map_core::ObjectId;
use map_core::{edit::Transform, query::Ray};
use source_formats::fgd::Catalog;

const FGD: &str = r#"@PointClass size(4 4 4)
frustum(lightfov,nearz,farz,lightcolor,-1) = env_projectedtexture [
angles(angle) : "Pitch Yaw Roll" : "0 0 0"
lightfov(float) : "FOV" : 90
nearz(float) : "NearZ" : 4
farz(float) : "FarZ" : 120
lightcolor(color255) : "Light Color" : "12 200 80 200"
target(target_destination) : "target"
parentname(target_destination) : "Parent"
cameraspace(integer) : "Camera Space" : 0
]"#;

fn fixture(actor: Actor) -> (Session, u32) {
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(
            actor,
            "env_projectedtexture",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    (session, id)
}

fn vertices(mesh: &render::SceneMesh) -> Vec<render::scene::LineVertex> {
    mesh.lines
        .iter()
        .filter(|vertex| vertex.color == [12, 200, 80, 255])
        .copied()
        .collect()
}

#[test]
fn source_projector_inspection_scene_preview_and_bounds_share_read_only_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, id) = fixture(actor);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let object = ObjectId::Entity(id);
        let bounds = session.object_bounds_in(session.document(), &[object]);
        let scene = session.build_scene();
        assert_eq!(vertices(&scene).len(), 24);
        let inspector = session.entity_inspector(id).unwrap().helpers.unwrap();
        let report = inspector.source_projector.unwrap();
        assert!(report.displayed && report.state == "ready");
        assert!(!report.helper_pitch_scale_evaluated);
        assert_eq!(report.helper_pitch_scales[0].raw.as_deref(), Some("-1"));
        assert_eq!(report.intensity, Some(200.0));
        assert!(
            report
                .orientation_policy
                .contains("Positive pitch points down")
        );
        assert!(inspector.status.contains("pitch_scale is not evaluated"));
        let body = Ray {
            origin: DVec3::new(0.0, -100.0, 0.0),
            direction: DVec3::Y,
        };
        assert_eq!(session.pick(&body).unwrap().object, object);
        assert!(
            session
                .pick(&Ray {
                    origin: DVec3::new(60.0, -200.0, 0.0),
                    direction: DVec3::Y
                })
                .is_none()
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
        let preview = session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap();
        assert_eq!(preview.origin, Some([16.0, 24.0, 32.0]));
        assert_eq!(
            vertices(&session.build_scene())[0].position,
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
            scene
                .vertices
                .iter()
                .map(|v| v.position)
                .collect::<Vec<_>>(),
            plain
                .vertices
                .iter()
                .map(|v| v.position)
                .collect::<Vec<_>>()
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
fn source_projector_visibility_regions_and_model_precedence_explain_suppression() {
    let (mut session, id) = fixture(Actor::Agent);
    session
        .edit(Actor::Agent, "hide source", |tx| {
            tx.entity_mut(id)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap()
            .suppression_reason,
        Some("authored_hidden")
    );
    assert!(vertices(&session.build_scene()).is_empty());
    session.undo(Actor::Agent).unwrap();
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [
                    DVec3::new(80.0, -32.0, -32.0),
                    DVec3::new(144.0, 32.0, 32.0),
                ],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap()
            .suppression_reason,
        Some("outside_working_region")
    );
    assert!(vertices(&session.build_scene()).is_empty());
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&FGD.replace("size(4 4 4)", "size(4 4 4) studio(missing.mdl)")).unwrap(),
    );
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap()
            .suppression_reason,
        Some("model_helper")
    );
    assert!(vertices(&session.build_scene()).is_empty());
}

#[test]
fn source_projector_defaults_reload_color_brightness_and_unsupported_inputs_preserve_authorship() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, id) = fixture(actor);
        session
            .set_entity_properties(actor, id, &[("lightfov".into(), None)])
            .unwrap();
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        session.set_definitions(
            actor,
            Catalog::parse(&FGD.replace(": 90", ": 60").replace(",-1)", ",100)")).unwrap(),
        );
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap();
        assert_eq!(report.fov_degrees, Some(60.0));
        assert_eq!(report.inputs[2].value_source, "fgd_default");
        assert_eq!(report.helper_pitch_scales[0].raw.as_deref(), Some("100"));
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .set_entity_color(actor, id, "lightcolor", [31, 150, 91], None)
            .unwrap();
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("lightcolor"),
            Some("31 150 91 200")
        );
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap();
        assert_eq!(report.color, Some([31, 150, 91]));
        assert_eq!(report.intensity, Some(200.0));
        assert_eq!(report.inputs[5].raw.as_deref(), Some("31 150 91 200"));
        session
            .set_entity_properties(actor, id, &[("target".into(), Some("aim".into()))])
            .unwrap();
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .unwrap();
        assert_eq!(report.state, "unsupported");
        assert!(report.error.unwrap().contains("target"));
        assert!(report.corners.is_none());
    }
}

#[test]
fn source_projector_scene_budget_and_non_projector_classes_are_explicit() {
    let (mut session, id) = fixture(Actor::Agent);
    let mut ids = vec![ObjectId::Entity(id)];
    for index in 0..MAX_DISPLAYED_PROJECTORS {
        ids.push(
            session
                .create_entity(
                    Actor::Agent,
                    "env_projectedtexture",
                    Some(DVec3::X * index as f64),
                    BTreeMap::new(),
                )
                .unwrap(),
        );
    }
    session.select(&ids, SelectMode::Replace).unwrap();
    assert_eq!(
        vertices(&session.build_scene()).len(),
        24 * MAX_DISPLAYED_PROJECTORS
    );
    let ObjectId::Entity(last) = *ids.last().unwrap() else {
        panic!()
    };
    assert_eq!(
        session
            .entity_helper_report(last)
            .unwrap()
            .source_projector
            .unwrap()
            .suppression_reason,
        Some("scene_guide_limit")
    );
    session
        .edit(Actor::Agent, "different class", |tx| {
            tx.entity_mut(id)?.properties.set("classname", "light_spot");
            Ok(())
        })
        .unwrap();
    assert!(
        session
            .entity_helper_report(id)
            .unwrap()
            .source_projector
            .is_none()
    );
}
