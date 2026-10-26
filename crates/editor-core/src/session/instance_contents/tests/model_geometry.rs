use super::*;
use crate::test_assets::{instance_static_props::MODEL, model_lighting};

fn drawable_props(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId) {
    let (directory, mut session, source, child, owner) = super::static_props::prop_session(actor);
    let (_assets, game) = model_lighting::game();
    let source_model = game.game_dir.join("models/test/receiver");
    let target_model = session.game.game_dir.join("models/props/instance");
    for extension in ["mdl", "vvd", "dx90.vtx"] {
        std::fs::copy(
            source_model.with_extension(extension),
            target_model.with_extension(extension),
        )
        .unwrap();
    }
    let materials = session.game.game_dir.join("materials/models/test");
    std::fs::create_dir_all(&materials).unwrap();
    for material in ["one.vmt", "two.vmt"] {
        std::fs::copy(
            game.game_dir.join("materials/models/test").join(material),
            materials.join(material),
        )
        .unwrap();
    }
    let textures = session.game.game_dir.join("materials/test");
    std::fs::create_dir_all(&textures).unwrap();
    std::fs::copy(
        game.game_dir.join("materials/test/checker.vtf"),
        textures.join("checker.vtf"),
    )
    .unwrap();
    let mut mdl = std::fs::read(target_model.with_extension("mdl")).unwrap();
    for (offset, values) in [
        (104, [0f32; 3]),
        (116, [8., 4., 0.]),
        (128, [0.; 3]),
        (140, [8., 4., 0.]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            mdl[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(target_model.with_extension("mdl"), mdl).unwrap();
    let mut vvd = std::fs::read(target_model.with_extension("vvd")).unwrap();
    for (index, point) in [[0f32, 0., 0.], [8., 0., 0.], [8., 4., 0.], [0., 4., 0.]]
        .into_iter()
        .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            let offset = 64 + index * 48 + 16 + axis * 4;
            vvd[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(target_model.with_extension("vvd"), vvd).unwrap();
    let mut vmf = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    vmf.entities[0].properties.set("origin", "3 5 7");
    vmf.entities[0].properties.set("skin", "1");
    std::fs::write(&child, vmf.write(&Default::default())).unwrap();
    session.select(&[owner], SelectMode::Replace).unwrap();
    (directory, session, source, child, owner)
}

fn nested_props(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId) {
    let (directory, mut session, source, child, owner) = drawable_props(actor);
    let leaf = directory.path().join("leaf.vmf");
    std::fs::rename(&child, &leaf).unwrap();
    let mut middle = Document::empty();
    middle
        .edit("nested repeated props", |tx| {
            for (origin, angles) in [
                (DVec3::new(20., 0., 0.), "0 0 90"),
                (DVec3::new(0., 40., 0.), "0 0 0"),
            ] {
                edit::create_entity(
                    tx,
                    "func_instance",
                    Some(origin),
                    &[
                        ("file".into(), "leaf.vmf".into()),
                        ("angles".into(), angles.into()),
                    ],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&child, middle.vmf().write(&Default::default())).unwrap();
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    session
        .edit(actor, "root instance pose", |tx| {
            tx.entity_mut(id)?.properties.set("origin", "100 200 300");
            tx.entity_mut(id)?.properties.set("angles", "0 90 0");
            Ok(())
        })
        .unwrap();
    (directory, session, source, leaf, owner)
}

fn assert_point(actual: DVec3, expected: [f64; 3]) {
    assert!(
        (actual - DVec3::from_array(expected)).abs().max_element() < 1e-5,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn repeated_nested_models_match_literal_root_geometry_skin_materials_and_owner_selection() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, leaf, owner) = nested_props(actor);
        let before = session.document.vmf().clone();
        let history = session.history_snapshot();
        let sources = [source, leaf].map(|path| (path.clone(), std::fs::read(path).unwrap()));
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready");
        assert_eq!(report["stats"]["static_props"], 2);
        assert_eq!(report["static_prop_preview"]["state"], "not_loaded");
        assert!(session.instance_contents.models.borrow().is_none());
        let preview = session.instance_contents_preview(true);
        assert_eq!(preview.models.len(), 2);
        assert_eq!(preview.report["static_prop_preview"]["available"], true);
        let rows = preview.report["static_prop_preview"]["models"]
            .as_array()
            .unwrap();
        assert_eq!(rows[0]["child_entity"], rows[1]["child_entity"]);
        assert_ne!(rows[0]["occurrence"], rows[1]["occurrence"]);
        assert!(
            preview
                .models
                .iter()
                .all(|instance| instance.id == owner && instance.skin == 1)
        );
        let expected = [
            [
                [107., 223., 305.],
                [107., 231., 305.],
                [107., 231., 309.],
                [107., 223., 309.],
            ],
            [
                [55., 203., 307.],
                [55., 211., 307.],
                [51., 211., 307.],
                [51., 203., 307.],
            ],
        ];
        for (instance, expected) in preview.models.iter().zip(expected) {
            for (vertex, expected) in instance.asset.model.vertices.iter().zip(expected) {
                assert_point(instance.point(vertex.position), expected);
            }
        }
        let mut literal = Document::empty();
        literal
            .edit("literal root controls", |tx| {
                for (origin, angles) in [
                    (DVec3::new(107., 223., 305.), "0 90 90"),
                    (DVec3::new(55., 203., 307.), "0 90 0"),
                ] {
                    edit::create_entity(
                        tx,
                        "prop_static",
                        Some(origin),
                        &[
                            ("model".into(), MODEL.into()),
                            ("skin".into(), "1".into()),
                            ("angles".into(), angles.into()),
                        ],
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let roots = session.model_instances(&literal);
        assert!(roots.issues.is_empty());
        assert!(
            roots
                .instances
                .iter()
                .zip(&preview.models)
                .all(|(root, child)| Arc::ptr_eq(&root.asset, &child.asset))
        );
        let selected = literal
            .vmf()
            .entities
            .iter()
            .map(|entity| ObjectId::Entity(entity.id))
            .collect();
        let control = render::SceneMesh::build_with_models(
            literal.vmf(),
            &session.game,
            &mut session.materials,
            &selected,
            &Default::default(),
            &[],
            &roots.instances,
        );
        let actual = session.build_scene();
        assert_eq!(actual.stats.model_entities, 2);
        let child_vertices = &actual.vertices[actual.vertices.len() - 8..];
        assert_eq!(control.vertices.len(), 8);
        for (actual, control) in child_vertices.iter().zip(&control.vertices) {
            assert_point(
                DVec3::from_array(actual.position.map(f64::from)),
                control.position.map(f64::from),
            );
            assert_point(
                DVec3::from_array(actual.normal.map(f64::from)),
                control.normal.map(f64::from),
            );
            assert_eq!(actual.uv, control.uv);
            assert_eq!(actual.color, control.color);
            assert_eq!(actual.lighting_receiver, 0);
        }
        assert!(
            actual
                .batches
                .iter()
                .any(
                    |batch| session.materials.get(batch.material).name == "models/test/two"
                        && batch.index_count == 12
                )
        );
        let ray = Ray {
            origin: DVec3::new(120., 227., 307.),
            direction: DVec3::NEG_X,
        };
        let hit = session.instance_contents_hits(&ray, Some(true));
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].object, owner);
        assert!(hit[0].solid.is_none() && hit[0].side.is_none());
        assert_point(hit[0].point, [107., 227., 307.]);
        let bounds = session
            .instance_owner_bounds(&session.current_instance_contents().unwrap().owners[0])
            .unwrap();
        assert_point(bounds[0], [51., 203., 305.]);
        assert_point(bounds[1], [107., 231., 309.]);
        session
            .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.)
            .unwrap();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), vec![owner]);
        for (path, bytes) in sources {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn changed_leaf_cancellation_and_model_reload_never_keep_a_complete_old_display() {
    let (_directory, mut session, _source, leaf, owner) = nested_props(Actor::Agent);
    refresh(&mut session, Actor::Agent);
    assert_eq!(session.instance_contents_preview(true).models.len(), 2);
    let mut changed = Vmf::parse(&std::fs::read_to_string(&leaf).unwrap()).unwrap();
    changed.entities[0].properties.set("origin", "3 5 9");
    std::fs::write(&leaf, changed.write(&Default::default())).unwrap();
    let stale = session.instance_contents_preview(true);
    assert!(stale.mesh.is_none() && stale.models.is_empty());
    assert_eq!(stale.report["state"], "stale");
    refresh(&mut session, Actor::Agent);
    let current = session.instance_contents_preview(true);
    assert_eq!(current.models.len(), 2);
    assert_point(current.models[0].origin, [109., 223., 305.]);
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    session
        .cancel_instance_contents_refresh(Actor::Agent, request.token())
        .unwrap();
    let cancelled = session
        .finish_instance_contents_refresh(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    assert!(session.instance_contents_preview(true).models.is_empty());
    assert!(!session.instance_workers_busy());
    refresh(&mut session, Actor::Agent);
    assert_eq!(session.instance_contents_preview(true).models.len(), 2);
    std::fs::write(
        session.game.game_dir.join("models/props/instance.vvd"),
        b"broken VVD",
    )
    .unwrap();
    session.refresh_model_assets(Actor::Agent);
    let unavailable = session.instance_contents_preview(true);
    assert!(unavailable.mesh.is_none() && unavailable.models.is_empty());
    assert_eq!(unavailable.report["state"], "ready");
    assert_eq!(unavailable.report["status"], "model_unavailable");
    assert!(
        unavailable.report["error"]
            .as_str()
            .unwrap()
            .contains("instance.mdl")
    );
    let camera = session.camera();
    assert!(
        session
            .frame_instance_contents(
                Actor::Agent,
                unavailable.report["token"].as_u64().unwrap(),
                &[owner],
                1.
            )
            .is_err()
    );
    assert_eq!(session.camera(), camera);
}

#[test]
fn intermediate_model_box_failure_is_not_hidden_by_a_valid_final_pose() {
    let (directory, mut session, _source, child, owner) = drawable_props(Actor::Agent);
    std::fs::rename(&child, directory.path().join("leaf.vmf")).unwrap();
    let mut middle = Document::empty();
    middle
        .edit("intermediate model box", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::new(16380., 0., 0.)),
                &[("file".into(), "leaf.vmf".into())],
                Vec::new(),
            )
        })
        .unwrap();
    std::fs::write(child, middle.vmf().write(&Default::default())).unwrap();
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    session
        .edit(Actor::Agent, "final pose in bounds", |tx| {
            tx.entity_mut(id)?.properties.set("origin", "-16380 0 0");
            tx.entity_mut(id)?.properties.set("angles", "0 0 0");
            Ok(())
        })
        .unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready");
    let unavailable = session.instance_contents_preview(true);
    assert!(unavailable.mesh.is_none() && unavailable.models.is_empty());
    assert!(
        unavailable.report["error"]
            .as_str()
            .unwrap()
            .contains("pose 1")
    );
}

#[test]
fn repeated_model_payload_budget_refuses_the_entire_reference_display() {
    let (_directory, mut session, _source, _child, _owner) = drawable_props(Actor::Agent);
    let path = session.game.game_dir.join("models/props/instance.vvd");
    let mut vvd = std::fs::read(&path).unwrap();
    let vertex = vvd[64..112].to_vec();
    vvd[16..20].copy_from_slice(&1200i32.to_le_bytes());
    for _ in 4..1200 {
        vvd.extend_from_slice(&vertex);
    }
    std::fs::write(path, vvd).unwrap();
    let asset = session
        .models
        .borrow_mut()
        .get(&session.game, MODEL)
        .unwrap();
    assert_eq!(asset.model.vertices.len(), 1200);
    session
        .edit(Actor::Agent, "repeat model payload", |tx| {
            for _ in 1..instances::MAX_INSTANCES {
                edit::create_entity(
                    tx,
                    "func_instance",
                    Some(DVec3::ZERO),
                    &[("file".into(), "child.vmf".into())],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready");
    assert_eq!(report["stats"]["static_props"], instances::MAX_INSTANCES);
    let unavailable = session.instance_contents_preview(true);
    assert!(unavailable.mesh.is_none() && unavailable.models.is_empty());
    assert!(
        unavailable.report["error"]
            .as_str()
            .unwrap()
            .contains("budget")
    );
    assert_eq!(session.build_scene().stats.model_entities, 0);
}

#[test]
fn actual_model_bounds_control_region_visibility_and_hidden_sources_do_not_load() {
    let (_directory, mut session, _source, child, owner) = drawable_props(Actor::Agent);
    let mut far = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    far.entities[0].properties.set("origin", "300 500 700");
    let mut mixed = Document::new(far);
    mixed
        .edit("mixed reference brush", |tx| {
            edit::create_block(
                tx,
                DVec3::new(320., 500., 710.),
                DVec3::new(328., 508., 718.),
                "test/checker",
            )
        })
        .unwrap();
    std::fs::write(&child, mixed.vmf().write(&Default::default())).unwrap();
    refresh(&mut session, Actor::Agent);
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [DVec3::new(300., 500., 699.), DVec3::new(308., 504., 701.)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    let preview = session.instance_contents_preview(true);
    assert_eq!(preview.models.len(), 1);
    assert!(!preview.mesh.as_ref().unwrap().indices.is_empty());
    let bounds = session
        .instance_owner_bounds(&session.current_instance_contents().unwrap().owners[0])
        .unwrap();
    assert_point(bounds[0], [300., 500., 700.]);
    assert_point(bounds[1], [328., 508., 718.]);
    assert_eq!(session.build_scene().stats.model_entities, 1);
    assert!(preview.owners.contains(&match owner {
        ObjectId::Entity(id) => id,
        _ => unreachable!(),
    }));
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [
                    DVec3::new(1000., 1000., 1000.),
                    DVec3::new(1010., 1010., 1010.),
                ],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(session.instance_contents_preview(true).models.is_empty());
    let mut hidden = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    hidden.entities[0].hidden = true;
    hidden.entities[0]
        .properties
        .set("model", "models/missing.mdl");
    std::fs::write(child, hidden.write(&Default::default())).unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert!(report["stats"].get("static_props").is_none());
    assert!(session.instance_contents_preview(true).models.is_empty());
    assert!(session.inspect_instance_contents(0, 64).unwrap()["error"].is_null());
}
