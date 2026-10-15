use super::*;
use crate::session::{SelectMode, overlay};
use map_core::{BrushGeometry, edit, query::Ray, surface::FaceId};

fn set(session: &mut Session, actor: Actor, bounds: [DVec3; 2]) -> Value {
    session
        .set_working_region(
            actor,
            Action::Set {
                bounds,
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap()
}

fn block(session: &mut Session, mins: DVec3, maxs: DVec3) -> ObjectId {
    session
        .create_block(Actor::User, mins, maxs, "test/checker", false)
        .unwrap()
}

#[test]
fn whole_owner_visibility_uses_visible_children_and_preserves_authored_context_and_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let inside = block(
            &mut session,
            DVec3::new(-4.125, -4.375, -4.625),
            DVec3::new(4.875, 4.625, 4.375),
        );
        let outside = block(&mut session, DVec3::splat(100.125), DVec3::splat(120.125));
        let hidden_inside = block(&mut session, DVec3::splat(-2.0), DVec3::splat(2.0));
        let ObjectId::Solid(outside_solid) = outside else {
            unreachable!()
        };
        let ObjectId::Solid(hidden_solid) = hidden_inside else {
            unreachable!()
        };
        let owner = session
            .edit(actor, "detail owner", |tx| {
                edit::tie_to_entity(tx, &[outside_solid, hidden_solid], "func_detail", &[])?;
                let owner = tx.document().vmf().entities.last().unwrap().id;
                tx.entity_mut(owner)?
                    .solids
                    .iter_mut()
                    .find(|solid| solid.id == hidden_solid)
                    .unwrap()
                    .hidden = true;
                Ok(ObjectId::Entity(owner))
            })
            .unwrap();
        session
            .select(&[inside, owner], SelectMode::Replace)
            .unwrap();
        let original = session.document.vmf().clone();
        let history = session
            .document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let camera = session.camera();
        let original_revision = session.document.revision();
        let report = set(&mut session, actor, [DVec3::splat(-8.0), DVec3::splat(8.0)]);
        assert_eq!(report["selected_excluded"], 1);
        let evaluation = session.working_evaluation(session.render_document());
        assert!(evaluation.objects.contains(&inside));
        assert!(!evaluation.objects.contains(&owner));
        assert!(!evaluation.solids.contains(&outside_solid));
        assert!(!evaluation.solids.contains(&hidden_solid));
        let ray = Ray {
            origin: DVec3::new(110.0, 110.0, 200.0),
            direction: -DVec3::Z,
        };
        assert!(session.pick(&ray).is_none());
        let mesh = session.build_scene();
        assert_eq!(mesh.stats.brushes, 1);
        assert!(mesh.vertices.iter().all(|vertex| vertex.position[0] < 10.0));
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.revision(), original_revision);
        assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
        assert_eq!(session.selected_ids(), [inside, owner]);
        assert_eq!(session.camera(), camera);
        for bad in [
            [DVec3::ZERO, DVec3::ZERO],
            [DVec3::splat(f64::NAN), DVec3::ONE],
            [DVec3::ZERO, DVec3::splat(1e30)],
        ] {
            let snapshot = session.working_region_stamp();
            assert!(
                session
                    .set_working_region(
                        actor,
                        Action::Set {
                            bounds: bad,
                            enabled: true
                        },
                        &snapshot
                    )
                    .is_err()
            );
            assert_eq!(session.working_region_stamp(), snapshot);
        }
        set(
            &mut session,
            actor,
            [DVec3::new(4.875, 4.625, 4.375), DVec3::splat(8.0)],
        );
        assert!(
            session.working_object_visible(&session.document, inside),
            "a touching corner retains the complete unit"
        );
        session
            .set_working_region(actor, Action::Clear, &session.working_region_stamp())
            .unwrap();
        assert!(session.pick(&ray).is_some());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.build_scene().stats.brushes, 2);
    }
}

