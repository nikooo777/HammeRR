use super::*;
use crate::session::{Pointer, components::partitions::tests::fixture};
use render::{Camera, Projection};

fn piece(solid: u32, partition: u32) -> PartitionRef {
    PartitionRef { solid, partition }
}

#[test]
fn boundary_cut_authoring_shares_materials_storage_extrusion_and_history() {
    for actor in [Actor::User, Actor::Agent] {
        for owned in [false, true] {
            for direction in 0..4 {
                let (mut session, solid, _) = fixture(actor, usize::from(owned));
                let choices = session.component_partition_choices().unwrap();
                assert_eq!(choices[0].1.len(), 6);
                assert_eq!(
                    session.component_crease_choices().unwrap()[0].1,
                    choices[0].1[..2]
                );
                let side = choices[0].0.sides[0];
                let pairs = [CreaseEdges {
                    solid,
                    edges: choices[0].1[direction + 2].edges,
                }];
                let before = session.document.vmf().clone();
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                let token = session
                    .update_component_partition_cut(token, [0.2, 0.8])
                    .unwrap();
                let token = session
                    .update_component_partition_material(
                        token,
                        &[piece(solid, 0)],
                        Some("brick/triangle"),
                    )
                    .unwrap();
                let token = session
                    .update_component_partition_material(
                        token,
                        &[piece(solid, 1)],
                        Some("wood/pentagon"),
                    )
                    .unwrap();
                let report = session.component_report(&[solid]).unwrap();
                assert!(
                    session
                        .raise_component_partition(token, 16.0)
                        .unwrap_err()
                        .contains("two quad")
                );
                assert_eq!(session.component_report(&[solid]).unwrap(), report);
                assert_eq!(session.document.vmf(), &before);
                session.store_component_partition(actor, token).unwrap();
                let stored = session.document.vmf().clone();
                let history = session.document.undo_labels().count();
                let token = session
                    .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                    .unwrap();
                let draft = session.checked_partition(token).unwrap();
                assert_eq!(
                    draft.partitions[0]
                        .loops
                        .each_ref()
                        .map(|piece| piece.vertices.len()),
                    [3, 5]
                );
                let spec = subobject::ExtrusionSpec {
                    height: 16.125,
                    wall_material: None,
                    texture_lock: true,
                };
                let token = session
                    .preview_partition_extrusion(token, spec.clone())
                    .unwrap();
                let applied = session.render_document().vmf().clone();
                let plan = session
                    .checked_component_preview(token)
                    .unwrap()
                    .extrusion
                    .as_ref()
                    .unwrap()
                    .plan
                    .clone();
                assert_eq!(
                    plan.pieces
                        .iter()
                        .map(|piece| piece.solid.sides.len())
                        .collect::<Vec<_>>(),
                    vec![5, 7]
                );
                assert_eq!(session.document.undo_labels().count(), history);
                session.cancel_component_move(token).unwrap();
                assert_eq!(session.document.vmf(), &stored);
                let token = session
                    .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                    .unwrap();
                let token = session.preview_partition_extrusion(token, spec).unwrap();
                assert_eq!(session.render_document().vmf(), &applied);
                session.commit_component_edit(actor, token).unwrap();
                assert_eq!(session.document.vmf(), &applied);
                assert_eq!(session.document.undo_labels().count(), history + 1);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &stored);
                session.redo(actor).unwrap();
                assert_eq!(session.document.vmf(), &applied);
            }
        }
    }
}

