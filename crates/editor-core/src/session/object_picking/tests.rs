use super::*;
use glam::DVec3;
use map_core::{edit, groups};

fn ray() -> Ray {
    Ray {
        origin: DVec3::ZERO,
        direction: DVec3::X * 7.0,
    }
}

fn block(session: &mut Session, actor: Actor, x: f64, material: &str) -> ObjectId {
    session
        .create_block(
            actor,
            DVec3::new(x, -8.0, -8.0),
            DVec3::new(x + 8.0, 8.0, 8.0),
            material,
            false,
        )
        .unwrap()
}

fn group(session: &mut Session, actor: Actor, members: Vec<groups::Member>) -> u32 {
    session
        .edit(actor, "group candidates", |tx| {
            groups::apply(
                tx,
                &groups::Edit {
                    group: None,
                    name: "Cycle group".into(),
                    parent: None,
                    members,
                    ungroup: false,
                },
            )
        })
        .unwrap()
}

#[test]
fn both_actors_cycle_depth_order_wrap_and_reset_from_the_exact_shared_selection() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets required".into()));
        let far = block(&mut session, actor, 80.0, "test");
        let near = block(&mut session, actor, 24.0, "test");
        let middle = block(&mut session, actor, 48.0, "test");
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let camera = session.camera();
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(report["total"], 3);
        assert_eq!(report["truncated"], false);
        assert_eq!(
            report["candidates"][0]["object"],
            crate::identity::id_string(near)
        );
        assert_eq!(
            report["candidates"][1]["object"],
            crate::identity::id_string(middle)
        );
        assert_eq!(session.pick(&ray()).unwrap().object, near);
        for (index, id) in [(0, near), (1, middle), (2, far), (0, near)] {
            let report = session.cycle_object_hit(actor, &ray()).unwrap();
            assert_eq!(report["chosen"], index);
            assert_eq!(
                session.recent_log().next().unwrap().text,
                report["message"].as_str().unwrap()
            );
            assert_eq!(session.selected_ids(), [id]);
        }
        session.select(&[near, far], SelectMode::Replace).unwrap();
        assert_eq!(
            session.cycle_object_hit(actor, &ray()).unwrap()["chosen"],
            0
        );
        session.select(&[middle], SelectMode::Replace).unwrap();
        assert_eq!(
            session.cycle_object_hit(actor, &ray()).unwrap()["chosen"],
            2
        );
        let before = session.selected_ids();
        let miss = Ray {
            origin: DVec3::new(0.0, 128.0, 0.0),
            ..ray()
        };
        let log_serial = session.log_serial();
        assert!(session.cycle_object_hit(actor, &miss).unwrap()["chosen"].is_null());
        assert_eq!(session.log_serial(), log_serial);
        assert_eq!(session.selected_ids(), before);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.camera(), camera);
    }
}