#[test]
fn region_changes_keep_owned_overlay_and_transform_previews_and_apply_exactly_once() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let id = block(&mut session, DVec3::ZERO, DVec3::splat(64.0));
        let ObjectId::Solid(solid) = id else {
            unreachable!()
        };
        let brush = session.document.solid(solid).unwrap();
        let geometry = BrushGeometry::from_solid(brush).unwrap();
        let side = brush.sides[geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap()
            .side]
            .id;
        let source = FaceId { solid, side };
        session
            .select_faces(&[source], SelectMode::Replace)
            .unwrap();
        let token = session
            .preview_overlay(
                actor,
                overlay::Request {
                    source,
                    options: map_core::overlay::Options {
                        material: "test/checker".into(),
                        ..Default::default()
                    },
                },
                &session.overlay_stamp(),
            )
            .unwrap()["draft"]["token"]
            .as_u64()
            .unwrap();
        let draft = session.get_overlay_state()["draft"].clone();
        let before = session.document.vmf().clone();
        set(
            &mut session,
            actor,
            [DVec3::splat(100.0), DVec3::splat(200.0)],
        );
        assert_eq!(session.get_overlay_state()["draft"], draft);
        let mut mesh = session.build_scene();
        let lines = mesh.lines.len();
        session.append_overlay_guide(&mut mesh);
        assert_eq!(mesh.lines.len(), lines);
        assert!(mesh.vertices.is_empty());
        session.apply_overlay(actor, token).unwrap();
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.select(&[id], SelectMode::Replace).unwrap();
        session
            .preview_transform(
                actor,
                &[id],
                edit::Transform {
                    translation: DVec3::splat(128.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let transformed = session.render_document().vmf().clone();
        assert_eq!(session.build_scene().stats.brushes, 1);
        set(
            &mut session,
            actor,
            [DVec3::splat(-8.0), DVec3::splat(72.0)],
        );
        assert!(session.transform_preview().is_some());
        assert_eq!(session.render_document().vmf(), &transformed);
        assert_eq!(session.build_scene().stats.brushes, 0);
        assert!(session.transform_guide().is_none());
        session.cancel_transform_preview();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.build_scene().stats.brushes, 1);
    }
}