#[test]
fn adjusted_planar_ridge_drafts_share_rejection_history_and_material_replay() {
    for actor in [Actor::User, Actor::Agent] {
        for direction in 0..2 {
            let (mut session, solid, pairs) = fixture(actor, direction);
            let original = session.document.vmf().clone();
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some("brick/unequal"),
                )
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 1)],
                    Some("wood/unequal"),
                )
                .unwrap();
            let token = session
                .update_component_partition_cut(token, [0.2; 2])
                .unwrap();
            let side = session.checked_partition(token).unwrap().partitions[0]
                .source
                .side;
            session.store_component_partition(actor, token).unwrap();
            let saved = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            session
                .select_component_partitions(token, &[piece(solid, 0)], SelectMode::Replace)
                .unwrap();
            let token = session
                .raise_component_partition_with_texture(token, 16.125, Some(true))
                .unwrap();
            let draft = session.checked_partition(token).unwrap();
            assert_eq!(draft.partitions[0].cut().unwrap().edge_parameters, [0.2; 2]);
            assert_eq!(draft.selection.len(), 1);
            assert_eq!(session.document.vmf(), &saved);
            assert_eq!(session.document.undo_labels().count(), history);
            let raised = session.render_document().vmf().clone();
            let token = session.raise_component_partition(token, 0.0).unwrap();
            assert_eq!(session.render_document().vmf(), &saved);
            let token = session
                .update_component_partition_cut(token, [0.2, 0.8])
                .unwrap();
            let report = session.component_report(&[solid]).unwrap();
            let error = session
                .raise_component_partition(token, 16.125)
                .unwrap_err();
            assert!(error.contains("nonplanar"));
            assert_eq!(session.partition_preview_token(), Some(token));
            assert_eq!(session.component_report(&[solid]).unwrap(), report);
            assert_eq!(session.document.vmf(), &saved);
            let token = session
                .update_component_partition_cut(token, [0.2; 2])
                .unwrap();
            let token = session.raise_component_partition(token, 16.125).unwrap();
            assert_eq!(session.render_document().vmf(), &raised);
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &raised);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.saved_partition_report()["entries"], json!([]));
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &saved);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &raised);
            session.undo(actor).unwrap();
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &original);
        }
    }
}

#[test]
fn staging_a_changed_incidence_does_not_reassign_saved_materials() {
    for actor in [Actor::User, Actor::Agent] {
        for direction in 0..2 {
            let (mut session, solid, pairs) = fixture(actor, direction);
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some("brick/planned"),
                )
                .unwrap();
            let source = session.checked_partition(token).unwrap().partitions[0]
                .source
                .clone();
            session.store_component_partition(actor, token).unwrap();
            session
                .edit(actor, "change corner incidence", |tx| {
                    let id = tx.new_side_id()?;
                    let owner = subobject::solid(tx.document(), solid)?.0;
                    let brush = match owner {
                        ObjectId::Solid(id) => tx.solid_mut(id)?,
                        ObjectId::Entity(id) => tx
                            .entity_mut(id)?
                            .solids
                            .iter_mut()
                            .find(|brush| brush.id == solid)
                            .unwrap(),
                    };
                    brush
                        .sides
                        .iter_mut()
                        .find(|side| {
                            side.id != source.side
                                && !source.edges.contains(&side.id)
                                && map_core::Plane::from_points(side.plane.0)
                                    .unwrap()
                                    .normal
                                    .z
                                    .abs()
                                    < 0.99
                        })
                        .unwrap()
                        .id = id;
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                session.saved_partition_report()["entries"][0]["valid"],
                false
            );
            session
                .set_component_picking(
                    Some(crate::session::component_picking::Kind::Face),
                    &[solid],
                    &session.component_stamp(),
                )
                .unwrap();
            session
                .select_components(
                    &[Component {
                        solid,
                        sides: vec![source.side],
                    }],
                    SelectMode::Replace,
                    &session.component_stamp(),
                )
                .unwrap();
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            assert!(
                session
                    .checked_partition(token)
                    .unwrap()
                    .materials
                    .is_empty()
            );
            assert!(
                session
                    .update_component_partition_material(
                        token,
                        &[piece(solid, 0)],
                        Some("wood/reassigned")
                    )
                    .is_err()
            );
            assert_eq!(session.partition_preview_token(), Some(token));
            let token = session.raise_component_partition(token, 16.125).unwrap();
            let brush = subobject::solid(session.render_document(), solid)
                .unwrap()
                .1;
            assert!(brush.sides.iter().all(|side| side.material == "BRICK/TEST"));
            session.cancel_component_move(token).unwrap();
        }
    }
}

