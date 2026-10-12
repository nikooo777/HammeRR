use map_core::subobject::{Partition, VertexSource};

use super::*;

mod materials;
mod picking;
mod textures;

#[cfg(test)]
mod polygon_tests;

pub(super) fn consume_saved(
    tx: &mut map_core::Transaction,
    requests: &[Crease],
) -> Result<(), EditError> {
    if let Ok(mut metadata) = map_core::partition_metadata::Metadata::parse(tx.vmf()) {
        let count = metadata.entries.len();
        for request in requests {
            metadata.entries.remove(&(request.solid, request.side));
        }
        if metadata.entries.len() != count {
            metadata.write(tx)?;
        }
    }
    Ok(())
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PartitionRef {
    pub solid: u32,
    pub partition: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_partition_lifecycle_shares_actors_geometry_history_and_materialization() {
        for actor in [Actor::User, Actor::Agent] {
            for direction in 0..2 {
                let (mut session, id, pairs) = fixture(actor, direction);
                let source = session.document.vmf().clone();
                let history = session.document.undo_labels().count();
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                session.store_component_partition(actor, token).unwrap();
                let stored = session.document.vmf().clone();
                assert_eq!(stored.world, source.world);
                assert_eq!(stored.entities, source.entities);
                assert_eq!(session.partition_preview_token(), None);
                session.ensure_partitions_resolved().unwrap();
                assert_eq!(session.document.undo_labels().count(), history + 1);
                let report = session.saved_partition_report();
                assert_eq!(report["entries"].as_array().unwrap().len(), 1);
                assert_eq!(report["entries"][0]["valid"], true);
                let side = report["entries"][0]["side"].as_u64().unwrap() as u32;
                assert!(
                    session
                        .resume_component_partitions(
                            &[(id, side), (id, u32::MAX)],
                            &session.component_stamp()
                        )
                        .is_err()
                );
                assert_eq!(session.partition_preview_token(), None);
                let token = session
                    .resume_component_partitions(&[(id, side)], &session.component_stamp())
                    .unwrap();
                let token = session.raise_component_partition(token, 16.125).unwrap();
                let raised = session.render_document().vmf().clone();
                assert!(
                    map_core::partition_metadata::Metadata::parse(&raised)
                        .unwrap()
                        .entries
                        .is_empty()
                );
                assert!(session.store_component_partition(actor, token).is_err());
                session.commit_component_edit(actor, token).unwrap();
                assert_eq!(session.document.vmf(), &raised);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &stored);
                session
                    .remove_saved_component_partitions(
                        actor,
                        &[(id, side)],
                        &session.component_stamp(),
                    )
                    .unwrap();
                assert_eq!(session.document.vmf(), &source);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &stored);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &source);
            }
        }
    }

    #[test]
    fn keeping_an_unchanged_saved_partition_closes_the_draft_without_extra_history() {
        let (mut session, id, pairs) = fixture(Actor::Agent, 0);
        let token = session
            .preview_component_partition(&pairs, &session.component_stamp())
            .unwrap();
        session
            .store_component_partition(Actor::Agent, token)
            .unwrap();
        let report = session.saved_partition_report();
        let side = report["entries"][0]["side"].as_u64().unwrap() as u32;
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let token = session
            .resume_component_partitions(&[(id, side)], &session.component_stamp())
            .unwrap();
        session
            .store_component_partition(Actor::Agent, token)
            .unwrap();
        assert_eq!(session.partition_preview_token(), None);
        session.ensure_partitions_resolved().unwrap();
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        let mut mesh = session.build_scene();
        mesh.lines.clear();
        session.append_partition_guides(&mut mesh);
        assert_eq!(mesh.lines.len(), 16);
        session
            .set_component_picking(None, &[], &session.component_stamp())
            .unwrap();
        mesh.lines.clear();
        session.append_partition_guides(&mut mesh);
        assert!(mesh.lines.is_empty());
    }

    #[test]
    fn draft_loop_picking_shares_projection_boundaries_and_read_only_guards() {
        use crate::session::Pointer;
        use render::{Camera, Projection};
        for actor in [Actor::User, Actor::Agent] {
            for direction in 0..2 {
                let (mut session, id, pairs) = fixture(actor, direction);
                let source = session.document().vmf().clone();
                let history = session.document().undo_labels().count();
                let mut token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                for rise in [0.0, 16.125] {
                    if rise != 0.0 {
                        let previous = token;
                        token = session.raise_component_partition(token, rise).unwrap();
                        assert!(
                            session
                                .pick_component_partitions(
                                    previous,
                                    session.camera(),
                                    Pointer {
                                        position: glam::DVec2::splat(300.0),
                                        size: (1100, 700)
                                    }
                                )
                                .is_err()
                        );
                    }
                    let part = session.checked_partition(token).unwrap().partitions[0].clone();
                    let positions = part.positions(rise);
                    let near = Camera::looking_at(
                        glam::Vec3::new(32.0, 32.0, 64.0 + rise as f32 + 2.0),
                        glam::Vec3::new(32.0, 32.0, 64.0 + rise as f32),
                    );
                    let center_pointer = Pointer {
                        position: glam::DVec2::new(550.0, 350.0),
                        size: (1100, 700),
                    };
                    assert!(
                        session
                            .pick_component_partitions(token, near, center_pointer)
                            .unwrap()
                            .is_empty()
                    );
                    let far = Camera {
                        position: glam::Vec3::new(32.0, 32.0, 200000.0),
                        ..Camera::planar(Projection::Top, glam::Vec3::splat(32.0), 256.0)
                    };
                    assert!(
                        session
                            .pick_component_partitions(token, far, center_pointer)
                            .unwrap()
                            .is_empty()
                    );
                    for projection in [Projection::Top, Projection::Perspective] {
                        let camera = if projection == Projection::Top {
                            Camera::planar(projection, glam::Vec3::splat(32.0), 256.0)
                        } else {
                            Camera::looking_at(
                                glam::Vec3::new(35.0, 39.0, 350.0),
                                glam::Vec3::new(32.0, 32.0, 64.0),
                            )
                        };
                        let before = session.component_report(&[id]).unwrap();
                        for piece in &part.loops {
                            let point = piece
                                .vertices
                                .iter()
                                .map(|index| positions[*index as usize])
                                .sum::<DVec3>()
                                / 4.0;
                            let pointer = Pointer {
                                position: render::handles::project(
                                    &camera,
                                    point.as_vec3(),
                                    (1100, 700),
                                )
                                .unwrap()
                                .as_dvec2(),
                                size: (1100, 700),
                            };
                            let picks = session
                                .pick_component_partitions(token, camera, pointer)
                                .unwrap();
                            assert_eq!(picks.len(), 1);
                            assert_eq!(
                                picks[0].partition,
                                PartitionRef {
                                    solid: id,
                                    partition: piece.id
                                }
                            );
                            assert!(picks[0].position.distance(point) < 0.001);
                            assert_eq!(session.component_report(&[id]).unwrap(), before);
                        }
                        let point = (positions[4] + positions[5]) * 0.5;
                        let pointer = Pointer {
                            position: render::handles::project(
                                &camera,
                                point.as_vec3(),
                                (1100, 700),
                            )
                            .unwrap()
                            .as_dvec2(),
                            size: (1100, 700),
                        };
                        let picks = session
                            .pick_component_partitions(token, camera, pointer)
                            .unwrap();
                        assert_eq!(
                            picks
                                .iter()
                                .map(|pick| pick.partition.partition)
                                .collect::<Vec<_>>(),
                            vec![0, 1],
                            "direction={direction} rise={rise} projection={projection:?} picks={picks:?} pointer={pointer:?}"
                        );
                        assert!(
                            session
                                .pick_component_partitions(
                                    token,
                                    camera,
                                    Pointer {
                                        position: glam::DVec2::new(-1.0, 350.0),
                                        ..pointer
                                    }
                                )
                                .is_err()
                        );
                        assert!(
                            session
                                .pick_component_partitions(
                                    token,
                                    Camera {
                                        fov_y_degrees: f32::NAN,
                                        ..camera
                                    },
                                    pointer
                                )
                                .is_err()
                        );
                        assert_eq!(session.document().vmf(), &source);
                        assert_eq!(session.document().undo_labels().count(), history);
                    }
                }
                session.cancel_component_move(token).unwrap();
                assert!(
                    session
                        .pick_component_partitions(
                            token,
                            session.camera(),
                            Pointer {
                                position: glam::DVec2::splat(300.0),
                                size: (1100, 700)
                            }
                        )
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn roof_attachment_policy_survives_omitted_updates_flattening_and_exact_apply() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, id, pairs) = fixture(actor, 0);
            let source = session.document().vmf().clone();
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            assert!(!session.checked_partition(token).unwrap().roof_texture_lock);
            let token = session
                .raise_component_partition_with_texture(token, 16.125, Some(true))
                .unwrap();
            assert_eq!(
                session.component_report(&[id]).unwrap()["texture_mode"],
                "roof_attached"
            );
            assert!(session.checked_partition(token).unwrap().roof_texture_lock);
            let token = session.raise_component_partition(token, 32.25).unwrap();
            let token = session.raise_component_partition(token, 0.0).unwrap();
            assert_eq!(session.render_document().vmf(), &source);
            assert!(session.checked_partition(token).unwrap().roof_texture_lock);
            let token = session.raise_component_partition(token, 16.125).unwrap();
            let expected = session.render_document().vmf().clone();
            assert!(
                session
                    .checked_component_preview(token)
                    .unwrap()
                    .crease
                    .as_ref()
                    .unwrap()
                    .roof_texture_lock
            );
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document().vmf(), &expected);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &source);
        }
    }

    pub(super) fn fixture(actor: Actor, direction: usize) -> (Session, u32, Vec<CreaseEdges>) {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
            .unwrap()
        else {
            unreachable!()
        };
        if direction == 1 {
            session
                .edit(actor, "detail", |tx| {
                    map_core::edit::tie_to_entity(tx, &[id], "func_detail", &[])
                })
                .unwrap();
        }
        let (_, source) = subobject::solid(session.document(), id).unwrap();
        let side = source
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let edges = subobject::crease_pairs(source, side).unwrap()[direction].edges;
        session
            .set_component_picking(
                Some(super::super::super::component_picking::Kind::Face),
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
        session.document.mark_saved();
        (session, id, vec![CreaseEdges { solid: id, edges }])
    }

    #[test]
    fn adjustable_partition_cut_shares_saved_identity_extrusion_and_exact_history() {
        for actor in [Actor::User, Actor::Agent] {
            for direction in 0..2 {
                let (mut session, id, pairs) = fixture(actor, direction);
                let source = session.document().vmf().clone();
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                let token = session
                    .update_component_partition_cut(token, [0.2, 0.8])
                    .unwrap();
                let draft = session
                    .components
                    .preview
                    .as_ref()
                    .unwrap()
                    .partition
                    .as_ref()
                    .unwrap()
                    .clone();
                assert_eq!(
                    draft.partitions[0].cut().unwrap().edge_parameters,
                    [0.2, 0.8]
                );
                assert_eq!(session.render_document().vmf(), &source);
                assert!(
                    session
                        .raise_component_partition(token, 16.0)
                        .unwrap_err()
                        .contains("nonplanar")
                );
                assert_eq!(session.partition_preview_token(), Some(token));
                for parameters in [[0.0, 0.8], [0.2, 1.0], [f64::NAN, 0.8]] {
                    assert!(
                        session
                            .update_component_partition_cut(token, parameters)
                            .is_err()
                    );
                    assert_eq!(session.partition_preview_token(), Some(token));
                }
                session.store_component_partition(actor, token).unwrap();
                let stored = session.document().vmf().clone();
                assert_eq!(stored.world, source.world);
                let side = draft.partitions[0].source.side;
                let token = session
                    .resume_component_partitions(&[(id, side)], &session.component_stamp())
                    .unwrap();
                assert_eq!(
                    session
                        .components
                        .preview
                        .as_ref()
                        .unwrap()
                        .partition
                        .as_ref()
                        .unwrap()
                        .partitions,
                    draft.partitions
                );
                let token = session
                    .preview_partition_extrusion(
                        token,
                        subobject::ExtrusionSpec {
                            height: 32.125,
                            wall_material: None,
                            texture_lock: true,
                        },
                    )
                    .unwrap();
                assert!(
                    session
                        .update_component_partition_cut(token, [0.4, 0.6])
                        .is_err()
                );
                session.cancel_component_move(token).unwrap();
                assert_eq!(session.document().vmf(), &stored);
                session.undo(actor).unwrap();
                assert_eq!(session.document().vmf(), &source);
                session.redo(actor).unwrap();
                assert_eq!(session.document().vmf(), &stored);
                let token = session
                    .resume_component_partitions(&[(id, side)], &session.component_stamp())
                    .unwrap();
                let token = session
                    .update_component_partition_cut(token, [0.5; 2])
                    .unwrap();
                session.raise_component_partition(token, 16.0).unwrap();
            }
        }
    }

    #[test]
    fn partition_drafts_share_actor_history_selection_flattening_save_guards_and_id_replay() {
        for actor in [Actor::User, Actor::Agent] {
            for direction in 0..2 {
                let (mut session, id, pairs) = fixture(actor, direction);
                let source = session.document().vmf().clone();
                let history = session.document().undo_labels().count();
                let selected = session.component_selection().clone();
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("source.vmf");
                std::fs::write(&path, "existing source").unwrap();
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                assert_eq!(session.partition_preview_token(), Some(token));
                assert_eq!(session.render_document().vmf(), &source);
                assert!(!session.document.is_dirty());
                assert!(session.component_topology_preview());
                assert_eq!(session.checked_partition(token).unwrap().selection.len(), 2);
                assert_eq!(
                    session.component_report(&[id]).unwrap()["preview"]["can_apply"],
                    false
                );
                assert!(
                    session
                        .commit_component_edit(actor, token)
                        .unwrap_err()
                        .contains("Flat partitions")
                );
                assert!(
                    session
                        .save(actor, Some(&path))
                        .unwrap_err()
                        .contains("partition")
                );
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing source");
                assert!(
                    session
                        .begin_compile(actor, Some(&path.with_extension("bsp")))
                        .err()
                        .unwrap()
                        .contains("partition")
                );
                assert!(
                    session
                        .launch_readiness()
                        .err()
                        .unwrap()
                        .contains("partition")
                );
                assert!(
                    session
                        .new_map(actor, false)
                        .unwrap_err()
                        .contains("partition")
                );
                assert!(
                    session
                        .select_components(&[], SelectMode::Replace, &session.component_stamp())
                        .is_err()
                );
                assert!(
                    session
                        .preview_component_partition(&[], &session.component_stamp())
                        .is_err()
                );
                for rise in [f64::NAN, f64::INFINITY, -1.0, 0.01, 4097.0] {
                    assert!(session.raise_component_partition(token, rise).is_err());
                    assert_eq!(session.partition_preview_token(), Some(token));
                }
                let first = PartitionRef {
                    solid: id,
                    partition: 0,
                };
                let second = PartitionRef {
                    solid: id,
                    partition: 1,
                };
                session
                    .select_component_partitions(
                        token,
                        std::slice::from_ref(&first),
                        SelectMode::Replace,
                    )
                    .unwrap();
                session
                    .select_component_partitions(
                        token,
                        std::slice::from_ref(&second),
                        SelectMode::Add,
                    )
                    .unwrap();
                session
                    .select_component_partitions(
                        token,
                        std::slice::from_ref(&first),
                        SelectMode::Toggle,
                    )
                    .unwrap();
                assert_eq!(
                    session.checked_partition(token).unwrap().selection,
                    BTreeSet::from([second.clone()])
                );
                session
                    .frame_component_partitions(actor, token, 0.6)
                    .unwrap();
                assert!(session.camera().position.is_finite());
                assert!(
                    session
                        .select_component_partitions(
                            token,
                            &[PartitionRef {
                                solid: id,
                                partition: 9
                            }],
                            SelectMode::Replace
                        )
                        .is_err()
                );
                session
                    .select_component_partitions(token, &[], SelectMode::Replace)
                    .unwrap();
                let camera = session.camera();
                assert!(
                    session
                        .frame_component_partitions(actor, token, 1.6)
                        .is_err()
                );
                assert_eq!(session.camera(), camera);
                assert!(
                    session
                        .frame_component_partitions(actor, token, f32::NAN)
                        .is_err()
                );
                session
                    .select_component_partitions(token, &[first, second], SelectMode::Replace)
                    .unwrap();
                let raised = session.raise_component_partition(token, 16.125).unwrap();
                assert!(session.raise_component_partition(token, 0.0).is_err());
                assert_eq!(
                    subobject::topology(subobject::solid(session.render_document(), id).unwrap().1)
                        .unwrap()
                        .vertices
                        .len(),
                    10
                );
                let flat = session.raise_component_partition(raised, 0.0).unwrap();
                assert_eq!(session.render_document().vmf(), &source);
                assert_eq!(session.component_selection(), &selected);
                assert_eq!(session.document().undo_labels().count(), history);
                session.cancel_component_move(flat).unwrap();
                assert_eq!(session.partition_preview_token(), None);
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.component_selection(), &selected);
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                let token = session.raise_component_partition(token, 16.125).unwrap();
                let draft = session.render_document().vmf().clone();
                assert_eq!(
                    session
                        .checked_component_preview(token)
                        .unwrap()
                        .crease
                        .as_ref()
                        .unwrap()
                        .changes[0]
                        .new_side,
                    7
                );
                session.commit_component_edit(actor, token).unwrap();
                assert_eq!(session.document().vmf(), &draft);
                assert_eq!(session.document().undo_labels().count(), history + 1);
                assert_eq!(session.component_selection().len(), 2);
                session.undo(actor).unwrap();
                assert_eq!(session.document().vmf(), &source);
                session.redo(actor).unwrap();
                assert_eq!(session.document().vmf(), &draft);
                session.save(actor, Some(&path)).unwrap();
                let reopened =
                    source_formats::vmf::Vmf::parse(&std::fs::read_to_string(path).unwrap())
                        .unwrap();
                reopened.validate_ids().unwrap();
                assert_eq!(
                    subobject::topology(reopened.solids().find(|solid| solid.id == id).unwrap())
                        .unwrap()
                        .vertices
                        .len(),
                    10
                );
            }
        }
    }

    #[test]
    fn partition_source_and_allocation_changes_reject_stale_drafts_atomically() {
        let (mut session, id, pairs) = fixture(Actor::Agent, 0);
        let token = session
            .preview_component_partition(&pairs, &session.component_stamp())
            .unwrap();
        let raised = session.raise_component_partition(token, 16.0).unwrap();
        let source = session.document().vmf().clone();
        session
            .document
            .edit("allocation", |tx| tx.new_side_id())
            .unwrap();
        assert!(
            session
                .commit_component_edit(Actor::Agent, raised)
                .unwrap_err()
                .contains("allocation")
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.partition_preview_token(), Some(raised));
        session.cancel_component_move(raised).unwrap();
        let token = session
            .preview_component_partition(&pairs, &session.component_stamp())
            .unwrap();
        session
            .document
            .edit("source", |tx| {
                tx.solid_mut(id)?.sides[0].material = "wood/test".into();
                Ok(())
            })
            .unwrap();
        assert!(
            session
                .raise_component_partition(token, 16.0)
                .unwrap_err()
                .contains("Map changed")
        );
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.raise_component_partition(token, 16.0).is_err());
        assert_eq!(session.partition_preview_token(), None);
    }
}

