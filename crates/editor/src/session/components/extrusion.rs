use super::*;
use map_core::subobject::{ExtrusionPlan, ExtrusionSpec, ExtrusionSurface};
use map_core::surface::FaceId;

pub(super) struct Draft {
    pub spec: ExtrusionSpec,
    pub selection: BTreeSet<PartitionRef>,
    pub plan: ExtrusionPlan,
}

impl Draft {
    pub fn report(&self) -> Value {
        json!({"height":self.spec.height,"wall_material":self.spec.wall_material,"texture_lock":self.spec.texture_lock,
            "selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "pieces":self.plan.pieces.iter().map(|piece|json!({"source_solid":piece.source.solid,"source_side":piece.source.side,"partition":piece.partition,"solid":piece.solid.id,
                "owner":match piece.owner {ObjectId::Solid(_)=>"world".to_string(),ObjectId::Entity(id)=>format!("entity:{id}")},
                "bounds":map_core::query::solid_bounds(&piece.solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()})),
                "faces":piece.faces.iter().map(|face| {
                    let side = piece.solid.sides.iter().find(|side|side.id==face.side).unwrap();
                    json!({"side":face.side,"source_side":face.source_side,"surface":match face.surface {ExtrusionSurface::Top=>"top",ExtrusionSurface::Base=>"base",ExtrusionSurface::Boundary=>"boundary",ExtrusionSurface::Divider=>"divider"},
                        "material":side.material.to_ascii_lowercase(),"plane":side.plane.0.map(|point|point.to_array()),"uaxis":{"axis":side.u_axis.axis.to_array(),"offset":side.u_axis.offset,"scale":side.u_axis.scale},"vaxis":{"axis":side.v_axis.axis.to_array(),"offset":side.v_axis.offset,"scale":side.v_axis.scale},"rotation":side.rotation,"lightmap_scale":side.lightmap_scale,"smoothing_groups":side.smoothing_groups})
                }).collect::<Vec<_>>() })).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn preview_partition_extrusion(
        &mut self,
        token: u64,
        spec: ExtrusionSpec,
    ) -> Result<u64, String> {
        let draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 {
            return Err("Return to flat before extruding highlighted pieces".into());
        }
        let selection = draft
            .selection
            .iter()
            .map(|part| (part.solid, part.partition))
            .collect::<Vec<_>>();
        let mut document = self.document.preview_snapshot();
        let plan = document
            .edit("preview partition extrusion", |tx| {
                subobject::extrude_partitions(tx, &draft.partitions, &selection, &spec)
            })
            .map_err(|error| error.to_string())?;
        Ok(self.install_component_preview(Preview {
            extrusion: Some(Draft {
                spec,
                selection: draft.selection.clone(),
                plan,
            }),
            partition: Some(draft),
            ..Preview::new(document, self.components.selection.clone())
        }))
    }

    pub(super) fn commit_partition_extrusion(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<usize, String> {
        let preview = self.checked_component_preview(token)?;
        let draft = preview.partition.as_ref().unwrap();
        let extrusion = preview.extrusion.as_ref().unwrap();
        if draft.selection != extrusion.selection {
            return Err(
                "Highlighted pieces changed; preview the extrusion again before Apply".into(),
            );
        }
        let partitions = draft.partitions.clone();
        let selection = extrusion
            .selection
            .iter()
            .map(|part| (part.solid, part.partition))
            .collect::<Vec<_>>();
        let spec = extrusion.spec.clone();
        let expected = extrusion.plan.clone();
        let faces = expected
            .pieces
            .iter()
            .flat_map(|piece| {
                piece.solid.sides.iter().map(|side| FaceId {
                    solid: piece.solid.id,
                    side: side.id,
                })
            })
            .collect::<Vec<_>>();
        let count = self.edit(actor, "extrude flat partition pieces", |tx| {
            let actual = subobject::extrude_partitions(tx, &partitions, &selection, &spec)?;
            if actual != expected {
                return Err(EditError::Invalid(
                    "Extrusion allocation changed; preview the pieces again".into(),
                ));
            }
            Ok(actual.pieces.len())
        })?;
        self.select_faces(&faces, SelectMode::Replace)?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::component_picking::Kind;

    fn fixture(actor: Actor, owned: bool, direction: usize) -> (Session, u32, u64) {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(actor, DVec3::ZERO, DVec3::splat(64.0), "brick/test", false)
            .unwrap()
        else {
            unreachable!()
        };
        if owned {
            session
                .edit(actor, "detail", |tx| {
                    map_core::edit::tie_to_entity(tx, &[id], "func_detail", &[])
                })
                .unwrap();
        }
        let (_, solid) = subobject::solid(session.document(), id).unwrap();
        let side = solid
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let pairs = vec![CreaseEdges {
            solid: id,
            edges: subobject::crease_pairs(solid, side).unwrap()[direction].edges,
        }];
        session
            .set_component_picking(Some(Kind::Face), &[id], &session.component_stamp())
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
        let token = session
            .preview_component_partition(&pairs, &session.component_stamp())
            .unwrap();
        session.store_component_partition(actor, token).unwrap();
        let token = session
            .resume_component_partitions(&[(id, side)], &session.component_stamp())
            .unwrap();
        (session, id, token)
    }

    fn spec(height: f64) -> ExtrusionSpec {
        ExtrusionSpec {
            height,
            wall_material: Some("concrete/test".into()),
            texture_lock: true,
        }
    }

    #[test]
    fn extrusion_shares_actor_ownership_saved_intent_selection_cancel_and_history() {
        for actor in [Actor::User, Actor::Agent] {
            for owned in [false, true] {
                for direction in 0..2 {
                    let (mut session, id, flat) = fixture(actor, owned, direction);
                    let source = session.document().vmf().clone();
                    let history = session.document().undo_labels().count();
                    let token = session
                        .preview_partition_extrusion(flat, spec(32.125))
                        .unwrap();
                    let preview = session.render_document().vmf().clone();
                    assert_eq!(
                        subobject::solid(session.render_document(), id).unwrap(),
                        subobject::solid(session.document(), id).unwrap()
                    );
                    assert_eq!(
                        session.component_report(&[id]).unwrap()["texture_mode"],
                        "top_attached"
                    );
                    assert!(session.ensure_partitions_resolved().is_err());
                    assert!(session.store_component_partition(actor, token).is_err());
                    assert_eq!(session.document().vmf(), &source);
                    assert_eq!(session.document().undo_labels().count(), history);
                    session
                        .frame_component_partitions(actor, token, 1.6)
                        .unwrap();
                    session.cancel_component_move(token).unwrap();
                    assert_eq!(session.render_document().vmf(), &source);
                    assert!(
                        session
                            .preview_partition_extrusion(token, spec(32.125))
                            .is_err()
                    );
                    let side = session.component_selection().iter().next().unwrap().sides[0];
                    let flat = session
                        .resume_component_partitions(&[(id, side)], &session.component_stamp())
                        .unwrap();
                    let token = session
                        .preview_partition_extrusion(flat, spec(32.125))
                        .unwrap();
                    assert_eq!(session.render_document().vmf(), &preview);
                    let created = session
                        .components
                        .preview
                        .as_ref()
                        .unwrap()
                        .extrusion
                        .as_ref()
                        .unwrap()
                        .plan
                        .pieces
                        .iter()
                        .map(|piece| piece.solid.id)
                        .collect::<BTreeSet<_>>();
                    assert_eq!(session.commit_component_edit(actor, token).unwrap(), 2);
                    assert_eq!(session.document().vmf(), &preview);
                    assert_eq!(session.document().undo_labels().count(), history + 1);
                    assert_eq!(session.selected_faces().len(), 12);
                    assert!(
                        session
                            .selected_faces()
                            .iter()
                            .all(|face| created.contains(&face.solid))
                    );
                    assert!(session.component_selection().is_empty());
                    assert_eq!(session.partition_preview_token(), None);
                    assert_eq!(
                        session.saved_partition_report()["entries"][0]["valid"],
                        true
                    );
                    session.undo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &source);
                    session.redo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &preview);
                }
            }
        }
    }

    #[test]
    fn extrusion_invalid_updates_and_changed_highlights_preserve_reviewed_draft() {
        let (mut session, id, flat) = fixture(Actor::Agent, false, 0);
        let source = session.document().vmf().clone();
        let token = session
            .preview_partition_extrusion(flat, spec(32.0))
            .unwrap();
        let preview = session.render_document().vmf().clone();
        for height in [f64::NAN, 0.0, -1.0, 4097.0] {
            assert!(
                session
                    .preview_partition_extrusion(token, spec(height))
                    .is_err()
            );
            assert_eq!(session.render_document().vmf(), &preview);
            assert_eq!(session.partition_preview_token(), Some(token));
        }
        session
            .select_component_partitions(
                token,
                &[PartitionRef {
                    solid: id,
                    partition: 0,
                }],
                SelectMode::Remove,
            )
            .unwrap();
        assert_eq!(
            session.component_report(&[id]).unwrap()["preview"]["can_apply"],
            false
        );
        assert!(
            session
                .commit_component_edit(Actor::Agent, token)
                .unwrap_err()
                .contains("Highlighted pieces changed")
        );
        assert_eq!(session.render_document().vmf(), &preview);
        let updated = session
            .preview_partition_extrusion(token, spec(48.0))
            .unwrap();
        assert!(
            session
                .preview_partition_extrusion(token, spec(32.0))
                .is_err()
        );
        assert_eq!(
            session
                .components
                .preview
                .as_ref()
                .unwrap()
                .extrusion
                .as_ref()
                .unwrap()
                .plan
                .pieces
                .len(),
            1
        );
        let flat = session.raise_component_partition(updated, 0.0).unwrap();
        assert_eq!(session.render_document().vmf(), &source);
        assert!(
            session
                .components
                .preview
                .as_ref()
                .unwrap()
                .extrusion
                .is_none()
        );
        session
            .select_component_partitions(flat, &[], SelectMode::Replace)
            .unwrap();
        assert!(
            session
                .preview_partition_extrusion(flat, spec(32.0))
                .is_err()
        );
        session.cancel_component_move(flat).unwrap();
        let (_, _, stale) = fixture(Actor::Agent, false, 0);
        session.new_map(Actor::Agent, true).unwrap();
        assert!(
            session
                .preview_partition_extrusion(stale, spec(32.0))
                .is_err()
        );
    }

    #[test]
    fn extrusion_maximum_batch_selects_new_faces_without_expanding_component_scope() {
        let mut session = Session::new(Err("no assets needed".into()));
        let mut components = Vec::new();
        let mut pairs = Vec::new();
        for index in 0..subobject::MAX_SOLIDS {
            let offset = DVec3::X * index as f64 * 128.0;
            let ObjectId::Solid(id) = session
                .create_block(
                    Actor::Agent,
                    offset,
                    offset + DVec3::splat(64.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let (_, solid) = subobject::solid(session.document(), id).unwrap();
            let side = solid
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            pairs.push(CreaseEdges {
                solid: id,
                edges: subobject::crease_pairs(solid, side).unwrap()[index % 2].edges,
            });
            components.push(Component {
                solid: id,
                sides: vec![side],
            });
        }
        session
            .select_components(&components, SelectMode::Replace, &session.component_stamp())
            .unwrap();
        let flat = session
            .preview_component_partition(&pairs, &session.component_stamp())
            .unwrap();
        let token = session
            .preview_partition_extrusion(flat, spec(32.0))
            .unwrap();
        assert_eq!(
            session.commit_component_edit(Actor::Agent, token).unwrap(),
            32
        );
        assert_eq!(session.document().vmf().world.solids.len(), 48);
        assert_eq!(session.selected_faces().len(), 192);
        assert!(session.component_solids().is_empty());
        assert!(session.component_report(&[]).is_ok());
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document().vmf().world.solids.len(), 16);
    }

    #[test]
    fn extrusion_body_picking_maps_back_to_the_source_piece() {
        let (mut session, id, flat) = fixture(Actor::Agent, false, 0);
        session
            .select_component_partitions(
                flat,
                &[PartitionRef {
                    solid: id,
                    partition: 0,
                }],
                SelectMode::Replace,
            )
            .unwrap();
        let token = session
            .preview_partition_extrusion(flat, spec(32.0))
            .unwrap();
        let piece = &session
            .components
            .preview
            .as_ref()
            .unwrap()
            .extrusion
            .as_ref()
            .unwrap()
            .plan
            .pieces[0];
        let (mins, maxs) = map_core::query::solid_bounds(&piece.solid).unwrap();
        let center = (mins + maxs) * 0.5;
        let camera =
            render::Camera::looking_at((center + DVec3::Z * 200.0).as_vec3(), center.as_vec3());
        let picks = session
            .pick_component_partitions(
                token,
                camera,
                crate::session::Pointer {
                    position: glam::DVec2::new(400.0, 300.0),
                    size: (800, 600),
                },
            )
            .unwrap();
        assert_eq!(picks.len(), 1);
        assert_eq!(
            picks[0].partition,
            PartitionRef {
                solid: id,
                partition: 0
            }
        );
        assert!((picks[0].position.z - maxs.z).abs() < 0.01);
    }
}
