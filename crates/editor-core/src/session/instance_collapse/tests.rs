use super::*;
use glam::{DMat3, DVec3};
use map_core::edit;

fn opened() -> (tempfile::TempDir, Session, PathBuf, PathBuf) {
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    (directory, session, parent, child)
}

fn preview(session: &mut Session, actor: Actor) -> u64 {
    let request = session
        .begin_instance_collapse(actor, &session.instance_collapse_stamp())
        .unwrap();
    let token = request.token();
    let report = session
        .finish_instance_collapse(actor, request.run())
        .unwrap();
    assert_eq!(report["ready"], true, "{report}");
    token
}

#[test]
fn instance_collapse_matches_literal_nested_geometry_and_compiler_bsp() {
    let (directory, mut session, parent, child_path) = opened();
    let mut child =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&child_path).unwrap()).unwrap();
    let editor = source_formats::vmf::Properties(vec![
        ("color".into(), "17 99 231".into()),
        ("logicalpos".into(), "[12 24]".into()),
        ("visgroupshown".into(), "1".into()),
    ]);
    child.world.solids[0].editor = Some(editor.clone());
    child.entities[0].editor = Some(editor.clone());
    std::fs::write(&child_path, child.write(&Default::default())).unwrap();
    let mut middle = Document::empty();
    middle
        .edit("nested helper", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::new(4.5, -6.25, 12.125)),
                &[
                    ("file".into(), "child.vmf".into()),
                    ("angles".into(), "90 0 0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let middle_path = directory.path().join("middle.vmf");
    std::fs::write(&middle_path, middle.vmf().write(&Default::default())).unwrap();
    let owner = session
        .document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .unwrap()
        .id;
    session
        .edit(Actor::User, "nested root", |tx| {
            let entity = tx.entity_mut(owner)?;
            entity.editor = Some(editor.clone());
            entity.properties.set("file", "middle.vmf");
            entity.properties.set("origin", "80.125 16.375 80.0625");
            entity.properties.set("angles", "0 90 0");
            Ok(())
        })
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let camera = session.camera();
    let disk = [&parent, &child_path, &middle_path].map(|path| std::fs::read(path).unwrap());
    let token = preview(&mut session, Actor::Agent);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    let shown = session.render_document().vmf().clone();
    assert_eq!(
        shown.world.solids.last().unwrap().editor,
        Some(editor.clone())
    );
    assert_eq!(shown.entities.last().unwrap().editor, Some(editor));
    let matrix = DMat3::from_cols(-DVec3::Z, -DVec3::X, DVec3::Y);
    let offset = DVec3::new(86.375, 20.875, 92.1875);
    let mut literal = Document::new(source.clone());
    literal
        .edit("independent literal flattening", |tx| {
            tx.remove(ObjectId::Entity(owner))?;
            for (original, detail) in child.world.solids.iter().map(|solid| (solid, false)).chain(
                child
                    .entities
                    .iter()
                    .flat_map(|entity| entity.solids.iter().map(|solid| (solid, true))),
            ) {
                let mut solid = original.clone();
                solid.id = tx.new_object_id()?;
                for side in &mut solid.sides {
                    side.id = tx.new_side_id()?;
                    side.plane.0 = side.plane.0.map(|point| matrix * point + offset);
                    for axis in [&mut side.u_axis, &mut side.v_axis] {
                        axis.axis = matrix * axis.axis;
                        axis.offset -= offset.dot(axis.axis) / axis.scale;
                    }
                }
                if detail {
                    let mut entity = Entity::new(tx.new_object_id()?, "func_detail");
                    entity.solids.push(solid);
                    tx.add_entity(entity)?;
                } else {
                    tx.add_solid(solid)?;
                }
            }
            Ok(())
        })
        .unwrap();
    let expected_brushes = literal
        .vmf()
        .world
        .solids
        .iter()
        .chain(
            literal
                .vmf()
                .entities
                .iter()
                .filter(|entity| entity.classname() == Some("func_detail"))
                .flat_map(|entity| &entity.solids),
        )
        .collect::<Vec<_>>();
    let actual_brushes = shown
        .world
        .solids
        .iter()
        .chain(
            shown
                .entities
                .iter()
                .filter(|entity| entity.classname() == Some("func_detail"))
                .flat_map(|entity| &entity.solids),
        )
        .collect::<Vec<_>>();
    assert_eq!(actual_brushes.len(), expected_brushes.len());
    for (actual, expected) in actual_brushes.iter().zip(expected_brushes) {
        for (actual, expected) in actual.sides.iter().zip(&expected.sides) {
            for (actual, expected) in actual.plane.0.iter().zip(expected.plane.0) {
                assert!(actual.abs_diff_eq(expected, 1e-8));
            }
            for (actual, expected) in [
                (actual.u_axis, expected.u_axis),
                (actual.v_axis, expected.v_axis),
            ] {
                assert!(actual.axis.abs_diff_eq(expected.axis, 1e-10));
                assert!((actual.offset - expected.offset).abs() < 1e-8);
                assert_eq!(actual.scale, expected.scale);
            }
        }
    }
    let control = Control::default();
    let source_context = instances::SourceContext::capture(&parent, &control).unwrap();
    let original_bsp = compiler::compile_with_source_controlled(
        &source,
        &session.game,
        compiler::Options::default(),
        None,
        Some(&source_context),
        &control,
    );
    let actual_bsp = compiler::compile(&shown, &session.game);
    assert!(
        original_bsp.bsp.is_some(),
        "{:?}",
        original_bsp.report.diagnostics
    );
    assert!(
        actual_bsp.bsp.is_some(),
        "{:?}",
        actual_bsp.report.diagnostics
    );
    assert_eq!(
        original_bsp.bsp.unwrap().to_bytes(),
        actual_bsp.bsp.unwrap().to_bytes()
    );
    session
        .cancel_instance_collapse(Actor::Agent, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &source);
    let token = preview(&mut session, Actor::Agent);
    let request = session
        .apply_instance_collapse(Actor::Agent, token)
        .unwrap();
    session
        .finish_instance_collapse(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(session.document.vmf(), &shown);
    assert_eq!(session.document.undo_labels().count(), history + 1);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.camera(), camera);
    for (path, bytes) in [&parent, &child_path, &middle_path].into_iter().zip(disk) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn instance_collapse_rechecks_sources_and_keeps_the_previous_preview_on_failure() {
    let (_directory, mut session, parent, child) = opened();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let bytes = std::fs::read(&child).unwrap();
    let parent_bytes = std::fs::read(&parent).unwrap();
    let token = preview(&mut session, Actor::User);
    let shown = session.render_document().vmf().clone();
    std::fs::write(&child, b"changed dependency").unwrap();
    let request = session.apply_instance_collapse(Actor::User, token).unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.history_snapshot(), history);
    let report = session
        .review_instance_collapse(Actor::Agent, token)
        .unwrap();
    let next = report["token"].as_u64().unwrap();
    assert_ne!(next, token);
    assert!(
        session
            .cancel_instance_collapse(Actor::User, token)
            .is_err()
    );
    let request = session
        .begin_instance_collapse(Actor::Agent, &session.instance_collapse_stamp())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::Agent, request.run())
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.document.vmf(), &source);
    std::fs::write(&child, &bytes).unwrap();
    let token = preview(&mut session, Actor::Agent);
    session
        .cancel_instance_collapse(Actor::Agent, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &source);
    assert_eq!(std::fs::read(parent).unwrap(), parent_bytes);
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let token = request.token();
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
}