#[derive(Clone)]
pub(super) struct Draft {
    pub partitions: Vec<Partition>,
    pub rise: f64,
    pub roof_texture_lock: bool,
    pub selection: BTreeSet<PartitionRef>,
    pub materials: BTreeMap<PartitionRef, String>,
    pub textures: BTreeMap<PartitionRef, subobject::PartitionTexture>,
}

impl Draft {
    fn requests(&self) -> Vec<Crease> {
        self.partitions
            .iter()
            .map(|partition| partition.source.clone())
            .collect()
    }

    pub fn report(&self, document: &map_core::Document) -> Value {
        let parameters = self
            .partitions
            .iter()
            .map(|partition| partition.cut().unwrap().edge_parameters)
            .collect::<Vec<_>>();
        let edge_parameters: [Option<f64>; 2] = std::array::from_fn(|index| {
            parameters
                .first()
                .map(|first| first[index])
                .filter(|value| {
                    parameters
                        .iter()
                        .all(|parameters| parameters[index] == *value)
                })
        });
        json!({"ridge_supported":self.partitions.iter().all(|part|part.loops.iter().all(|piece|piece.vertices.len()==4)),"edge_parameters":edge_parameters,"rise":self.rise,"roof_texture_lock":self.roof_texture_lock,"temporary":true,"selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "partitions":self.partitions.iter().map(|part| {
                let positions = part.positions(self.rise);
                let owner = match part.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")};
                json!({"solid":part.source.solid,"source_side":part.source.side,"source_corner_count":part.source_corner_count(),"edges":part.source.edges,"owner":owner,"plane":part.plane.map(|point|point.to_array()),
                    "vertices":part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"preview_position":positions[vertex.id as usize].to_array(),"source":match &vertex.source {VertexSource::Corner(sides)=>json!({"kind":"corner","sides":sides}),VertexSource::Edge{corners,parameter}=>json!({"kind":"edge","corners":corners,"parameter":parameter})}})).collect::<Vec<_>>(),
                    "loops":part.loops.iter().map(|piece|json!({"id":piece.id,"vertices":piece.vertices,"shape":match piece.vertices.len(){3=>"triangle",4=>"quad",5=>"pentagon",6=>"hexagon",7=>"heptagon",8=>"octagon",9=>"nonagon",_=>unreachable!()},"source_side":piece.source_side,"top_material":self.materials.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).map(|name|name.to_ascii_lowercase()),"effective_material":self.effective_material(document, part, piece.id).map(str::to_ascii_lowercase),"inherited":!self.materials.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"texture_plan":self.textures.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).copied().map(textures::texture_json),"effective_texture":self.effective_texture(document,part,piece.id).map(textures::texture_json),"texture_inherited":!self.textures.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"selected":self.selection.contains(&PartitionRef{solid:part.source.solid,partition:piece.id})})).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn saved_partition_report(&self) -> Value {
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf());
        let (entries, error) = match metadata {
            Ok(metadata) => (metadata.resolve(&self.document).into_iter().map(|entry| {
                json!({"solid":entry.source.solid,"side":entry.source.side,"edges":entry.source.edges,
                    "piece_membership":metadata.entries.get(&(entry.source.solid, entry.source.side)).and_then(|intent|intent.piece_membership.as_ref()),
                    "source_corner_count":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.corners.len()),
                    "top_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_materials.each_ref().map(|value|value.as_ref().map(|name|name.to_ascii_lowercase()))),
                    "top_textures":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_textures.map(|value|value.map(textures::texture_json))),
                    "effective_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent| std::array::from_fn::<_, 2, _>(|index| intent.top_materials[index].as_deref().or_else(||subobject::solid(&self.document, entry.source.solid).ok()?.1.sides.iter().find(|side|side.id==entry.source.side).map(|side|side.material.as_str())).map(str::to_ascii_lowercase))),
                    "valid":entry.result.is_ok(),"edge_parameters":entry.result.as_ref().ok().map(|part|part.cut().unwrap().edge_parameters),"error":entry.result.err()})
            }).collect::<Vec<_>>(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        json!({"snapshot":self.component_stamp(),"entries":entries,"error":error,
            "limits":{"partitions":map_core::partition_metadata::MAX_PARTITIONS,"text_bytes":map_core::partition_metadata::MAX_TEXT_BYTES}})
    }