#[test]
fn material_drafts_share_actors_keep_cancel_and_exact_geometry_replay() {
    for actor in [Actor::User, Actor::Agent] {
        for direction in 0..2 {
            let (mut session, solid, pairs) = fixture(actor, direction);
            session
                .document
                .edit("source surface", |tx| {
                    let owner = subobject::solid(tx.document(), solid)?.0;
                    let source = match owner {
                        ObjectId::Solid(id) => tx.solid_mut(id)?,
                        ObjectId::Entity(id) => tx
                            .entity_mut(id)?
                            .solids
                            .iter_mut()
                            .find(|brush| brush.id == solid)
                            .unwrap(),
                    };
                    for side in &mut source.sides {
                        side.u_axis.offset = 12.0;
                        side.v_axis.offset = -8.0;
                        side.lightmap_scale = 16;
                        side.smoothing_groups = Some(0x8000_0001);
                    }
                    Ok(())
                })
                .unwrap();
            let original = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some(" brick/planned "),
                )
                .unwrap();
            let report = session.component_report(&[solid]).unwrap();
            let loops = &report["preview"]["partition"]["partitions"][0]["loops"];
            assert_eq!(loops[0]["top_material"], "brick/planned");
            assert_eq!(loops[1]["effective_material"], "brick/test");
            let mesh = session.build_scene();
            assert_eq!(mesh.indices.len(), 42);
            let source_side = subobject::solid(&session.document, solid)
                .unwrap()
                .1
                .sides
                .iter()
                .find(|side| {
                    side.id
                        == session.checked_partition(token).unwrap().partitions[0]
                            .source
                            .side
                })
                .unwrap();
            let top = mesh
                .vertices
                .iter()
                .filter(|vertex| vertex.normal[2] > 0.99)
                .collect::<Vec<_>>();
            assert_eq!(top.len(), 8);
            for vertex in top {
                let point = glam::Vec3::from_array(vertex.position).as_dvec3();
                let uv = map_core::texel_uv(source_side, point) / 128.0;
                assert!(
                    glam::Vec2::from_array(vertex.uv)
                        .as_dvec2()
                        .abs_diff_eq(uv, 1e-6)
                );
                assert_eq!(vertex.smoothing, 0x8000_0001);
                assert_eq!(vertex.lightmap[2], 16.0);
            }
            assert_eq!(session.document.vmf(), &original);
            assert_eq!(session.document.undo_labels().count(), history);
            for refs in [
                vec![],
                vec![piece(solid, 2)],
                vec![piece(solid, 0); 2],
                vec![piece(u32::MAX, 0)],
            ] {
                assert!(
                    session
                        .update_component_partition_material(token, &refs, Some("test"))
                        .is_err()
                );
                assert_eq!(session.partition_preview_token(), Some(token));
            }
            for name in [
                "".into(),
                "test\n".into(),
                "x".repeat(map_core::partition_metadata::MAX_TEXT_BYTES + 1),
            ] {
                assert!(
                    session
                        .update_component_partition_material(token, &[piece(solid, 0)], Some(&name))
                        .is_err()
                );
                assert_eq!(session.partition_preview_token(), Some(token));
            }
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 1)],
                    Some("wood/planned"),
                )
                .unwrap();
            let token = session
                .update_component_partition_material(token, &[piece(solid, 1)], None)
                .unwrap();
            assert_eq!(session.checked_partition(token).unwrap().materials.len(), 1);
            session.cancel_component_move(token).unwrap();
            assert_eq!(session.document.vmf(), &original);
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some("brick/planned"),
                )
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 1)],
                    Some("wood/planned"),
                )
                .unwrap();
            let side = session.checked_partition(token).unwrap().partitions[0]
                .source
                .side;
            session.store_component_partition(actor, token).unwrap();
            let stored = session.document.vmf().clone();
            assert_eq!(stored.world, original.world);
            assert_eq!(stored.entities, original.entities);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(
                session.saved_partition_report()["entries"][0]["top_materials"],
                json!(["brick/planned", "wood/planned"])
            );
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            assert_eq!(session.checked_partition(token).unwrap().materials.len(), 2);
            session.store_component_partition(actor, token).unwrap();
            assert_eq!(session.document.undo_labels().count(), history + 1);
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            let token = session.raise_component_partition(token, 16.125).unwrap();
            assert!(
                session
                    .update_component_partition_material(token, &[piece(solid, 0)], None)
                    .is_err()
            );
            let raised = session.render_document().vmf().clone();
            let (_, brush) = subobject::solid(session.render_document(), solid).unwrap();
            assert_eq!(
                brush
                    .sides
                    .iter()
                    .filter(|side| side.material.ends_with("/PLANNED"))
                    .count(),
                2
            );
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &raised);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &stored);
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            let token = session
                .preview_partition_extrusion(
                    token,
                    subobject::ExtrusionSpec {
                        height: 24.125,
                        wall_material: Some("concrete/walls".into()),
                        texture_lock: true,
                    },
                )
                .unwrap();
            let expected = session.render_document().vmf().clone();
            let output = &session
                .geometry
                .components
                .preview
                .as_ref()
                .unwrap()
                .extrusion
                .as_ref()
                .unwrap()
                .plan;
            for item in &output.pieces {
                for face in &item.faces {
                    let material = &item
                        .solid
                        .sides
                        .iter()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .material;
                    match face.surface {
                        subobject::ExtrusionSurface::Top => assert_eq!(
                            material,
                            if item.partition == 0 {
                                "BRICK/PLANNED"
                            } else {
                                "WOOD/PLANNED"
                            }
                        ),
                        subobject::ExtrusionSurface::Base => assert_eq!(material, "BRICK/TEST"),
                        _ => assert_eq!(material, "CONCRETE/WALLS"),
                    }
                }
            }
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &expected);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &stored);
        }
    }
}