#[test]
fn owner_and_outer_group_deduplication_precedes_the_limit_and_respects_bypass() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets required".into()));
        let mut members = Vec::new();
        for index in 0..70 {
            members.push(block(
                &mut session,
                actor,
                16.0 + index as f64 * 12.0,
                "test",
            ));
        }
        let ObjectId::Solid(a) = members[0] else {
            unreachable!()
        };
        let ObjectId::Solid(b) = members[1] else {
            unreachable!()
        };
        let owner = session
            .edit(actor, "owned candidates", |tx| {
                edit::tie_to_entity(tx, &[a, b], "func_brush", &[])
            })
            .unwrap();
        let inner = group(
            &mut session,
            actor,
            members[2..35]
                .iter()
                .copied()
                .map(groups::Member::Object)
                .collect(),
        );
        group(
            &mut session,
            actor,
            std::iter::once(groups::Member::Group(inner))
                .chain(members[35..].iter().copied().map(groups::Member::Object))
                .collect(),
        );
        let far = block(&mut session, actor, 1000.0, "test");
        session.select(&[], SelectMode::Replace).unwrap();
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(report["total"], 3);
        assert_eq!(report["truncated"], false);
        assert_eq!(
            report["candidates"][0]["object"],
            crate::identity::id_string(owner)
        );
        assert_eq!(
            report["candidates"][1]["selection_unit"]
                .as_array()
                .unwrap()
                .len(),
            68
        );
        assert_eq!(
            report["candidates"][2]["object"],
            crate::identity::id_string(far)
        );
        session.cycle_object_hit(actor, &ray()).unwrap();
        session.cycle_object_hit(actor, &ray()).unwrap();
        assert_eq!(session.selected_ids().len(), 68);
        assert_eq!(
            session.cycle_object_hit(actor, &ray()).unwrap()["chosen"],
            2
        );
        session.set_group_selection_bypass(actor, true);
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(report["total"], 70);
        assert_eq!(report["truncated"], true);
        assert_eq!(
            report["candidates"].as_array().unwrap().len(),
            MAX_CANDIDATES
        );
        assert!(
            report["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate["selection_unit"].as_array().unwrap().len() == 1)
        );
        let ObjectId::Entity(owner_id) = owner else {
            unreachable!()
        };
        session
            .edit(actor, "hide owned child", |tx| {
                tx.entity_mut(owner_id)?
                    .solids
                    .iter_mut()
                    .find(|solid| solid.id == a)
                    .unwrap()
                    .hidden = true;
                Ok(())
            })
            .unwrap();
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(
            report["candidates"][0]["object"],
            crate::identity::id_string(owner)
        );
        assert_eq!(report["candidates"][0]["solid"], b);
        session
            .edit(actor, "hide owner", |tx| {
                tx.entity_mut(owner_id)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(report["total"], 69);
        assert!(
            report["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate["object"] != crate::identity::id_string(owner))
        );
    }
}

#[test]
fn equal_depth_ids_hidden_children_and_tool_filters_are_deterministic() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game) = crate::test_assets::game();
        std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
        std::fs::write(
            directory.path().join("materials/tools/toolsnodraw.vmt"),
            "LightmappedGeneric { %compilenodraw 1 }",
        )
        .unwrap();
        let mut session = Session::new(Ok(game));
        let first = block(&mut session, actor, 32.0, "test");
        let second = block(&mut session, actor, 32.0, "test");
        session.document.vmf().validate_ids().unwrap();
        let report = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(
            report["candidates"][0]["object"],
            crate::identity::id_string(first)
        );
        assert_eq!(
            report["candidates"][1]["object"],
            crate::identity::id_string(second)
        );
        let ordinary = session.pick(&ray()).unwrap();
        assert_eq!(ordinary.object, first);
        let ObjectId::Solid(id) = first else {
            unreachable!()
        };
        session
            .edit(actor, "hide near", |tx| {
                tx.solid_mut(id)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(session.object_hit_candidates(&ray()).unwrap()["total"], 1);
        let tool = block(&mut session, actor, 16.0, "tools/toolsnodraw");
        let tools = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(
            tools["candidates"][0]["object"],
            crate::identity::id_string(tool)
        );
        let mut view = session.view();
        view.tool_textures = false;
        session.set_view(view);
        let filtered = session.object_hit_candidates(&ray()).unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(
            filtered["candidates"][0]["object"],
            crate::identity::id_string(second)
        );
        assert_eq!(session.pick(&ray()).unwrap().object, second);
    }
}

#[test]
fn incompatible_modes_and_previews_reject_without_changing_selection_or_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets required".into()));
        let id = block(&mut session, actor, 32.0, "test");
        let before = session.document.vmf().clone();
        for ray in [
            Ray {
                direction: DVec3::ZERO,
                ..ray()
            },
            Ray {
                origin: DVec3::splat(f64::INFINITY),
                ..ray()
            },
        ] {
            assert!(session.cycle_object_hit(actor, &ray).is_err());
        }
        session.set_face_mode(true);
        assert!(session.object_hit_candidates(&ray()).is_err());
        session.set_face_mode(false);
        session.select(&[id], SelectMode::Replace).unwrap();
        session
            .preview_transform(
                actor,
                &[id],
                edit::Transform {
                    translation: DVec3::Y,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let preview = session.render_document().vmf().clone();
        assert!(session.cycle_object_hit(actor, &ray()).is_err());
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.selected_ids(), [id]);
        session.cancel_transform_preview();
        session.set_paint_tool(actor, true, 16.0).unwrap();
        assert!(session.cycle_object_hit(actor, &ray()).is_err());
        session.set_paint_tool(actor, false, 16.0).unwrap();
        session.clip_tool = true;
        assert!(session.cycle_object_hit(actor, &ray()).is_err());
        session.clip_tool = false;
        session.spatial.draw_block = true;
        assert!(session.cycle_object_hit(actor, &ray()).is_err());
        session.spatial.draw_block = false;
        assert_eq!(session.document.vmf(), &before);
    }
}

#[test]
fn missing_models_keep_point_helpers_and_usable_model_triangles_match_nearest_picking() {
    let mut fallback = Session::new(Err("no assets required".into()));
    let helper = fallback
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::new(32.0, 0.0, 0.0)),
            BTreeMap::from([("model".into(), "models/absent.mdl".into())]),
        )
        .unwrap();
    let report = fallback.object_hit_candidates(&ray()).unwrap();
    assert_eq!(report["total"], 1);
    assert_eq!(
        report["candidates"][0]["object"],
        crate::identity::id_string(helper)
    );
    assert_eq!(report["candidates"][0]["distance"], 24.0);
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    let id = session
        .create_entity(
            Actor::User,
            "prop_static",
            Some(DVec3::ZERO),
            BTreeMap::from([(
                "model".into(),
                "models/props_junk/wood_crate001a.mdl".into(),
            )]),
        )
        .unwrap();
    let models = session.model_instances(session.document());
    let instance = &models.instances[0];
    let model = &instance.asset.model;
    let points = model.parts[0].indices[..3]
        .iter()
        .map(|&index| instance.point(model.vertices[index as usize].position))
        .collect::<Vec<_>>();
    let center = (points[0] + points[1] + points[2]) / 3.0;
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize();
    let ray = Ray {
        origin: center - normal * 64.0,
        direction: normal,
    };
    let ordinary = session.pick(&ray).unwrap();
    assert_eq!(ordinary.object, id);
    let report = session.object_hit_candidates(&ray).unwrap();
    assert_eq!(report["total"], 1);
    assert_eq!(
        report["candidates"][0]["object"],
        crate::identity::id_string(id)
    );
    assert_eq!(report["candidates"][0]["distance"], ordinary.distance);
    assert_eq!(
        report["candidates"][0]["point"],
        json!(ordinary.point.to_array())
    );
}
