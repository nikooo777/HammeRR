use map_core::subobject::{Partition, VertexSource};

use super::*;

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

    fn fixture(actor: Actor, direction: usize) -> (Session, u32, Vec<CreaseEdges>) {
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
    pub selection: BTreeSet<PartitionRef>,
}

impl Draft {
    fn requests(&self) -> Vec<Crease> {
        self.partitions
            .iter()
            .map(|partition| partition.source.clone())
            .collect()
    }

    pub fn report(&self) -> Value {
        json!({"rise":self.rise,"temporary":true,"selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "partitions":self.partitions.iter().map(|part| {
                let positions = part.positions(self.rise);
                let owner = match part.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")};
                json!({"solid":part.source.solid,"source_side":part.source.side,"edges":part.source.edges,"owner":owner,"plane":part.plane.map(|point|point.to_array()),
                    "vertices":part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"preview_position":positions[vertex.id as usize].to_array(),"source":match &vertex.source {VertexSource::Corner(sides)=>json!({"kind":"corner","sides":sides}),VertexSource::Edge{corners,parameter}=>json!({"kind":"edge","corners":corners,"parameter":parameter})}})).collect::<Vec<_>>(),
                    "loops":part.loops.iter().map(|piece|json!({"id":piece.id,"vertices":piece.vertices,"source_side":piece.source_side,"selected":self.selection.contains(&PartitionRef{solid:part.source.solid,partition:piece.id})})).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn partition_preview_token(&self) -> Option<u64> {
        self.components
            .preview
            .as_ref()
            .filter(|preview| preview.partition.is_some())
            .map(|preview| preview.token)
    }

    pub fn ensure_partitions_resolved(&self) -> Result<(), String> {
        if self.partition_preview_token().is_some() {
            return Err("A temporary face partition is pending. Raise and Apply it, or Cancel it before saving, compiling or playing.".into());
        }
        Ok(())
    }

    fn checked_partition(&self, token: u64) -> Result<&Draft, String> {
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
                    requests,
                    changes: plan.changes,
                }),
            ),
            None => (Vec::new(), None),
        };
        self.components.serial += 1;
        let token = self.components.serial;
        self.components.drag = None;
        self.components.preview = Some(Preview {
            document,
            token,
            delta: DVec3::ZERO,
            texture_lock: false,
            replacements,
            generation: self.document_generation,
            revision: self.document.revision(),
            weld: None,
            weld_target: None,
            split: None,
            crease,
            transform: None,
            partition: Some(draft),
            selection,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        token
    }

    pub fn preview_component_partition(
        &mut self,
        pairs: &[CreaseEdges],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let requests = self.crease_requests(pairs)?;
        let partitions = subobject::plan_partitions(&self.document, &requests)
            .map_err(|error| error.to_string())?;
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
            selection,
        };
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }

    pub fn raise_component_partition(&mut self, token: u64, rise: f64) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if !rise.is_finite()
            || (rise != 0.0
                && !(subobject::MIN_SPLIT_RISE..=subobject::MAX_SPLIT_RISE).contains(&rise))
        {
            return Err("Partition ridge height must be zero (flat) or 0.125–4096 units".into());
        }
        let requests = draft.requests();
        let current = subobject::plan_partitions(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        if current != draft.partitions {
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
                        subobject::crease(tx, &requests, rise)
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
                    for index in piece.vertices {
                        let point = positions[index as usize];
                        bounds = Some(bounds.map_or((point, point), |(mins, maxs)| {
                            (mins.min(point), maxs.max(point))
                        }));
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
        let hidden = self.hidden_tool_brushes();
        let Some(draft) = self
            .components
            .preview
            .as_ref()
            .and_then(|preview| preview.partition.as_ref())
        else {
            return;
        };
        for part in &draft.partitions {
            if self.component_hidden(part.source.solid, &hidden) {
                continue;
            }
            let positions = part.positions(draft.rise);
            let mut pieces = part.loops.iter().collect::<Vec<_>>();
            pieces.sort_by_key(|piece| {
                draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            });
            for piece in pieces {
                let selected = draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                });
                let color = if selected {
                    [255, 190, 65, 255]
                } else {
                    [95, 215, 255, 255]
                };
                for index in 0..4 {
                    for vertex in [piece.vertices[index], piece.vertices[(index + 1) % 4]] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: (positions[vertex as usize] + part.normal() * 0.02)
                                .as_vec3()
                                .to_array(),
                            color,
                        });
                    }
                }
            }
        }
    }
}