#[test]
fn planned_surface_visibility_and_picking_follow_each_piece_and_owner() {
    let assets = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(assets.path().join("materials/tools")).unwrap();
    std::fs::write(
        assets.path().join("materials/tools/toolsnodraw.vmt"),
        "UnlitGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let gameinfo = assets.path().join("gameinfo.txt");
    std::fs::write(
        &gameinfo,
        "GameInfo { FileSystem { SearchPaths { Game |gameinfo_path|. } } }",
    )
    .unwrap();
    let game =
        std::sync::Arc::new(source_formats::gamefs::GameFs::from_gameinfo(gameinfo).unwrap());
    for direction in 0..2 {
        for tool_source in [false, true] {
            let (mut session, solid, pairs) = fixture(Actor::Agent, direction);
            session.game = game.clone();
            session.view.tool_textures = false;
            if tool_source {
                let owner = subobject::solid(&session.document, solid).unwrap().0;
                session
                    .edit(Actor::Agent, "tool source", |tx| {
                        map_core::edit::set_material(
                            tx,
                            &[map_core::edit::FaceTarget::Object(owner)],
                            "tools/toolsnodraw",
                        )
                    })
                    .unwrap();
            }
            let side = subobject::solid(&session.document, solid)
                .unwrap()
                .1
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            session
                .set_component_picking(
                    Some(crate::session::component_picking::Kind::Face),
                    &[solid],
                    &session.component_stamp(),
                )
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
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some(if tool_source {
                        "brick/planned"
                    } else {
                        "tools/toolsnodraw"
                    }),
                )
                .unwrap();
            let draft = session.checked_partition(token).unwrap().clone();
            let visibility = session.partition_piece_visibility(&draft);
            assert_eq!(visibility[&piece(solid, 0)], tool_source);
            assert_eq!(visibility[&piece(solid, 1)], !tool_source);
            let mut mesh = session.build_scene();
            mesh.lines.clear();
            session.append_partition_guides(&mut mesh);
            assert_eq!(mesh.lines.len(), 8);
            let part = &draft.partitions[0];
            let divider = (part.vertices[4].position + part.vertices[5].position) * 0.5;
            let invisible = part
                .loops
                .iter()
                .find(|item| !visibility[&piece(solid, item.id)])
                .unwrap();
            let center = invisible
                .vertices
                .iter()
                .map(|index| part.vertices[*index as usize].position)
                .sum::<DVec3>()
                / 4.0;
            let point = divider + (center - divider).normalize() * (256.0 / 700.0 * 0.25);
            let camera = Camera::planar(Projection::Top, divider.as_vec3(), 256.0);
            let picks = session
                .pick_component_partitions(
                    token,
                    camera,
                    Pointer {
                        position: render::handles::project(&camera, point.as_vec3(), (1100, 700))
                            .unwrap()
                            .as_dvec2(),
                        size: (1100, 700),
                    },
                )
                .unwrap();
            assert_eq!(picks.len(), 1);
            assert!(visibility[&picks[0].partition]);
            for item in &draft.partitions[0].loops {
                let part = &draft.partitions[0];
                let center = item
                    .vertices
                    .iter()
                    .map(|index| part.vertices[*index as usize].position)
                    .sum::<DVec3>()
                    / 4.0;
                let camera = Camera::planar(Projection::Top, center.as_vec3(), 256.0);
                let pointer = Pointer {
                    position: render::handles::project(&camera, center.as_vec3(), (1100, 700))
                        .unwrap()
                        .as_dvec2(),
                    size: (1100, 700),
                };
                let picks = session
                    .pick_component_partitions(token, camera, pointer)
                    .unwrap();
                assert_eq!(picks.len(), usize::from(visibility[&piece(solid, item.id)]));
                if let Some(hit) = picks.first() {
                    assert_eq!(hit.partition, piece(solid, item.id));
                }
            }
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0), piece(solid, 1)],
                    Some("tools/toolsnodraw"),
                )
                .unwrap();
            let token = session
                .preview_partition_extrusion(
                    token,
                    subobject::ExtrusionSpec {
                        height: 32.0,
                        wall_material: Some("brick/walls".into()),
                        texture_lock: false,
                    },
                )
                .unwrap();
            let camera = Camera::planar(Projection::Top, glam::Vec3::new(32.0, 32.0, 64.0), 256.0);
            assert!(
                session
                    .pick_component_partitions(
                        token,
                        camera,
                        Pointer {
                            position: glam::DVec2::new(550.0, 350.0),
                            size: (1100, 700)
                        }
                    )
                    .unwrap()
                    .is_empty()
            );
            let mut mesh = session.build_scene();
            mesh.lines.clear();
            session.append_partition_guides(&mut mesh);
            assert_eq!(mesh.lines.len(), 48);
            let token = session.raise_component_partition(token, 0.0).unwrap();
            let token = session
                .update_component_partition_material(
                    token,
                    &[piece(solid, 0)],
                    Some(if tool_source {
                        "brick/planned"
                    } else {
                        "tools/toolsnodraw"
                    }),
                )
                .unwrap();
            let token = session
                .update_component_partition_material(token, &[piece(solid, 1)], None)
                .unwrap();
            session
                .store_component_partition(Actor::Agent, token)
                .unwrap();
            let mut mesh = session.build_scene();
            mesh.lines.clear();
            session.append_partition_guides(&mut mesh);
            assert_eq!(mesh.lines.len(), 8);
            session
                .edit(Actor::Agent, "hide owner", |tx| {
                    match draft.partitions[0].owner {
                        ObjectId::Solid(id) => tx.solid_mut(id)?.hidden = true,
                        ObjectId::Entity(id) => tx.entity_mut(id)?.hidden = true,
                    }
                    Ok(())
                })
                .unwrap();
            let mut mesh = session.build_scene();
            mesh.lines.clear();
            session.append_partition_guides(&mut mesh);
            assert!(mesh.lines.is_empty());
            assert!(mesh.indices.is_empty());
        }
    }
}