#[test]
fn region_membership_is_shared_by_points_groups_volume_cycle_and_paint_and_resets_on_new_map() {
    use map_core::{
        picking::Picker,
        region::{Match, Region},
    };
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let inside = block(&mut session, DVec3::splat(-8.0), DVec3::splat(8.0));
    let outside = block(&mut session, DVec3::splat(100.0), DVec3::splat(120.0));
    let helper = session
        .edit(Actor::User, "outside helper", |tx| {
            edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::new(110.0, 110.0, 180.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    session
        .edit(Actor::User, "mixed inside/outside group", |tx| {
            map_core::groups::apply(
                tx,
                &map_core::groups::Edit {
                    group: None,
                    name: "region group".into(),
                    parent: None,
                    members: vec![
                        map_core::groups::Member::Object(inside),
                        map_core::groups::Member::Object(outside),
                    ],
                    ungroup: false,
                },
            )
        })
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    set(
        &mut session,
        Actor::Agent,
        [DVec3::splat(-16.0), DVec3::splat(16.0)],
    );
    let ray = Ray {
        origin: DVec3::new(110.0, 110.0, 250.0),
        direction: -DVec3::Z,
    };
    assert!(session.pick(&ray).is_none());
    assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 0);
    let region = Region::from_bounds(DVec3::splat(-500.0), DVec3::splat(500.0)).unwrap();
    let matches = session.select_region(&region, Match::Intersecting, false);
    assert_eq!(matches.objects, [inside]);
    assert_eq!(session.group_selection_units(&[inside]), vec![vec![inside]]);
    let evaluation = session.working_evaluation(&session.document);
    assert!(!evaluation.objects.contains(&outside));
    assert!(!evaluation.objects.contains(&helper));
    let picker = Picker::with_entity_filter(
        &session.document,
        |solid| evaluation.solids.contains(&solid.id),
        true,
        |entity| evaluation.objects.contains(&ObjectId::Entity(entity.id)),
    );
    assert!(picker.pick(&ray).is_none());
    let page = session.get_working_region(1, 1).unwrap();
    assert_eq!(page["objects"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 3);
    let old = session.working_region_stamp();
    session.select(&[inside], SelectMode::Replace).unwrap();
    assert!(
        session
            .set_working_region(Actor::Agent, Action::Clear, &old)
            .is_err()
    );
    session.new_map(Actor::Agent, true).unwrap();
    assert!(!session.working_region_enabled());
}

#[test]
fn transformed_model_bounds_and_fallback_are_reported_and_asset_reload_recomputes_membership() {
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    let model = session
        .edit(Actor::User, "rotated model", |tx| {
            edit::create_entity(
                tx,
                "prop_dynamic",
                Some(DVec3::new(400.125, -200.375, 24.625)),
                &[
                    (
                        "model".into(),
                        "models/props_junk/wood_crate001a.mdl".into(),
                    ),
                    ("angles".into(), "17.125 -23.375 8.625".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let missing = session
        .edit(Actor::User, "missing model", |tx| {
            edit::create_entity(
                tx,
                "prop_dynamic",
                Some(DVec3::splat(1000.0)),
                &[("model".into(), "models/test/absent.mdl".into())],
                Vec::new(),
            )
        })
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    let model_bounds = session.model_bounds_in(&session.document)[&model];
    let bounds = [model_bounds.0 - DVec3::ONE, model_bounds.0 + DVec3::ONE];
    let report = set(&mut session, Actor::Agent, bounds);
    let rows = report["objects"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row["id"] == crate::identity::id_string(model))
            .unwrap()["bounds_source"],
        "model"
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["id"] == crate::identity::id_string(missing))
            .unwrap()["bounds_source"],
        "model_helper_fallback"
    );
    assert_eq!(session.build_scene().stats.model_entities, 1);
    let before = session.working_evaluation(&session.document);
    let source = session.document.vmf().clone();
    session.refresh_model_assets(Actor::User);
    let after = session.working_evaluation(&session.document);
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(after.objects.contains(&model));
    assert!(!after.objects.contains(&missing));
    assert_eq!(session.document.vmf(), &source);
}

#[test]
fn unknown_visible_child_keeps_whole_owner_included_while_hidden_unknown_child_does_not() {
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(a) = block(&mut session, DVec3::splat(100.0), DVec3::splat(110.0)) else {
        unreachable!()
    };
    let ObjectId::Solid(b) = block(&mut session, DVec3::splat(200.0), DVec3::splat(210.0)) else {
        unreachable!()
    };
    let ObjectId::Entity(owner) = session
        .edit(Actor::User, "owner with unresolved visible child", |tx| {
            let id = edit::tie_to_entity(tx, &[a, b], "func_detail", &[])?;
            let ObjectId::Entity(owner) = id else {
                unreachable!()
            };
            tx.entity_mut(owner)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == b)
                .unwrap()
                .sides
                .clear();
            Ok(id)
        })
        .unwrap()
    else {
        unreachable!()
    };
    let report = set(
        &mut session,
        Actor::Agent,
        [DVec3::splat(-8.0), DVec3::splat(8.0)],
    );
    let row = report["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == format!("entity:{owner}"))
        .unwrap();
    assert_eq!(row["included"], true);
    assert_eq!(row["bounds"], Value::Null);
    assert!(
        row["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("unresolved_bounds"))
    );
    session
        .edit(Actor::User, "hide unresolved child", |tx| {
            tx.entity_mut(owner)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == b)
                .unwrap()
                .hidden = true;
            Ok(())
        })
        .unwrap();
    assert!(!session.working_object_visible(&session.document, ObjectId::Entity(owner)));
    session
        .edit(Actor::User, "hide all owner children", |tx| {
            for solid in &mut tx.entity_mut(owner)?.solids {
                solid.hidden = true;
            }
            Ok(())
        })
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: false,
        ..session.view()
    });
    let report = session.get_working_region(0, 50).unwrap();
    let row = report["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == format!("entity:{owner}"))
        .unwrap();
    assert_eq!(row["included"], false);
    assert_eq!(row["reasons"], json!(["authored_hidden"]));
}

#[test]
fn indexed_paint_uses_committed_membership_after_it_discards_a_geometry_preview() {
    use crate::session::{Pointer, paint::Spec};
    use render::{Camera, Projection};
    for committed_inside in [false, true] {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let x = if committed_inside { 0.0 } else { 100.0 };
        let id = block(
            &mut session,
            DVec3::new(x, -8.0, 0.0),
            DVec3::new(x + 16.0, 8.0, 16.0),
        );
        set(
            &mut session,
            Actor::Agent,
            [DVec3::splat(-32.0), DVec3::splat(32.0)],
        );
        session
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    translation: DVec3::X * if committed_inside { 100.0 } else { -100.0 },
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let camera = Camera::planar(
            Projection::Top,
            glam::Vec3::new(x as f32 + 8.0, 0.0, 8.0),
            128.0,
        );
        let token = session
            .begin_paint_selection(Spec {
                camera,
                pointer: Pointer {
                    position: glam::DVec2::splat(128.0),
                    size: (256, 256),
                },
                radius: 8.0,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        assert!(session.transform_preview().is_none());
        let selected = session.commit_paint_selection(Actor::Agent, token).unwrap();
        assert_eq!(selected.objects.contains(&id), committed_inside);
    }
}

#[test]
fn component_faces_corners_and_terrain_spatial_subsets_follow_the_same_mask() {
    use crate::session::{Pointer, component_picking::Kind, volume};
    use render::{Camera, Projection};
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = block(&mut session, DVec3::ZERO, DVec3::splat(64.0)) else {
        unreachable!()
    };
    let face = FaceId {
        solid,
        side: session
            .document
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    };
    let pointer = Pointer {
        position: glam::DVec2::splat(256.0),
        size: (512, 512),
    };
    let top = Camera::planar(Projection::Top, glam::Vec3::new(32.0, 32.0, 64.0), 128.0);
    let corner = Camera::planar(Projection::Top, glam::Vec3::new(0.0, 0.0, 64.0), 128.0);
    assert!(
        !session
            .pick_components(top, pointer, Kind::Face, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(
        !session
            .pick_components(corner, pointer, Kind::Vertex, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    set(
        &mut session,
        Actor::Agent,
        [DVec3::splat(100.0), DVec3::splat(200.0)],
    );
    assert!(
        session
            .pick_components(top, pointer, Kind::Face, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(
        session
            .pick_components(corner, pointer, Kind::Vertex, &[solid], 8.0)
            .unwrap()
            .candidates
            .is_empty()
    );
    session
        .edit(Actor::User, "terrain", |tx| {
            map_core::displacement::edit::create(tx, &[face], 2, 0)
        })
        .unwrap();
    let ray = Ray {
        origin: DVec3::new(32.0, 32.0, 200.0),
        direction: -DVec3::Z,
    };
    assert!(
        session
            .pick_terrain_triangle(&ray, &[face])
            .unwrap()
            .is_none()
    );
    let spec = volume::Spec {
        mins: DVec3::splat(-100.0),
        maxs: DVec3::splat(300.0),
        rule: map_core::region::Match::Intersecting,
        mode: SelectMode::Replace,
        faces: true,
    };
    let token = session.preview_volume_selection(spec).unwrap();
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
    session
        .set_working_region(Actor::User, Action::Clear, &session.working_region_stamp())
        .unwrap();
    assert!(session.commit_volume_selection(Actor::User, token).is_err());
    assert!(
        session
            .pick_terrain_triangle(&ray, &[face])
            .unwrap()
            .is_some()
    );
    let token = session.preview_volume_selection(spec).unwrap();
    assert!(
        !session
            .terrain_vertices_in_volume(&[face], token)
            .unwrap()
            .is_empty()
    );
    assert!(
        !session
            .terrain_triangles_in_volume(&[face], token)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn planned_partition_materials_cannot_reintroduce_an_excluded_owner() {
    use crate::session::{
        Pointer,
        component_picking::Kind,
        components::{CreaseEdges, PartitionRef},
    };
    use map_core::subobject::{self, Component};
    use render::{Camera, Projection};
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsnodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "tools/toolsnodraw",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let brush = session.document.solid(solid).unwrap();
    let side = brush
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let edges = subobject::crease_pairs(brush, side).unwrap()[0].edges;
    session
        .set_component_picking(Some(Kind::Face), &[solid], &session.component_stamp())
        .unwrap();
    session
        .select_components(
            &[Component {
                solid,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let token = session
        .preview_component_partition(&[CreaseEdges { solid, edges }], &session.component_stamp())
        .unwrap();
    let token = session
        .update_component_partition_material(
            token,
            &[
                PartitionRef {
                    solid,
                    partition: 0,
                },
                PartitionRef {
                    solid,
                    partition: 1,
                },
            ],
            Some("test/checker"),
        )
        .unwrap();
    session.set_view(render::ViewOptions {
        tool_textures: false,
        ..session.view()
    });
    let camera = Camera::planar(Projection::Top, glam::Vec3::new(16.0, 16.0, 64.0), 128.0);
    let pointer = Pointer {
        position: glam::DVec2::splat(256.0),
        size: (512, 512),
    };
    assert!(
        !session
            .pick_component_partitions(token, camera, pointer)
            .unwrap()
            .is_empty()
    );
    let source = session.document.vmf().clone();
    set(
        &mut session,
        Actor::Agent,
        [DVec3::splat(100.0), DVec3::splat(200.0)],
    );
    assert_eq!(session.partition_preview_token(), Some(token));
    assert!(
        session
            .pick_component_partitions(token, camera, pointer)
            .unwrap()
            .is_empty()
    );
    let mut mesh = session.build_scene();
    mesh.lines.clear();
    session.append_component_guides(&mut mesh);
    assert!(mesh.lines.is_empty());
    assert_eq!(session.document.vmf(), &source);
}
