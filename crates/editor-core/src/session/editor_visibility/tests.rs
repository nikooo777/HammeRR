use super::*;
use crate::session::{
    Pointer, SelectMode, component_picking::Kind, paint, selection_filters::SelectionFilters,
    volume, working_region,
};
use glam::{DVec2, DVec3, Vec3};
use map_core::{
    edit, groups,
    query::Ray,
    region::{Match, Region},
    surface::FaceId,
};
use render::{Camera, Projection};

fn fixture() -> (Session, [ObjectId; 3]) {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for z in [64.0, 32.0] {
        ids.push(
            session
                .create_block(
                    Actor::User,
                    DVec3::new(-16.0, -16.0, z),
                    DVec3::new(16.0, 16.0, z + 16.0),
                    "test/checker",
                    false,
                )
                .unwrap(),
        );
    }
    let ObjectId::Solid(solid) = ids[1] else {
        unreachable!()
    };
    ids[1] = session
        .edit(Actor::User, "owner", |tx| {
            edit::tie_to_entity(tx, &[solid], "func_detail", &[])
        })
        .unwrap();
    ids.push(
        session
            .edit(Actor::User, "point", |tx| {
                edit::create_entity(tx, "info_target", Some(DVec3::ZERO), &[], Vec::new())
            })
            .unwrap(),
    );
    session.select(&[], SelectMode::Replace).unwrap();
    session.set_camera(Camera::planar(Projection::Top, Vec3::ZERO, 256.0));
    (session, ids.try_into().unwrap())
}

fn change(session: &mut Session, action: Action) -> Value {
    session
        .set_editor_visibility(Actor::Agent, action, &session.working_region_stamp())
        .unwrap()
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::Z * 160.0,
        direction: DVec3::NEG_Z,
    }
}

fn paint_spec(session: &Session) -> paint::Spec {
    paint::Spec {
        camera: session.camera(),
        pointer: Pointer {
            position: DVec2::new(320.0, 240.0),
            size: (640, 480),
        },
        radius: 2.0,
        mode: SelectMode::Replace,
        faces: false,
    }
}

