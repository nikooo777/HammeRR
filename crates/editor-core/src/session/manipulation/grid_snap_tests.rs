use super::*;
use map_core::groups::Member;
use std::collections::BTreeMap;

fn block(session: &mut Session, minimum: DVec3) -> ObjectId {
    session
        .create_block(
            Actor::User,
            minimum,
            minimum + DVec3::splat(16.0),
            "brick/test",
            false,
        )
        .unwrap()
}

#[test]
fn independent_grid_snap_moves_group_members_and_whole_brush_owners_in_one_undo() {
    for (actor, texture_lock) in [(Actor::User, true), (Actor::Agent, false)] {
        let mut session = Session::new(Err("no assets needed".into()));
        let world = block(&mut session, DVec3::new(-3.125, 5.25, 7.5));
        let brush = block(&mut session, DVec3::new(34.25, -19.5, 9.75));
        let hidden = block(&mut session, DVec3::new(70.5, -31.75, 25.5));
        let ObjectId::Solid(hidden_id) = hidden else {
            panic!()
        };
        session
            .edit(actor, "hide child", |tx| {
                tx.solid_mut(hidden_id)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let owner = session
            .tie_to_entity(
                actor,
                &[brush, hidden],
                "func_detail",
                BTreeMap::from([
                    ("origin".into(), "12.5 18.25 30.5".into()),
                    ("angles".into(), "0 15 0".into()),
                ]),
            )
            .unwrap();
        let point = session
            .create_entity(
                actor,
                "info_target",
                Some(DVec3::new(113.25, 82.75, 65.5)),
                BTreeMap::from([("angles".into(), "0 25 0".into())]),
            )
            .unwrap();
        let token = session.begin_group_edit(None, false).unwrap()["token"]
            .as_u64()
            .unwrap();
        session
            .update_group_edit(
                token,
                Some("Snapped members".into()),
                None,
                Some(vec![
                    Member::Object(world),
                    Member::Object(owner),
                    Member::Object(point),
                ]),
            )
            .unwrap();
        session.apply_group_edit(actor, token).unwrap();
        session
            .select_grouped(&[world], SelectMode::Replace)
            .unwrap();
        let ids = session.selected_ids();
        assert_eq!(ids.len(), 3);
        session
            .set_spatial(
                actor,
                crate::spatial::Update {
                    snap: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        session
            .set_transform_tool(
                actor,
                TransformTool {
                    texture_lock,
                    texture_scale_lock: Some(!texture_lock),
                    ..Default::default()
                },
            )
            .unwrap();
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let camera = session.camera();
        let report = session
            .preview_snap_to_grid_mode(actor, &ids, GridSnapMode::EachObject)
            .unwrap();
        let GridSnapPreview::EachObject { objects, .. } = &report else {
            panic!()
        };
        let deltas = objects
            .iter()
            .map(|object| (object.id, DVec3::from_array(object.snap.delta)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(deltas[&world], DVec3::new(3.125, -5.25, -7.5));
        assert_eq!(deltas[&owner], DVec3::new(-2.25, -0.25, 6.25));
        assert_eq!(deltas[&point], DVec3::new(6.75, 5.25, 6.5));
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.selected_ids(), ids);
        assert_eq!(session.camera(), camera);
        let preview = session.render_document().vmf().clone();
        for (old, new) in source.solids().zip(preview.solids()) {
            let root = session.document.owner_of_solid(old.id).unwrap();
            for (old, new) in old.sides.iter().zip(&new.sides) {
                for (old, new) in old.plane.0.iter().zip(new.plane.0) {
                    assert!((new - *old).abs_diff_eq(deltas[&root], 1e-8));
                }
                if texture_lock {
                    assert!(
                        map_core::texel_uv(old, old.plane.0[0])
                            .abs_diff_eq(map_core::texel_uv(new, new.plane.0[0]), 1e-8)
                    );
                } else {
                    assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                }
            }
        }
        for (old, new) in source.entities.iter().zip(&preview.entities) {
            assert_eq!(old.properties.get("angles"), new.properties.get("angles"));
            assert!(
                (new.origin().unwrap() - old.origin().unwrap())
                    .abs_diff_eq(deltas[&ObjectId::Entity(old.id)], 1e-8)
            );
        }
        session.cancel_transform_preview();
        assert!(session.commit_transform_preview(actor).is_err());
        assert_eq!(session.render_document().vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        let duplicated = ids
            .iter()
            .rev()
            .copied()
            .chain(ids.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(
            session
                .preview_snap_to_grid_mode(actor, &duplicated, GridSnapMode::EachObject)
                .unwrap(),
            report
        );
        session.commit_transform_preview(actor).unwrap();
        assert_eq!(session.document.vmf(), &preview);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        let aligned = session
            .preview_snap_to_grid_mode(actor, &ids, GridSnapMode::EachObject)
            .unwrap();
        let GridSnapPreview::EachObject { objects, .. } = aligned else {
            panic!()
        };
        assert!(objects.iter().all(|object| object.snap.delta == [0.0; 3]));
        session.commit_transform_preview(actor).unwrap();
        assert_eq!(session.document.vmf(), &preview);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.camera(), camera);
    }
}

#[test]
fn independent_grid_snap_late_transform_failure_preserves_the_previous_draft() {
    let mut session = Session::new(Err("no assets needed".into()));
    let first = block(&mut session, DVec3::new(-3.125, 5.25, 7.5));
    let last = block(&mut session, DVec3::new(-35.125, 37.25, 71.5));
    let ObjectId::Solid(last_id) = last else {
        panic!()
    };
    session
        .edit(Actor::User, "invalid locked UV", |tx| {
            let axis = &mut tx.solid_mut(last_id)?.sides[0].u_axis;
            axis.axis = DVec3::NEG_X;
            axis.offset = f64::MAX;
            axis.scale = f64::MIN_POSITIVE;
            Ok(())
        })
        .unwrap();
    session
        .preview_snap_to_grid_mode(Actor::User, &[first], GridSnapMode::EachObject)
        .unwrap();
    let source = session.document.vmf().clone();
    let displayed = session.render_document().vmf().clone();
    let report = session.transform_preview().unwrap().grid_snap.clone();
    let selection = session.selected_ids();
    let history = session.history_snapshot();
    let camera = session.camera();
    let serials = (
        session.scene_serial,
        session.view_serial,
        session.interaction_epoch(),
    );
    let error = session
        .preview_snap_to_grid_mode(Actor::Agent, &[first, last], GridSnapMode::EachObject)
        .unwrap_err();
    assert!(error.contains("texture offset"), "{error}");
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &displayed);
    assert_eq!(session.transform_preview().unwrap().grid_snap, report);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(
        (
            session.scene_serial,
            session.view_serial,
            session.interaction_epoch()
        ),
        serials
    );
    session.commit_transform_preview(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &displayed);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
}

#[test]
fn independent_grid_snap_rejects_owned_children_stale_sources_and_invalid_hidden_children() {
    let mut session = Session::new(Err("no assets needed".into()));
    let child = block(&mut session, DVec3::new(-3.125, 5.25, 7.5));
    let hidden = block(&mut session, DVec3::new(34.25, -19.5, 9.75));
    let owner = session
        .tie_to_entity(
            Actor::User,
            &[child, hidden],
            "func_detail",
            BTreeMap::new(),
        )
        .unwrap();
    let ObjectId::Entity(owner_id) = owner else {
        panic!()
    };
    let ObjectId::Solid(hidden_id) = hidden else {
        panic!()
    };
    session
        .document
        .edit("invalid hidden child", |tx| {
            let solid = tx
                .entity_mut(owner_id)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == hidden_id)
                .unwrap();
            solid.hidden = true;
            solid.sides[0].plane.0 = [DVec3::ZERO; 3];
            Ok(())
        })
        .unwrap();
    let good = block(&mut session, DVec3::new(101.0, 5.0, 7.0));
    let source_corner = session
        .surface_placement_source_corners(&[good])
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .component;
    let pending = session
        .begin_surface_placement_pick(
            &[good],
            super::super::placement::Options {
                align: false,
                offset: 0.0,
                texture_lock: true,
                source_corner: Some(source_corner),
            },
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .preview_snap_to_grid_mode(Actor::Agent, &[owner, child], GridSnapMode::EachObject)
            .unwrap_err()
            .contains("whole brush entities")
    );
    assert!(
        session
            .preview_snap_to_grid_mode(Actor::Agent, &[good, owner], GridSnapMode::EachObject)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &source);
    assert_eq!(session.surface_placement(), pending);
    assert_eq!(session.history_snapshot(), history);
    session
        .preview_snap_to_grid_mode(Actor::User, &[good], GridSnapMode::EachObject)
        .unwrap();
    session
        .document
        .edit("source changed", |tx| {
            map_core::edit::translate(tx, &[good], DVec3::X, true)
        })
        .unwrap();
    let changed_source = session.document.vmf().clone();
    let changed_history = session.history_snapshot();
    assert!(
        session
            .commit_transform_preview(Actor::Agent)
            .unwrap_err()
            .contains("Map changed")
    );
    assert_eq!(session.document.vmf(), &changed_source);
    assert_eq!(session.history_snapshot(), changed_history);
    session
        .preview_snap_to_grid_mode(Actor::User, &[good], GridSnapMode::EachObject)
        .unwrap();
    session.document_generation += 1;
    assert!(session.commit_transform_preview(Actor::User).is_err());
    assert_eq!(session.document.vmf(), &changed_source);
}
