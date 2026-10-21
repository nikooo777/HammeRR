use super::*;
use crate::Report;
use crate::physics::{Body, MASK_SOLID, Model, default_materials, shape};
use map_core::{Document, ObjectId, edit};
use source_formats::gamefs::GameFs;

fn source() -> Vmf {
    let mut document = Document::empty();
    document
        .edit("lantern post", |transaction| {
            let ObjectId::Solid(id) = edit::create_block(
                transaction,
                DVec3::new(106.0, -334.0, 58.0),
                DVec3::new(114.0, -326.0, 166.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                transaction,
                &[id],
                "func_detail",
                &[("targetname".into(), "detail_misc".into())],
            )?;
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn request(map: &Map) -> Request {
    Request {
        model_count: 1,
        models: vec![Model {
            index: 0,
            bodies: vec![Body {
                contents: MASK_SOLID,
                shapes: vec![
                    shape(
                        map,
                        0,
                        None,
                        &mut crate::physics::topology::Budget::default(),
                        &mut Report::default(),
                    )
                    .unwrap(),
                ],
                surface: None,
                current_hu: None,
                damping: None,
                surface_property: None,
                mass_area: None,
                mesh: None,
            }],
        }],
        materials: default_materials(),
        surface_data: Vec::new(),
    }
}

#[test]
fn structured_failure_uses_authored_solid_and_actual_consumed_entity_owner() {
    let source = source();
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let map = Map::load(&source, &game, &mut Report::default()).unwrap();
    let request = request(&map);
    crate::physics::validate(&request).unwrap();
    let failure = Failure::shape(Stage::ConvexConstruction, 0, "native rejection").context(0, 0);
    let encoded = serde_json::to_vec(&failure).unwrap();
    let decoded: Failure = serde_json::from_slice(&encoded).unwrap();
    let diagnostic = decoded.diagnostic(&source, &map, &[], &request).unwrap();
    let brush = &map.brushes[0];
    assert_eq!(diagnostic.object, Some(format!("solid:{}", brush.id)));
    assert_eq!(diagnostic.position, Some(DVec3::new(110.0, -330.0, 112.0)));
    assert!(diagnostic.message.contains("convex construction"));
    assert!(diagnostic.message.contains("native rejection"));
    assert!(diagnostic.message.contains("func_detail"));
    assert!(diagnostic.message.contains("detail_misc"));
    assert!(
        diagnostic
            .message
            .contains(&format!("VMF entity {}", source.entities[0].id))
    );
    assert!(diagnostic.message.contains("[106.0, -334.0, 58.0]"));
    assert!(diagnostic.message.contains("[114.0, -326.0, 166.0]"));
}

#[test]
fn generated_fluid_failures_do_not_claim_authored_solid_identity() {
    let source = source();
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let map = Map::load(&source, &game, &mut Report::default()).unwrap();
    let mut request = request(&map);
    let body = &mut request.models[0].bodies[0];
    body.surface = Some(166.0);
    body.contents = source_formats::bsp::contents::WATER;
    body.shapes[0].brush = map.brushes.len() as u32 + 7;
    let diagnostic = Failure::shape(Stage::GeometryVerification, body.shapes[0].brush, "cropped")
        .context(0, 0)
        .diagnostic(&source, &map, &[], &request)
        .unwrap();
    assert!(diagnostic.object.is_none());
    assert!(diagnostic.message.contains("generated fluid hull"));
    assert!(diagnostic.message.contains("world wet leaf 7"));
    assert!(!diagnostic.message.contains("VMF solid"));
    assert!(!diagnostic.message.contains("func_detail"));
}

#[test]
fn failure_context_must_match_the_frozen_request_and_protocol() {
    let source = source();
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let map = Map::load(&source, &game, &mut Report::default()).unwrap();
    let request = request(&map);
    let valid =
        Failure::shape(Stage::GeometryVerification, 0, "brush 99 is free-form text").context(0, 0);
    let diagnostic = valid.diagnostic(&source, &map, &[], &request).unwrap();
    assert_eq!(
        diagnostic.object,
        Some(format!("solid:{}", map.brushes[0].id))
    );
    for case in 0..7 {
        let mut failure = valid.clone();
        match case {
            0 => failure.version += 1,
            1 => failure.model = Some(1),
            2 => failure.body = Some(1),
            3 => failure.tag = Some(99),
            4 => failure.body = None,
            5 => failure.message.clear(),
            6 => failure.stage = Stage::Library,
            _ => unreachable!(),
        }
        assert!(failure.diagnostic(&source, &map, &[], &request).is_err());
    }
    let unknown = serde_json::to_value(&valid).unwrap();
    let mut unknown = unknown.as_object().unwrap().clone();
    unknown.insert("unexpected".into(), true.into());
    assert!(serde_json::from_value::<Failure>(unknown.into()).is_err());
}

#[test]
fn structured_solid_identity_retains_instance_leaf_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = source();
    child.entities[0].properties.remove("targetname");
    let child_id = child.entities[0].solids[0].id;
    std::fs::write(
        directory.path().join("child.vmf"),
        child.write(&Default::default()),
    )
    .unwrap();
    let mut parent = Document::empty().into_vmf();
    let helper_id = parent.object_ids().max().unwrap() + 1;
    let mut helper = Entity::new(helper_id, "func_instance");
    helper.properties.set("file", "child.vmf");
    helper.properties.set("origin", "32 0 0");
    parent.entities.push(helper);
    let path = directory.path().join("parent.vmf");
    std::fs::write(&path, parent.write(&Default::default())).unwrap();
    let control = crate::Control::default();
    let context = crate::instances::SourceContext::capture(&path, &control).unwrap();
    let mut report = Report::default();
    let prepared =
        crate::instances::prepare(&parent, Some(&context), &control, &mut report).unwrap();
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let map = Map::load(prepared.vmf(), &game, &mut report).unwrap();
    let request = request(&map);
    let diagnostic = Failure::shape(Stage::GeometryVerification, 0, "expanded")
        .context(0, 0)
        .diagnostic(prepared.vmf(), &map, &[], &request)
        .unwrap();
    report.diagnostics.push(diagnostic);
    prepared.annotate(&mut report);
    let diagnostic = report.diagnostics.last().unwrap();
    assert_eq!(diagnostic.object, Some(format!("entity:{helper_id}")));
    assert_eq!(diagnostic.position, Some(DVec3::new(142.0, -330.0, 112.0)));
    assert!(diagnostic.message.contains("instance child.vmf"));
    assert!(
        diagnostic
            .message
            .contains(&format!("child object {child_id}"))
    );
}