#[test]
fn temporary_exclusions_share_render_hits_regions_paint_and_owner_reasons_without_source_edits() {
    for enabled in [false, true] {
        let (mut session, ids) = fixture();
        if enabled {
            session
                .set_working_region(
                    Actor::User,
                    working_region::Action::Set {
                        bounds: [DVec3::splat(-256.0), DVec3::splat(256.0)],
                        enabled: true,
                    },
                    &session.working_region_stamp(),
                )
                .unwrap();
        }
        session.select(&ids, SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let revision = session.document().revision();
        let dirty = session.document().is_dirty();
        assert_eq!(session.build_scene().stats.brushes, 2);
        assert_eq!(session.pick(&ray()).unwrap().object, ids[0]);
        change(&mut session, Action::Hide(vec![ids[0]]));
        assert_eq!(session.pick(&ray()).unwrap().object, ids[1]);
        assert_eq!(session.pick_for_selection(&ray()).unwrap().object, ids[1]);
        assert_eq!(session.object_hit_candidates(&ray()).unwrap()["total"], 2);
        assert_eq!(session.build_scene().stats.brushes, 1);
        let region = Region::from_bounds(DVec3::splat(-256.0), DVec3::splat(256.0)).unwrap();
        assert_eq!(
            session
                .select_region(&region, Match::Intersecting, false)
                .objects,
            ids[1..]
        );
        let token = session.begin_paint_selection(paint_spec(&session)).unwrap();
        assert_eq!(
            session
                .commit_paint_selection(Actor::Agent, token)
                .unwrap()
                .objects,
            [ids[1]]
        );
        change(&mut session, Action::Hide(vec![ids[1]]));
        assert_eq!(session.pick(&ray()).unwrap().object, ids[2]);
        let ObjectId::Entity(owner) = ids[1] else {
            unreachable!()
        };
        let child = session.document().entity(owner).unwrap().solids[0].id;
        assert!(session.hidden_tool_brushes().contains(&child));
        let report = session.get_working_region(0, 50).unwrap();
        let rows = report["objects"].as_array().unwrap();
        assert!(
            rows.iter()
                .find(|r| r["id"] == crate::identity::id_string(ids[1]))
                .unwrap()["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("editor_hidden"))
        );
        assert!(
            rows.iter()
                .find(|r| r["id"] == format!("solid:{child}"))
                .unwrap()["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("owner_editor_hidden"))
        );
        session.select(&[ids[0]], SelectMode::Replace).unwrap();
        assert_eq!(session.selected_ids(), [ids[0]]);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().revision(), revision);
        assert_eq!(session.document().is_dirty(), dirty);
        assert_eq!(session.history_snapshot(), history);
        change(&mut session, Action::ShowAll);
        assert_eq!(session.build_scene().stats.brushes, 2);
        assert_eq!(session.pick(&ray()).unwrap().object, ids[0]);
        assert_eq!(session.working_region_enabled(), enabled);
    }
}

#[test]
fn visibility_controls_do_not_expand_groups_and_disabled_type_peers_only_group_when_visible() {
    let (mut session, ids) = fixture();
    session
        .edit(Actor::User, "mixed group", |tx| {
            groups::apply(
                tx,
                &groups::Edit {
                    group: None,
                    name: "Mixed".into(),
                    parent: None,
                    members: vec![
                        groups::Member::Object(ids[0]),
                        groups::Member::Object(ids[2]),
                    ],
                    ungroup: false,
                },
            )
        })
        .unwrap();
    session
        .set_selection_filters(
            Actor::User,
            SelectionFilters {
                world_brushes: false,
                brush_entities: false,
                point_entities: true,
            },
        )
        .unwrap();
    assert_eq!(session.group_selection_units(&[ids[2]])[0].len(), 2);
    change(&mut session, Action::Hide(vec![ids[0]]));
    assert_eq!(session.group_selection_units(&[ids[2]]), vec![vec![ids[2]]]);
    assert_eq!(session.editor_hidden_count(), 1);
    let region = Region::from_bounds(DVec3::splat(-12.0), DVec3::splat(12.0)).unwrap();
    assert_eq!(
        session
            .select_region(&region, Match::Contained, false)
            .objects,
        [ids[2]]
    );
    session
        .set_selection_filters(
            Actor::User,
            SelectionFilters {
                world_brushes: false,
                brush_entities: false,
                point_entities: false,
            },
        )
        .unwrap();
    session.set_face_mode(true);
    session.select_faces(&[], SelectMode::Replace).unwrap();
    session
        .set_working_region(
            Actor::User,
            working_region::Action::Set {
                bounds: [DVec3::splat(1000.0), DVec3::splat(1100.0)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    change(&mut session, Action::ShowAll);
    assert_eq!(session.editor_hidden_count(), 0);
    assert!(session.face_mode());
    assert!(session.working_region_enabled());
    assert!(session.pick(&ray()).is_none());
    assert!(!session.selection_filters().world_brushes);
}

#[test]
fn rejected_and_noop_hides_preserve_drafts_and_show_all_keeps_owned_review() {
    let (mut session, ids) = fixture();
    change(&mut session, Action::Hide(vec![ids[0]]));
    let token = session
        .preview_volume_selection(volume::Spec {
            mins: DVec3::splat(-256.0),
            maxs: DVec3::splat(256.0),
            rule: Match::Intersecting,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap();
    let before = (
        session.scene_serial(),
        session.view_serial(),
        session.interaction_epoch(),
        session.working_region_stamp(),
    );
    change(&mut session, Action::Hide(vec![ids[0]]));
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.interaction_epoch(),
            session.working_region_stamp()
        ),
        before
    );
    assert_eq!(session.volume_preview().unwrap().token, token);
    let ObjectId::Entity(owner) = ids[1] else {
        unreachable!()
    };
    let child = session.document().entity(owner).unwrap().solids[0].id;
    for (action, stamp) in [
        (
            Action::Hide(vec![ids[2], ObjectId::Solid(child)]),
            session.working_region_stamp(),
        ),
        (
            Action::Hide(vec![ids[1], ObjectId::Entity(u32::MAX)]),
            session.working_region_stamp(),
        ),
        (Action::ShowAll, "stale".into()),
        (Action::Hide(Vec::new()), session.working_region_stamp()),
    ] {
        assert!(
            session
                .set_editor_visibility(Actor::Agent, action, &stamp)
                .is_err()
        );
        assert_eq!(session.volume_preview().unwrap().token, token);
        assert_eq!(session.editor_hidden_count(), 1);
    }
    let serial = session.working_region.serial;
    session.working_region.serial = u64::MAX;
    assert!(
        session
            .set_editor_visibility(
                Actor::Agent,
                Action::Hide(vec![ids[1]]),
                &session.working_region_stamp()
            )
            .is_err()
    );
    assert_eq!(session.volume_preview().unwrap().token, token);
    assert_eq!(session.editor_hidden_count(), 1);
    session.working_region.serial = serial;
    session.clear_volume_selection();
    session
        .preview_transform(
            Actor::Agent,
            &[ids[1]],
            edit::Transform {
                translation: DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let scene = session.scene_serial();
    assert!(
        session
            .set_editor_visibility(
                Actor::Agent,
                Action::Hide(vec![ids[1]]),
                &session.working_region_stamp()
            )
            .unwrap_err()
            .contains("Apply or cancel")
    );
    assert_eq!(session.scene_serial(), scene);
    assert_eq!(
        session.transform_preview().unwrap().transform.translation,
        DVec3::X * 32.0
    );
    change(&mut session, Action::ShowAll);
    assert_eq!(session.editor_hidden_count(), 0);
    assert_eq!(
        session.transform_preview().unwrap().transform.translation,
        DVec3::X * 32.0
    );
    session.cancel_transform_preview();
    let token = session
        .preview_volume_selection(volume::Spec {
            mins: DVec3::splat(-256.0),
            maxs: DVec3::splat(256.0),
            rule: Match::Intersecting,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap();
    let epoch = session.interaction_epoch();
    change(&mut session, Action::Hide(vec![ids[0]]));
    assert!(session.interaction_epoch() > epoch);
    assert!(session.commit_volume_selection(Actor::User, token).is_err());
    session.closing = true;
    assert!(
        session
            .set_editor_visibility(
                Actor::Agent,
                Action::ShowAll,
                &session.working_region_stamp()
            )
            .is_err()
    );
    assert_eq!(session.editor_hidden_count(), 1);
}

#[test]
fn hidden_ids_prune_against_committed_edits_undo_redo_and_reset_on_new_open() {
    let (mut session, ids) = fixture();
    let source = session.document().vmf().clone();
    change(&mut session, Action::Hide(vec![ids[0], ids[1]]));
    session.delete_objects(Actor::User, &[ids[0]]).unwrap();
    assert_eq!(session.editor_hidden_count(), 1);
    session.undo(Actor::User).unwrap();
    assert!(session.document().contains(ids[0]));
    assert!(!session.editor_object_hidden(ids[0]));
    assert!(session.editor_object_hidden(ids[1]));
    session.redo(Actor::User).unwrap();
    assert_eq!(session.editor_hidden_count(), 1);
    assert_eq!(Session::new(Err("peer".into())).editor_hidden_count(), 0);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("map.vmf");
    std::fs::write(&path, source.write(&Default::default())).unwrap();
    session.open(Actor::User, &path, true).unwrap();
    assert_eq!(session.editor_hidden_count(), 0);
    change(&mut session, Action::Hide(vec![ids[1]]));
    session.new_map(Actor::User, true).unwrap();
    assert_eq!(session.editor_hidden_count(), 0);
}

#[test]
fn components_terrain_and_surface_hits_obey_temporary_exclusions_with_region_off() {
    let (mut session, ids) = fixture();
    let ObjectId::Solid(solid) = ids[0] else {
        unreachable!()
    };
    let face = FaceId {
        solid,
        side: session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    };
    let pointer = Pointer {
        position: DVec2::splat(256.0),
        size: (512, 512),
    };
    let camera = Camera::planar(Projection::Top, Vec3::new(0.0, 0.0, 80.0), 128.0);
    assert!(
        !session
            .pick_components(camera, pointer, Kind::Face, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    session
        .edit(Actor::User, "terrain", |tx| {
            map_core::displacement::edit::create(tx, &[face], 2, 0)
        })
        .unwrap();
    assert!(
        session
            .pick_terrain_triangle(&ray(), &[face])
            .unwrap()
            .is_some()
    );
    change(&mut session, Action::Hide(vec![ids[0]]));
    assert!(
        session
            .pick_components(camera, pointer, Kind::Face, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(
        session
            .pick_terrain_triangle(&ray(), &[face])
            .unwrap()
            .is_none()
    );
    assert_ne!(
        session
            .placement_surface(&[], ray(), false)
            .unwrap()
            .hit
            .object,
        ids[0]
    );
    let token = session
        .preview_volume_selection(volume::Spec {
            mins: DVec3::splat(-256.0),
            maxs: DVec3::splat(256.0),
            rule: Match::Intersecting,
            mode: SelectMode::Replace,
            faces: true,
        })
        .unwrap();
    assert!(
        session
            .terrain_vertices_in_volume(&[face], token)
            .unwrap()
            .is_empty()
    );
    assert!(
        session
            .terrain_triangles_in_volume(&[face], token)
            .unwrap()
            .is_empty()
    );
    change(&mut session, Action::ShowAll);
    assert!(
        session
            .pick_terrain_triangle(&ray(), &[face])
            .unwrap()
            .is_some()
    );
}

#[test]
fn decoded_model_meshes_keep_explicit_inspection_while_temporarily_hidden() {
    let (_directory, game) = crate::test_assets::model_lighting::game();
    let mut session = Session::new(Ok(game));
    let model = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::ZERO),
            [("model".into(), "models/test/receiver.mdl".into())]
                .into_iter()
                .collect(),
        )
        .unwrap();
    let foreground = session
        .create_block(
            Actor::User,
            DVec3::new(-16.0, -16.0, 32.0),
            DVec3::new(16.0, 16.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    assert_eq!(session.pick(&ray()).unwrap().object, foreground);
    change(&mut session, Action::Hide(vec![foreground]));
    assert_eq!(session.pick(&ray()).unwrap().object, model);
    assert_eq!(session.build_scene().stats.model_entities, 1);
    change(&mut session, Action::Hide(vec![model]));
    assert!(session.pick(&ray()).is_none());
    assert_eq!(session.build_scene().stats.model_entities, 0);
    let ObjectId::Entity(id) = model else {
        unreachable!()
    };
    let report = session.inspect_entity_model(id).unwrap();
    assert_eq!(report["editor_hidden"], true);
    assert_eq!(report["displayed"], false);
    assert!(report["asset"]["vertices"].as_u64().unwrap() > 0);
    change(&mut session, Action::ShowAll);
    assert_eq!(session.build_scene().stats.model_entities, 1);
}

#[test]
fn show_all_clears_only_editor_exclusions_and_preserves_saved_hide_and_tool_filter() {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsnodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let visible = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(16.0),
            "test/checker",
            false,
        )
        .unwrap();
    let authored = session
        .create_block(
            Actor::User,
            DVec3::splat(32.0),
            DVec3::splat(48.0),
            "test/checker",
            false,
        )
        .unwrap();
    let tool = session
        .create_block(
            Actor::User,
            DVec3::splat(64.0),
            DVec3::splat(80.0),
            "tools/toolsnodraw",
            false,
        )
        .unwrap();
    let ObjectId::Solid(hidden) = authored else {
        unreachable!()
    };
    session
        .edit(Actor::User, "saved hide", |tx| {
            tx.solid_mut(hidden)?.hidden = true;
            Ok(())
        })
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: false,
        ..session.view()
    });
    let baseline = session.build_scene();
    let ObjectId::Solid(tool_solid) = tool else {
        unreachable!()
    };
    assert!(session.hidden_tool_brushes().contains(&tool_solid));
    let tool_ray = Ray {
        origin: DVec3::new(72.0, 72.0, 128.0),
        direction: DVec3::NEG_Z,
    };
    assert!(session.pick(&tool_ray).is_none());
    let source = session.document().vmf().clone();
    let scope = session.compile_region_build_scope();
    change(&mut session, Action::Hide(vec![visible, authored, tool]));
    assert_eq!(session.build_scene().stats.brushes, 0);
    change(&mut session, Action::ShowAll);
    let restored = session.build_scene();
    assert_eq!(restored.stats, baseline.stats);
    assert_eq!(restored.vertices.len(), baseline.vertices.len());
    assert_eq!(restored.indices, baseline.indices);
    assert!(session.hidden_tool_brushes().contains(&tool_solid));
    assert!(session.pick(&tool_ray).is_none());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.compile_region_build_scope(), scope);
    assert!(session.document().solid(hidden).unwrap().hidden);
    assert!(!session.view().tool_textures);
}