#[test]
fn instance_collapse_includes_all_visible_roots_and_preserves_terrain_and_hidden_roots() {
    let (_directory, mut session, parent, child_path) = opened();
    let mut child = Document::try_new(
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&child_path).unwrap()).unwrap(),
    )
    .unwrap();
    let solid = child.vmf().world.solids[0].id;
    let side = child
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    child
        .edit("terrain", |tx| {
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )
        })
        .unwrap();
    let terrain = child
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find_map(|side| side.displacement.clone())
        .unwrap();
    std::fs::write(&child_path, child.vmf().write(&Default::default())).unwrap();
    let original = session
        .document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .unwrap()
        .id;
    let hidden = session
        .edit(Actor::User, "other roots", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::new(48.0, 0.0, 0.0)),
                &[("file".into(), "child.vmf".into())],
                Vec::new(),
            )?;
            let mut hidden = Entity::new(tx.new_object_id()?, "func_instance");
            hidden.hidden = true;
            hidden.properties.set("file", "missing-hidden.vmf");
            let copy = hidden.clone();
            tx.add_entity(hidden)?;
            Ok(copy)
        })
        .unwrap();
    session
        .select(&[ObjectId::Entity(original)], SelectMode::Replace)
        .unwrap();
    let source = session.document.vmf().clone();
    let token = preview(&mut session, Actor::User);
    assert_eq!(session.get_instance_collapse()["stats"]["instances"], 2);
    let shown = session.render_document().vmf();
    assert_eq!(
        shown
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("func_instance"))
            .collect::<Vec<_>>(),
        vec![&hidden]
    );
    assert_eq!(
        shown
            .world
            .solids
            .iter()
            .flat_map(|solid| &solid.sides)
            .find_map(|side| side.displacement.as_ref()),
        Some(&terrain)
    );
    let mut literal = Document::new(source.clone());
    literal
        .edit("literal translated children", |tx| {
            for entity in source
                .entities
                .iter()
                .filter(|entity| !entity.hidden && entity.classname() == Some("func_instance"))
            {
                let delta = entity.origin().unwrap_or(DVec3::ZERO);
                tx.remove(ObjectId::Entity(entity.id))?;
                for (original, detail) in child
                    .vmf()
                    .world
                    .solids
                    .iter()
                    .map(|solid| (solid, false))
                    .chain(
                        child
                            .vmf()
                            .entities
                            .iter()
                            .flat_map(|entity| entity.solids.iter().map(|solid| (solid, true))),
                    )
                {
                    let mut solid = original.clone();
                    solid.id = tx.new_object_id()?;
                    for side in &mut solid.sides {
                        side.id = tx.new_side_id()?;
                        side.plane.0 = side.plane.0.map(|point| point + delta);
                        for axis in [&mut side.u_axis, &mut side.v_axis] {
                            axis.offset -= delta.dot(axis.axis) / axis.scale;
                        }
                        if let Some(info) = &mut side.displacement {
                            info.start_position += delta;
                        }
                    }
                    if detail {
                        let mut generated = Entity::new(tx.new_object_id()?, "func_detail");
                        generated.solids.push(solid);
                        tx.add_entity(generated)?;
                    } else {
                        tx.add_solid(solid)?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    let actual_bsp = compiler::compile(shown, &session.game);
    let literal_bsp = compiler::compile(literal.vmf(), &session.game);
    assert!(
        actual_bsp.bsp.is_some(),
        "{:?}",
        actual_bsp.report.diagnostics
    );
    assert!(
        literal_bsp.bsp.is_some(),
        "{:?}",
        literal_bsp.report.diagnostics
    );
    assert_eq!(
        actual_bsp.bsp.unwrap().to_bytes(),
        literal_bsp.bsp.unwrap().to_bytes()
    );
    let mut invalid = child.vmf().clone();
    let mut light = Entity::new(invalid.object_ids().max().unwrap() + 1, "light");
    light.properties.set("origin", "0 0 64");
    light.properties.set("_light", "255 240 220 300");
    invalid.entities.push(light);
    std::fs::write(&child_path, invalid.write(&Default::default())).unwrap();
    let control = Control::default();
    let context = instances::SourceContext::capture(&parent, &control).unwrap();
    let mut report = compiler::Report::default();
    assert!(instances::resolve(&source, Some(&context), &control, &mut report).is_ok());
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    let current = session.get_instance_collapse()["token"].as_u64().unwrap();
    assert!(
        session
            .cancel_instance_collapse(Actor::User, token)
            .is_err()
    );
    session
        .cancel_instance_collapse(Actor::User, current)
        .unwrap();
}

#[test]
fn instance_collapse_checks_parent_bindings_in_worker_without_partial_preview() {
    let (_directory, mut session, _parent, child) = opened();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = session.path.take().unwrap();
        session.path = Some(path.with_file_name(std::ffi::OsString::from_vec(vec![b'a', 0xff])));
        let error = session
            .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
            .err()
            .unwrap();
        assert!(error.contains("UTF-8 path"));
        assert!(session.get_instance_collapse()["token"].is_null());
        session.path = Some(path);
    }

    let owner = session
        .document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .unwrap()
        .id;
    session
        .edit(Actor::User, "parent helper binding", |tx| {
            tx.entity_mut(owner)?.properties.set("targetname", "module");
            let mut inbound = Entity::new(tx.new_object_id()?, "logic_relay");
            inbound.hidden = true;
            inbound.properties.set("unknown_text", "MODULE");
            tx.add_entity(inbound)?;
            Ok(())
        })
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let token = request.token();
    std::fs::remove_file(child).unwrap();
    let error = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(
        error.contains("parent reference targets an instance helper"),
        "{error}"
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.get_instance_collapse()["ready"], false);
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
}
