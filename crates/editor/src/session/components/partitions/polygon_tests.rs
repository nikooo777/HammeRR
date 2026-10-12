use super::*;
use crate::session::Pointer;
use map_core::{Plane, primitive};
use render::Camera;

fn fixture(actor: Actor, corners: u32, owned: bool) -> (Session, u32, u32) {
    let mut session = Session::new(Err("no game needed".into()));
    let objects = session
        .create_primitive(
            actor,
            primitive::Spec {
                shape: primitive::Shape::Cylinder,
                segments: corners,
                mins: DVec3::new(0.125, -7.25, 16.5),
                maxs: DVec3::new(64.875, 48.125, 80.25),
                material: "brick/test".into(),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let ObjectId::Solid(id) = objects[0] else {
        unreachable!()
    };
    if owned {
        session
            .tie_to_entity(actor, &objects, "func_detail", Default::default())
            .unwrap();
    }
    let side = subobject::solid(session.document(), id)
        .unwrap()
        .1
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    session
        .set_component_picking(
            Some(crate::session::component_picking::Kind::Face),
            &[id],
            &session.component_stamp(),
        )
        .unwrap();
    session
        .select_components(
            &[Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    (session, id, side)
}

#[test]
fn polygon_session_authors_all_choices_and_shares_storage_materials_picking_and_exact_replay() {
    for actor in [Actor::User, Actor::Agent] {
        for corners in [3, 5, 6, 8] {
            for owned in [false, true] {
                let (original, _, side) = fixture(actor, corners, owned);
                let choices = original.component_partition_choices().unwrap();
                assert_eq!(choices[0].1.len(), (corners * (corners - 1) / 2) as usize);
                assert!(original.component_crease_choices().unwrap().is_empty());
                for choice in &choices[0].1 {
                    let (mut session, id, _) = fixture(actor, corners, owned);
                    let source = session.document.vmf().clone();
                    let token = session
                        .preview_component_partition(
                            &[CreaseEdges {
                                solid: id,
                                edges: choice.edges,
                            }],
                            &session.component_stamp(),
                        )
                        .unwrap();
                    let token = session
                        .update_component_partition_cut(token, [0.2, 0.8])
                        .unwrap();
                    let token = session
                        .update_component_partition_material(
                            token,
                            &[PartitionRef {
                                solid: id,
                                partition: 0,
                            }],
                            Some("brick/red"),
                        )
                        .unwrap();
                    let token = session
                        .update_component_partition_material(
                            token,
                            &[PartitionRef {
                                solid: id,
                                partition: 1,
                            }],
                            Some("wood/blue"),
                        )
                        .unwrap();
                    let report = session.component_report(&[id]).unwrap();
                    assert_eq!(report["preview"]["partition"]["ridge_supported"], false);
                    assert!(
                        session
                            .raise_component_partition(token, 16.0)
                            .unwrap_err()
                            .contains("two quad")
                    );
                    assert_eq!(session.component_report(&[id]).unwrap(), report);
                    assert!(
                        session
                            .update_component_partition_cut(token, [0.0, 0.8])
                            .is_err()
                    );
                    assert_eq!(session.partition_preview_token(), Some(token));
                    assert!(
                        session
                            .preview_component_partition(
                                &vec![
                                    CreaseEdges {
                                        solid: id,
                                        edges: choice.edges
                                    };
                                    subobject::MAX_SOLIDS + 1
                                ],
                                &session.component_stamp()
                            )
                            .is_err()
                    );
                    assert_eq!(session.component_report(&[id]).unwrap(), report);

                    let draft = session.checked_partition(token).unwrap().clone();
                    let part = &draft.partitions[0];
                    let membership = part.loops.each_ref().map(|piece| {
                        piece
                            .vertices
                            .iter()
                            .filter_map(|index| match &part.vertices[*index as usize].source {
                                VertexSource::Corner(sides) => Some(sides.clone()),
                                _ => None,
                            })
                            .collect::<BTreeSet<_>>()
                    });
                    for piece in &part.loops {
                        let center = piece
                            .vertices
                            .iter()
                            .map(|index| part.vertices[*index as usize].position)
                            .sum::<DVec3>()
                            / piece.vertices.len() as f64;
                        let camera = Camera::looking_at(
                            (center + part.normal() * 256.0).as_vec3(),
                            center.as_vec3(),
                        );
                        let candidates = session
                            .pick_component_partitions(
                                token,
                                camera,
                                Pointer {
                                    position: glam::DVec2::new(550.0, 350.0),
                                    size: (1100, 700),
                                },
                            )
                            .unwrap();
                        assert_eq!(candidates.len(), 1);
                        assert_eq!(candidates[0].partition.partition, piece.id);
                    }
                    session
                        .frame_component_partitions(actor, token, 1.4)
                        .unwrap();
                    let mut mesh = session.build_scene();
                    mesh.lines.clear();
                    session.append_partition_guides(&mut mesh);
                    assert_eq!(mesh.lines.len(), (corners as usize + 4) * 2);
                    assert_eq!(session.document.vmf(), &source);
                    session.store_component_partition(actor, token).unwrap();
                    let stored = session.document.vmf().clone();
                    assert_eq!(stored.world, source.world);
                    assert_eq!(stored.entities, source.entities);
                    let token = session
                        .resume_component_partitions(&[(id, side)], &session.component_stamp())
                        .unwrap();
                    assert_eq!(
                        session.checked_partition(token).unwrap().materials,
                        draft.materials
                    );
                    session.store_component_partition(actor, token).unwrap();
                    let history = session.document.undo_labels().count();
                    let token = session
                        .resume_component_partitions(&[(id, side)], &session.component_stamp())
                        .unwrap();
                    let spec = subobject::ExtrusionSpec {
                        height: 16.125,
                        wall_material: None,
                        texture_lock: true,
                    };
                    let token = session
                        .preview_partition_extrusion(token, spec.clone())
                        .unwrap();
                    let applied = session.render_document().vmf().clone();
                    session.cancel_component_move(token).unwrap();
                    assert_eq!(session.document.vmf(), &stored);
                    assert_eq!(session.document.undo_labels().count(), history);
                    let token = session
                        .resume_component_partitions(&[(id, side)], &session.component_stamp())
                        .unwrap();
                    let token = session.preview_partition_extrusion(token, spec).unwrap();
                    assert_eq!(session.render_document().vmf(), &applied);
                    session.commit_component_edit(actor, token).unwrap();
                    assert_eq!(session.document.vmf(), &applied);
                    assert_eq!(session.selected_faces().len(), corners as usize + 8);
                    session.undo(actor).unwrap();
                    assert_eq!(session.document.vmf(), &stored);
                    session.redo(actor).unwrap();
                    assert_eq!(session.document.vmf(), &applied);
                    let metadata =
                        map_core::partition_metadata::Metadata::parse(session.document.vmf())
                            .unwrap();
                    let resolved = metadata
                        .resolve(session.document())
                        .remove(0)
                        .result
                        .unwrap();
                    let actual = resolved.loops.each_ref().map(|piece| {
                        piece
                            .vertices
                            .iter()
                            .filter_map(|index| match &resolved.vertices[*index as usize].source {
                                VertexSource::Corner(sides) => Some(sides.clone()),
                                _ => None,
                            })
                            .collect::<BTreeSet<_>>()
                    });
                    assert_eq!(actual, membership);
                }
            }
        }
    }
}

#[test]
fn mixed_polygon_batch_keeps_and_extrudes_at_the_aggregate_plane_limit() {
    let mut session = Session::new(Err("no game needed".into()));
    let mut sources = Vec::new();
    let mut pairs = Vec::new();
    for index in 0..subobject::MAX_SOLIDS {
        let objects = session
            .create_primitive(
                Actor::Agent,
                primitive::Spec {
                    shape: primitive::Shape::Cylinder,
                    segments: 8,
                    mins: DVec3::new(index as f64 * 128.0, 0.0, 16.0),
                    maxs: DVec3::new(index as f64 * 128.0 + 64.0, 64.0, 80.0),
                    material: "brick/test".into(),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        let ObjectId::Solid(id) = objects[0] else {
            unreachable!()
        };
        if index % 2 == 1 {
            session
                .tie_to_entity(Actor::Agent, &objects, "func_detail", Default::default())
                .unwrap();
        }
        let source = subobject::solid(session.document(), id).unwrap().1;
        let side = source
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let choices = subobject::partition_pairs(source, side).unwrap();
        pairs.push(CreaseEdges {
            solid: id,
            edges: choices[index % choices.len()].edges,
        });
        sources.push((id, side));
    }
    session
        .set_component_picking(
            Some(crate::session::component_picking::Kind::Face),
            &sources.iter().map(|source| source.0).collect::<Vec<_>>(),
            &session.component_stamp(),
        )
        .unwrap();
    session
        .select_components(
            &sources
                .iter()
                .map(|(id, side)| Component {
                    solid: *id,
                    sides: vec![*side],
                })
                .collect::<Vec<_>>(),
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let token = session
        .preview_component_partition(&pairs, &session.component_stamp())
        .unwrap();
    session
        .store_component_partition(Actor::User, token)
        .unwrap();
    let stored = session.document.vmf().clone();
    session.undo(Actor::Agent).unwrap();
    assert!(
        session.saved_partition_report()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    session.redo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &stored);
    let token = session
        .resume_component_partitions(&sources, &session.component_stamp())
        .unwrap();
    let token = session
        .preview_partition_extrusion(
            token,
            subobject::ExtrusionSpec {
                height: 16.0,
                wall_material: None,
                texture_lock: true,
            },
        )
        .unwrap();
    let draft = session
        .checked_component_preview(token)
        .unwrap()
        .extrusion
        .as_ref()
        .unwrap();
    assert_eq!(draft.plan.pieces.len(), subobject::MAX_EXTRUSION_PIECES);
    assert_eq!(
        draft
            .plan
            .pieces
            .iter()
            .map(|piece| piece.solid.sides.len())
            .sum::<usize>(),
        subobject::MAX_EXTRUSION_PLANES
    );
    let applied = session.render_document().vmf().clone();
    session.cancel_component_move(token).unwrap();
    assert_eq!(session.document.vmf(), &stored);
    let token = session
        .resume_component_partitions(&sources, &session.component_stamp())
        .unwrap();
    let token = session
        .preview_partition_extrusion(
            token,
            subobject::ExtrusionSpec {
                height: 16.0,
                wall_material: None,
                texture_lock: true,
            },
        )
        .unwrap();
    assert_eq!(session.render_document().vmf(), &applied);
    session.commit_component_edit(Actor::Agent, token).unwrap();
    assert_eq!(
        session.selected_faces().len(),
        subobject::MAX_EXTRUSION_PLANES
    );
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &stored);
}