    pub fn store_component_partition(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before keeping pieces, or Apply the extrusion".into());
        }
        let draft = self.checked_partition(token)?;
        if draft.rise != 0.0 {
            return Err(
                "Return to flat before keeping a partition; Apply commits raised geometry".into(),
            );
        }
        let draft = draft.clone();
        let selection = self.components.selection.clone();
        let picking = self.components.picking.clone();
        let count = self.edit(actor, "keep flat face partitions", |tx| {
            draft.store(tx)?;
            Ok(draft.partitions.len())
        })?;
        if self.partition_preview_token() == Some(token) {
            self.cancel_component_move(token)?;
        }
        self.components.selection = selection;
        self.components.picking = picking;
        self.refresh_component_origin();
        Ok(count)
    }

    pub fn resume_component_partitions(
        &mut self,
        sources: &[(u32, u32)],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        if sources.len() > subobject::MAX_SOLIDS {
            return Err("Resume at most 16 saved partitions".into());
        }
        if self.components.preview.is_some() {
            return Err(
                "Apply or discard the current component preview before resuming saved pieces"
                    .into(),
            );
        }
        let sources = sources.iter().copied().collect::<BTreeSet<_>>();
        let solids = sources
            .iter()
            .map(|source| source.0)
            .collect::<BTreeSet<_>>();
        if sources.is_empty()
            || sources.len() > subobject::MAX_SOLIDS
            || sources.len() != solids.len()
        {
            return Err("Resume one saved partition per brush across 1–16 brushes".into());
        }
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        let mut resolved = metadata
            .resolve(&self.document)
            .into_iter()
            .map(|entry| ((entry.source.solid, entry.source.side), entry.result))
            .collect::<BTreeMap<_, _>>();
        let mut partitions = Vec::new();
        for source in &sources {
            partitions.push(
                resolved
                    .remove(source)
                    .ok_or("Saved partition does not exist")??,
            );
        }
        let materials = materials::saved_materials(&metadata, &partitions);
        let textures = textures::saved_textures(&self.document, &metadata, &partitions);
        let ids = solids.into_iter().collect::<Vec<_>>();
        self.set_component_picking(
            Some(super::super::component_picking::Kind::Face),
            &ids,
            stamp,
        )?;
        let selection = sources
            .into_iter()
            .map(|(solid, side)| Component {
                solid,
                sides: vec![side],
            })
            .collect::<Vec<_>>();
        self.select_components(&selection, SelectMode::Replace, &self.component_stamp())?;
        let selection = partitions
            .iter()
            .flat_map(|part| {
                part.loops.iter().map(|piece| PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            })
            .collect();
        Ok(self.install_partition_preview(
            Draft {
                partitions,
                rise: 0.0,
                roof_texture_lock: false,
                selection,
                materials,
                textures,
            },
            self.document.preview_snapshot(),
            None,
        ))
    }

    pub fn remove_saved_component_partitions(
        &mut self,
        actor: Actor,
        sources: &[(u32, u32)],
        stamp: &str,
    ) -> Result<usize, String> {
        self.check_component_stamp(stamp)?;
        if sources.len() > map_core::partition_metadata::MAX_PARTITIONS {
            return Err("Remove at most 64 saved partitions".into());
        }
        if self.components.preview.is_some() {
            return Err(
                "Apply or discard the current component preview before removing saved pieces"
                    .into(),
            );
        }
        let sources = sources.iter().copied().collect::<BTreeSet<_>>();
        if sources.is_empty() || sources.len() > map_core::partition_metadata::MAX_PARTITIONS {
            return Err("Choose 1–64 saved partitions to remove".into());
        }
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        if sources
            .iter()
            .any(|source| !metadata.entries.contains_key(source))
        {
            return Err("Saved partition does not exist".into());
        }
        self.edit(actor, "remove saved face partitions", |tx| {
            for (solid, side) in &sources {
                map_core::partition_metadata::remove(tx, *solid, *side)?;
            }
            Ok(sources.len())
        })
    }

    pub fn partition_preview_token(&self) -> Option<u64> {
        self.components
            .preview
            .as_ref()
            .filter(|preview| preview.partition.is_some())
            .map(|preview| preview.token)
    }

    pub fn ensure_partitions_resolved(&self) -> Result<(), String> {
        if self.partition_preview_token().is_some() {
            return Err("A temporary face partition is pending. Keep flat pieces, raise and Apply, or Cancel before saving, compiling or playing.".into());
        }
        Ok(())
    }

    pub(super) fn checked_partition(&self, token: u64) -> Result<&Draft, String> {
        self.checked_component_preview(token)?
            .partition
            .as_ref()
            .ok_or("No flat partition draft to edit".into())
    }

    fn install_partition_preview(
        &mut self,
        draft: Draft,
        document: Document,
        plan: Option<subobject::CreasePlan>,
    ) -> u64 {
        let requests = draft.requests();
        let selection = plan.as_ref().map_or_else(
            || self.components.selection.clone(),
            |plan| {
                plan.changes
                    .iter()
                    .flat_map(|change| change.selection.iter().cloned())
                    .collect()
            },
        );
        let (replacements, crease) = match plan {
            Some(plan) => (
                plan.replacements,
                Some(CreaseDraft {
                    rise: draft.rise,
                    roof_texture_lock: draft.roof_texture_lock,
                    requests,
                    changes: plan.changes,
                }),
            ),
            None => (Vec::new(), None),
        };
        self.install_component_preview(Preview {
            replacements,
            crease,
            partition: Some(draft),
            ..Preview::new(document, selection)
        })
    }

    pub fn preview_component_partition(
        &mut self,
        pairs: &[CreaseEdges],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let requests = self.crease_requests(pairs)?;
        let mut partitions = subobject::plan_partitions(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        let materials = if let Ok(metadata) =
            map_core::partition_metadata::Metadata::parse(self.document.vmf())
        {
            let mut materials = BTreeMap::new();
            for resolved in metadata.resolve(&self.document) {
                if let Ok(saved) = resolved.result
                    && let Some(current) = partitions
                        .iter_mut()
                        .find(|part| part.source == saved.source)
                {
                    materials.extend(materials::saved_materials(
                        &metadata,
                        std::slice::from_ref(&saved),
                    ));
                    current.loops = saved.loops;
                }
            }
            materials
        } else {
            BTreeMap::new()
        };
        let textures = map_core::partition_metadata::Metadata::parse(self.document.vmf())
            .map(|metadata| textures::saved_textures(&self.document, &metadata, &partitions))
            .unwrap_or_default();
        let selection = partitions
            .iter()
            .flat_map(|part| {
                part.loops.iter().map(|piece| PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            })
            .collect();
        let draft = Draft {
            partitions,
            rise: 0.0,
            roof_texture_lock: false,
            selection,
            materials,
            textures,
        };
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }

    pub fn update_component_partition_cut(
        &mut self,
        token: u64,
        edge_parameters: [f64; 2],
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 || self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before adjusting the dividing edge".into());
        }
        let requests = draft
            .partitions
            .iter()
            .map(|part| subobject::PartitionCut {
                source: part.source.clone(),
                edge_parameters,
            })
            .collect::<Vec<_>>();
        let mut partitions = subobject::plan_partitions_at(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        for (part, previous) in partitions.iter_mut().zip(&draft.partitions) {
            part.loops = previous.loops.clone();
        }
        draft.partitions = partitions;
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }

    pub fn raise_component_partition(&mut self, token: u64, rise: f64) -> Result<u64, String> {
        self.raise_component_partition_with_texture(token, rise, None)
    }

    pub fn raise_component_partition_with_texture(
        &mut self,
        token: u64,
        rise: f64,
        roof_texture_lock: Option<bool>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        draft.roof_texture_lock = roof_texture_lock.unwrap_or(draft.roof_texture_lock);
        if !rise.is_finite()
            || (rise != 0.0
                && !(subobject::MIN_SPLIT_RISE..=subobject::MAX_SPLIT_RISE).contains(&rise))
        {
            return Err("Partition ridge height must be zero (flat) or 0.125–4096 units".into());
        }
        let requests = draft.requests();
        let cuts = draft
            .partitions
            .iter()
            .map(|part| part.cut())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let current = subobject::plan_partitions_at(&self.document, &cuts)
            .map_err(|error| error.to_string())?;
        if current
            .iter()
            .zip(&draft.partitions)
            .any(|(current, saved)| {
                let mut canonical = saved.clone();
                canonical.loops = current.loops.clone();
                current != &canonical
            })
        {
            return Err(
                "Partition source geometry or ownership changed; stage the face again".into(),
            );
        }
        let mut document = self.document.preview_snapshot();
        let plan = if rise == 0.0 {
            None
        } else {
            Some(
                document
                    .edit("preview raised partition", |tx| {
                        let plan = subobject::crease_partitions_with_surfaces(
                            tx,
                            &draft.partitions,
                            rise,
                            draft.roof_texture_lock,
                            &draft.material_plan(),
                            &draft.texture_plan(),
                        )?;
                        consume_saved(tx, &requests)?;
                        Ok(plan)
                    })
                    .map_err(|error| error.to_string())?,
            )
        };
        draft.rise = rise;
        Ok(self.install_partition_preview(draft, document, plan))
    }

    pub fn select_component_partitions(
        &mut self,
        token: u64,
        parts: &[PartitionRef],
        mode: SelectMode,
    ) -> Result<(), String> {
        let draft = self.checked_partition(token)?;
        if parts.len() > subobject::MAX_SOLIDS * 2
            || parts.iter().any(|part| {
                !draft.partitions.iter().any(|source| {
                    source.source.solid == part.solid
                        && source.loops.iter().any(|piece| piece.id == part.partition)
                })
            })
        {
            return Err("Choose loops from the current partition draft".into());
        }
        let mut selection = if mode == SelectMode::Replace {
            BTreeSet::new()
        } else {
            draft.selection.clone()
        };
        for part in parts.iter().collect::<BTreeSet<_>>() {
            match mode {
                SelectMode::Replace | SelectMode::Add => {
                    selection.insert(part.clone());
                }
                SelectMode::Remove => {
                    selection.remove(part);
                }
                SelectMode::Toggle => {
                    if !selection.remove(part) {
                        selection.insert(part.clone());
                    }
                }
            }
        }
        self.components
            .preview
            .as_mut()
            .unwrap()
            .partition
            .as_mut()
            .unwrap()
            .selection = selection;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn frame_component_partitions(
        &mut self,
        actor: Actor,
        token: u64,
        aspect: f32,
    ) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let draft = self.checked_partition(token)?;
        let mut bounds: Option<(DVec3, DVec3)> = None;
        for part in &draft.partitions {
            let positions = part.positions(draft.rise);
            for piece in &part.loops {
                if draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                }) {
                    for &index in &piece.vertices {
                        let point = positions[index as usize];
                        bounds = Some(bounds.map_or((point, point), |(mins, maxs)| {
                            (mins.min(point), maxs.max(point))
                        }));
                        if let Some(extrusion) = &self.checked_component_preview(token)?.extrusion
                            && extrusion.selection.contains(&PartitionRef {
                                solid: part.source.solid,
                                partition: piece.id,
                            })
                        {
                            let top = point + part.normal() * extrusion.spec.height;
                            bounds = bounds.map(|(mins, maxs)| (mins.min(top), maxs.max(top)));
                        }
                    }
                }
            }
        }
        let (mins, maxs) = bounds.ok_or("Select a partition loop to frame")?;
        self.set_camera(
            self.camera
                .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect.max(0.01)),
        );
        self.log(actor, "framed face partitions");
        Ok(())
    }

    pub(super) fn append_partition_guides(&mut self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self.partition_surface_draft() else {
            return;
        };
        let visible = self.partition_piece_visibility(&draft);
        for part in &draft.partitions {
            let positions = part.positions(draft.rise);
            let mut pieces = part.loops.iter().collect::<Vec<_>>();
            pieces.sort_by_key(|piece| {
                draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            });
            for piece in pieces {
                if !visible
                    .get(&PartitionRef {
                        solid: part.source.solid,
                        partition: piece.id,
                    })
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let selected = draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                });
                let color = if selected {
                    [255, 190, 65, 255]
                } else {
                    [95, 215, 255, 255]
                };
                for index in 0..piece.vertices.len() {
                    for vertex in [
                        piece.vertices[index],
                        piece.vertices[(index + 1) % piece.vertices.len()],
                    ] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: (positions[vertex as usize] + part.normal() * 0.02)
                                .as_vec3()
                                .to_array(),
                            color,
                        });
                    }
                    if let Some(extrusion) = self
                        .components
                        .preview
                        .as_ref()
                        .and_then(|preview| preview.extrusion.as_ref())
                        && extrusion.selection.contains(&PartitionRef {
                            solid: part.source.solid,
                            partition: piece.id,
                        })
                    {
                        let base = positions[piece.vertices[index] as usize];
                        let next =
                            positions[piece.vertices[(index + 1) % piece.vertices.len()] as usize];
                        let delta = part.normal() * extrusion.spec.height;
                        for point in [base + delta, next + delta, base, base + delta] {
                            mesh.lines.push(render::scene::LineVertex {
                                position: point.as_vec3().to_array(),
                                color,
                            });
                        }
                    }
                }
            }
        }
    }
}
