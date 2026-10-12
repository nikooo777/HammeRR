use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::subobject::{self, Component, Replacement, SplitChange, Topology, WeldChange};
use map_core::{Document, EditError, ObjectId};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub token: u64,
    pub(super) delta: DVec3,
    pub(super) texture_lock: bool,
    replacements: Vec<Replacement>,
    generation: u64,
    revision: u64,
    pub(super) weld: Option<Vec<WeldChange>>,
    split: Option<SplitDraft>,
    selection: BTreeSet<Component>,
}

struct SplitDraft {
    rise: f64,
    changes: Vec<SplitChange>,
}

#[derive(Default)]
pub struct State {
    pub selection: BTreeSet<Component>,
    pub preview: Option<Preview>,
    pub(super) serial: u64,
    pub picking: Option<super::component_picking::Picking>,
    pub drag: Option<super::component_drag::Drag>,
    pub options: super::component_drag::Options,
    pub(super) origin: Option<DVec3>,
}

pub fn component_json(component: &Component) -> Value {
    json!({"solid": component.solid, "sides": component.sides})
}

impl Session {
    pub fn component_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.components.serial
        )
    }

    pub(super) fn check_component_stamp(&self, stamp: &str) -> Result<(), String> {
        if stamp != self.component_stamp() {
            return Err("Map, selection or preview changed; inspect brush components again".into());
        }
        Ok(())
    }

    pub fn component_selection(&self) -> &BTreeSet<Component> {
        &self.components.selection
    }

    pub fn displayed_component_selection(&self) -> &BTreeSet<Component> {
        self.components
            .preview
            .as_ref()
            .map_or(&self.components.selection, |preview| &preview.selection)
    }

    pub fn component_topology_preview(&self) -> bool {
        self.components
            .preview
            .as_ref()
            .is_some_and(|preview| preview.weld.is_some() || preview.split.is_some())
    }

    pub fn clear_component_edit(&mut self) {
        if !self.components.selection.is_empty()
            || self.components.preview.is_some()
            || self.components.picking.is_some()
            || self.components.drag.is_some()
        {
            self.components.drag = None;
            self.components.origin = None;
            self.components.picking = None;
            self.components.selection.clear();
            self.components.preview = None;
            self.components.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn component_solids(&self) -> Vec<u32> {
        if let Some(picking) = &self.components.picking {
            return picking.solids.iter().copied().collect();
        }
        self.selected_ids()
            .iter()
            .flat_map(|id| match id {
                ObjectId::Solid(id) => vec![*id],
                ObjectId::Entity(id) => self.document.entity(*id).map_or_else(Vec::new, |entity| {
                    entity.solids.iter().map(|solid| solid.id).collect()
                }),
            })
            .collect()
    }

    pub fn component_topology(&self, ids: &[u32]) -> Result<Vec<Topology>, String> {
        let ids = ids.iter().copied().collect::<BTreeSet<_>>();
        if ids.len() > subobject::MAX_SOLIDS {
            return Err("Inspect at most 16 brushes at a time".into());
        }
        let document = self.render_document();
        ids.iter()
            .map(|id| {
                let (_, solid) =
                    subobject::solid(document, *id).map_err(|error| error.to_string())?;
                subobject::topology(solid).map_err(|error| error.to_string())
            })
            .collect()
    }

    pub fn component_report(&self, ids: &[u32]) -> Result<Value, String> {
        let topology = self.component_topology(ids)?;
        let brushes = topology.iter().map(|shape| {
            let selected = |sides: &[u32]| self.displayed_component_selection().contains(&Component { solid: shape.solid, sides: sides.to_vec() });
            json!({
                "solid":shape.solid,
                "vertices":shape.vertices.iter().map(|vertex| json!({"sides":vertex.sides,"position":vertex.position.to_array(),"selected":selected(&vertex.sides)})).collect::<Vec<_>>(),
                "edges":shape.edges.iter().map(|edge| json!({"sides":edge.sides,"vertices":edge.vertices,"positions":edge.vertices.map(|index|shape.vertices[index].position.to_array()),"selected":selected(&edge.sides)})).collect::<Vec<_>>(),
                "faces":shape.faces.iter().map(|face|json!({"side":face.side,"vertices":face.vertices,"selected":selected(&[face.side])})).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>();
        Ok(json!({
            "move_options":self.components.options, "drag":self.component_drag_report(),
            "picking":self.components.picking, "snapshot":self.component_stamp(), "generation":self.document_generation, "revision":self.document.revision(),
            "brushes":brushes, "selection":self.displayed_component_selection().iter().map(component_json).collect::<Vec<_>>(),
            "source_selection":self.components.selection.iter().map(component_json).collect::<Vec<_>>(),
            "preview":self.components.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"operation":if preview.weld.is_some() {"weld"} else if preview.split.is_some() {"split"} else {"move"},"delta":preview.delta.to_array(),"texture_lock":preview.texture_lock,"brushes":preview.replacements.len(),
                "weld":preview.weld.as_ref().map(|changes|changes.iter().map(|change|json!({"solid":change.solid,"position":change.position.to_array(),"merged_vertices":change.merged_vertices,"removed_sides":change.removed_sides,"selection":component_json(&change.selection)})).collect::<Vec<_>>()),
                "split":preview.split.as_ref().map(|split|json!({"rise":split.rise,"changes":split.changes.iter().map(|change|json!({"solid":change.solid,"position":change.position.to_array(),"source_sides":change.source_sides,"new_sides":change.new_sides,"faces":change.faces.iter().map(|face|json!({"side":face.side,"source":face.source})).collect::<Vec<_>>(),"selection":component_json(&change.selection)})).collect::<Vec<_>>()}))})),
            "texture_mode":if self.components.preview.as_ref().is_some_and(|preview|preview.texture_lock) {"attached"} else {"world"}, "limits":{"brushes":subobject::MAX_SOLIDS,"sides_per_brush":subobject::MAX_SIDES,"selected_components":subobject::MAX_COMPONENTS,"split_rise":[subobject::MIN_SPLIT_RISE,subobject::MAX_SPLIT_RISE]},
        }))
    }

    pub fn select_components(
        &mut self,
        components: &[Component],
        mode: SelectMode,
        stamp: &str,
    ) -> Result<(), String> {
        self.check_component_stamp(stamp)?;
        if self.component_topology_preview() {
            return Err(
                "Apply or cancel the topology preview before changing component selection".into(),
            );
        }
        if components.len() > subobject::MAX_COMPONENTS {
            return Err("Select at most 512 faces, vertices or edges".into());
        }
        let mut shapes = BTreeMap::new();
        let mut owners = BTreeSet::new();
        let mut incoming = Vec::new();
        for component in components {
            if shapes.len() >= subobject::MAX_SOLIDS && !shapes.contains_key(&component.solid) {
                return Err("Select components in at most 16 brushes".into());
            }
            let shape = match shapes.entry(component.solid) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let (_, solid) = subobject::solid(&self.document, component.solid)
                        .map_err(|error| error.to_string())?;
                    entry.insert(subobject::topology(solid).map_err(|error| error.to_string())?)
                }
            };
            shape
                .resolve(&component.sides)
                .map_err(|error| error.to_string())?;
            let mut component = component.clone();
            component.sides.sort_unstable();
            incoming.push(component);
        }
        let mut next = self.components.selection.clone();
        match mode {
            SelectMode::Replace => next = incoming.into_iter().collect(),
            SelectMode::Add => next.extend(incoming),
            SelectMode::Remove => incoming.iter().for_each(|component| {
                next.remove(component);
            }),
            SelectMode::Toggle => incoming.into_iter().for_each(|component| {
                if !next.remove(&component) {
                    next.insert(component);
                }
            }),
        }
        if next.len() > subobject::MAX_COMPONENTS
            || next
                .iter()
                .map(|component| component.solid)
                .collect::<BTreeSet<_>>()
                .len()
                > subobject::MAX_SOLIDS
        {
            return Err("Component selection exceeds 512 components or 16 brushes".into());
        }
        for component in &next {
            owners.insert(
                subobject::solid(&self.document, component.solid)
                    .map_err(|error| error.to_string())?
                    .0,
            );
        }
        let picking = self.components.picking.clone().filter(|picking| {
            next.iter()
                .all(|component| picking.solids.contains(&component.solid))
        });
        if !owners.is_empty() {
            self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        }
        self.cancel_block_preview();
        self.cancel_face_preview();
        self.cancel_clip_preview();
        self.clear_hollow_preview();
        self.clear_carve_preview();
        self.clear_primitive_preview();
        self.clear_volume_selection();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clear_paint_stroke();
        self.clip_tool = false;
        self.paint.enabled = false;
        self.spatial.draw_block = false;
        self.components.picking = picking;
        self.components.selection = next;
        self.components.preview = None;
        self.components.drag = None;
        self.refresh_component_origin();
        self.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn preview_component_move(
        &mut self,
        delta: DVec3,
        snap: bool,
        texture_lock: bool,
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let delta = if snap {
            self.spatial.snap_point(delta)
        } else {
            delta
        };
        let components = self
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let replacements = subobject::plan_move_with_texture_lock(
            &self.document,
            &components,
            delta,
            texture_lock,
        )
        .map_err(|error| error.to_string())?;
        let mut document = self.document.preview_snapshot();
        document
            .edit("preview component move", |tx| {
                subobject::apply(tx, &replacements)
            })
            .map_err(|error| error.to_string())?;
        self.components.serial += 1;
        let token = self.components.serial;
        self.components.drag = None;
        self.components.options.snap = snap;
        self.components.options.texture_lock = texture_lock;
        self.components.preview = Some(Preview {
            document,
            token,
            delta,
            texture_lock,
            replacements,
            generation: self.document_generation,
            revision: self.document.revision(),
            weld: None,
            split: None,
            selection: self.components.selection.clone(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(token)
    }

    pub fn preview_component_weld(&mut self, stamp: &str) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let components = self
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let plan =
            subobject::plan_weld(&self.document, &components).map_err(|error| error.to_string())?;
        let mut document = self.document.preview_snapshot();
        document
            .edit("preview corner weld", |tx| {
                subobject::apply(tx, &plan.replacements)
            })
            .map_err(|error| error.to_string())?;
        self.components.serial += 1;
        let token = self.components.serial;
        self.components.drag = None;
        self.components.preview = Some(Preview {
            document,
            token,
            delta: DVec3::ZERO,
            texture_lock: false,
            replacements: plan.replacements,
            generation: self.document_generation,
            revision: self.document.revision(),
            selection: plan
                .changes
                .iter()
                .map(|change| change.selection.clone())
                .collect(),
            weld: Some(plan.changes),
            split: None,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(token)
    }

    pub fn preview_component_split(&mut self, rise: f64, stamp: &str) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let components = self
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut document = self.document.preview_snapshot();
        let plan = document
            .edit("preview component split", |tx| {
                subobject::split(tx, &components, rise)
            })
            .map_err(|error| error.to_string())?;
        self.components.serial += 1;
        let token = self.components.serial;
        self.components.drag = None;
        self.components.preview = Some(Preview {
            document,
            token,
            delta: DVec3::ZERO,
            texture_lock: false,
            replacements: plan.replacements,
            generation: self.document_generation,
            revision: self.document.revision(),
            weld: None,
            selection: plan
                .changes
                .iter()
                .map(|change| change.selection.clone())
                .collect(),
            split: Some(SplitDraft {
                rise,
                changes: plan.changes,
            }),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(token)
    }

    fn checked_component_preview(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .components
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Component preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the component move again".into());
        }
        Ok(preview)
    }

    pub fn commit_component_move(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self.checked_component_preview(token)?.weld.is_some()
            || self.checked_component_preview(token)?.split.is_some()
        {
            return Err("Use Apply component edit for a topology change".into());
        }
        self.commit_component_edit(actor, token)
    }

    pub fn commit_component_edit(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self
            .components
            .drag
            .as_ref()
            .is_some_and(|drag| drag.error.is_some())
        {
            return Err(
                "The last handle update was invalid; move to a valid position or cancel".into(),
            );
        }
        let preview = self.checked_component_preview(token)?;
        let label = if preview.weld.is_some() {
            "weld brush corners"
        } else if preview.split.is_some() {
            "split brush component"
        } else {
            "move brush components"
        };
        let replacements = preview.replacements.clone();
        let selection = preview.selection.clone();
        let rise = preview.split.as_ref().map(|split| split.rise);
        let components = self
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let picking = self.components.picking.clone().map(|mut picking| {
            if rise.is_some() {
                picking.kind = super::component_picking::Kind::Vertex;
            }
            picking
        });
        let result = self.edit(actor, label, |tx| {
            if let Some(rise) = rise {
                let actual = subobject::split(tx, &components, rise)?;
                if actual.replacements != replacements {
                    return Err(EditError::Invalid(
                        "Face allocation changed; preview the split again".into(),
                    ));
                }
                Ok(actual.replacements.len())
            } else {
                subobject::apply(tx, &replacements)
            }
        })?;
        self.components.selection = selection;
        self.components.picking = picking;
        self.refresh_component_origin();
        Ok(result)
    }

    pub fn cancel_component_move(&mut self, token: u64) -> Result<(), String> {
        self.checked_component_preview(token)?;
        self.components.preview = None;
        self.components.drag = None;
        self.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn append_component_guides(&mut self, mesh: &mut render::SceneMesh) {
        let hidden = self.hidden_tool_brushes();
        let ids = if self.components.picking.is_some() {
            self.component_solids()
        } else {
            self.components
                .selection
                .iter()
                .map(|component| component.solid)
                .collect::<Vec<_>>()
        };
        let Ok(shapes) = self.component_topology(&ids) else {
            return;
        };
        for shape in shapes {
            if self.component_hidden(shape.solid, &hidden) {
                continue;
            }
            let mut selected_vertices = BTreeSet::new();
            for component in self
                .displayed_component_selection()
                .iter()
                .filter(|component| component.solid == shape.solid)
            {
                if let Ok(indices) = shape.resolve(&component.sides) {
                    selected_vertices.extend(indices);
                }
            }
            let radius = shape
                .edges
                .iter()
                .map(|edge| {
                    shape.vertices[edge.vertices[0]]
                        .position
                        .distance(shape.vertices[edge.vertices[1]].position)
                })
                .fold(24.0, f64::min)
                / 8.0;
            for (index, vertex) in shape.vertices.iter().enumerate() {
                let color = if selected_vertices.contains(&index) {
                    [255, 190, 65, 255]
                } else {
                    [95, 215, 255, 255]
                };
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    for point in [
                        vertex.position - axis * radius,
                        vertex.position + axis * radius,
                    ] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: point.as_vec3().to_array(),
                            color,
                        });
                    }
                }
            }
            for edge in &shape.edges {
                let selected = self.displayed_component_selection().contains(&Component {
                    solid: shape.solid,
                    sides: edge.sides.clone(),
                }) || edge.sides.iter().any(|side| {
                    self.displayed_component_selection().contains(&Component {
                        solid: shape.solid,
                        sides: vec![*side],
                    })
                });
                if selected || self.components.picking.is_some() {
                    for index in edge.vertices {
                        mesh.lines.push(render::scene::LineVertex {
                            position: shape.vertices[index].position.as_vec3().to_array(),
                            color: if selected {
                                [255, 190, 65, 255]
                            } else {
                                [95, 215, 255, 255]
                            },
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Session, u32, Component) {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        let edge = shape
            .edges
            .iter()
            .find(|edge| {
                edge.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap();
        let component = Component {
            solid: id,
            sides: edge.sides.clone(),
        };
        session
            .select_components(
                std::slice::from_ref(&component),
                SelectMode::Replace,
                &session.component_stamp(),
            )
            .unwrap();
        (session, id, component)
    }

    fn select_top_corners(session: &mut Session, id: u32) -> Vec<Component> {
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        let corners = shape
            .vertices
            .iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        session
            .select_components(&corners, SelectMode::Replace, &session.component_stamp())
            .unwrap();
        corners
    }

    #[test]
    fn component_weld_shares_atomic_preview_cancel_apply_and_result_selection() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, id, _) = fixture();
            let corners = select_top_corners(&mut session, id);
            let original = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let stamp = session.component_stamp();
            let first = session.preview_component_weld(&stamp).unwrap();
            let draft = session.render_document().vmf().clone();
            assert_eq!(session.document.vmf(), &original);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(session.component_guide().is_none());
            let report = session.component_report(&[id]).unwrap();
            assert_eq!(report["preview"]["operation"], "weld");
            assert_eq!(
                report["brushes"][0]["vertices"].as_array().unwrap().len(),
                5
            );
            assert_eq!(report["source_selection"].as_array().unwrap().len(), 4);
            assert_eq!(report["selection"].as_array().unwrap().len(), 1);
            assert!(session.preview_component_weld(&stamp).is_err());
            assert!(
                session
                    .select_components(&[], SelectMode::Replace, &session.component_stamp())
                    .is_err()
            );
            assert!(session.commit_component_move(actor, first).is_err());
            session.cancel_component_move(first).unwrap();
            assert_eq!(session.render_document().vmf(), &original);
            assert_eq!(
                session.component_selection(),
                &corners.into_iter().collect()
            );
            let token = session
                .preview_component_weld(&session.component_stamp())
                .unwrap();
            assert!(session.commit_component_edit(actor, first).is_err());
            assert!(session.cancel_component_move(first).is_err());
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &draft);
            assert_eq!(session.component_selection().len(), 1);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert!(session.component_guide().is_some());
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &original);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &draft);
        }
    }

    #[test]
    fn rejected_weld_preserves_move_preview_and_source_edits_invalidate_weld() {
        let (mut session, id, _) = fixture();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &session.component_stamp())
            .unwrap();
        let draft = session.render_document().vmf().clone();
        assert!(
            session
                .preview_component_weld(&session.component_stamp())
                .is_err()
        );
        assert_eq!(session.components.preview.as_ref().unwrap().token, token);
        assert_eq!(session.render_document().vmf(), &draft);
        session.cancel_component_move(token).unwrap();
        select_top_corners(&mut session, id);
        let token = session
            .preview_component_weld(&session.component_stamp())
            .unwrap();
        session
            .create_block(
                Actor::Agent,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "brick/test",
                false,
            )
            .unwrap();
        assert!(session.commit_component_edit(Actor::Agent, token).is_err());
        assert!(session.cancel_component_move(token).is_err());
        assert!(session.components.preview.is_none());
    }

    #[test]
    fn component_split_shares_preview_allocation_cancel_apply_and_scope() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, id, component) = fixture();
            session
                .set_component_picking(
                    Some(super::super::component_picking::Kind::Edge),
                    &[id],
                    &session.component_stamp(),
                )
                .unwrap();
            let source = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let first = session
                .preview_component_split(16.0, &session.component_stamp())
                .unwrap();
            let draft = session.render_document().vmf().clone();
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(session.component_guide().is_none());
            let report = session.component_report(&[id]).unwrap();
            assert_eq!(report["preview"]["operation"], "split");
            assert_eq!(
                report["preview"]["split"]["changes"][0]["new_sides"]
                    .as_array()
                    .unwrap()
                    .len(),
                4
            );
            assert_eq!(
                report["preview"]["split"]["changes"][0]["faces"]
                    .as_array()
                    .unwrap()
                    .len(),
                6
            );
            assert_eq!(
                report["brushes"][0]["vertices"].as_array().unwrap().len(),
                9
            );
            assert!(
                session
                    .preview_component_split(0.0, &session.component_stamp())
                    .is_err()
            );
            assert_eq!(session.render_document().vmf(), &draft);
            session.cancel_component_move(first).unwrap();
            assert_eq!(session.render_document().vmf(), &source);
            assert_eq!(session.component_selection(), &BTreeSet::from([component]));
            let token = session
                .preview_component_split(16.0, &session.component_stamp())
                .unwrap();
            assert_eq!(session.render_document().vmf(), &draft);
            assert!(session.commit_component_edit(actor, first).is_err());
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &draft);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.component_selection().len(), 1);
            assert_eq!(
                session.component_picking().unwrap().kind,
                super::super::component_picking::Kind::Vertex
            );
            assert_eq!(session.component_solids(), vec![id]);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &source);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &draft);
        }
    }

    #[test]
    fn split_commit_rejects_allocation_changes_without_recording_history() {
        let (mut session, _, _) = fixture();
        let token = session
            .preview_component_split(16.0, &session.component_stamp())
            .unwrap();
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        session
            .document
            .edit("reserve", |tx| tx.new_side_id())
            .unwrap();
        assert!(
            session
                .commit_component_edit(Actor::Agent, token)
                .unwrap_err()
                .contains("allocation")
        );
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.components.preview.as_ref().unwrap().token, token);
        let replacement = session
            .preview_component_split(16.0, &session.component_stamp())
            .unwrap();
        session
            .commit_component_edit(Actor::User, replacement)
            .unwrap();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session.document.vmf().validate_ids().unwrap();
    }

    #[test]
    fn component_preview_apply_cancel_and_undo_share_exact_geometry() {
        let (mut session, id, component) = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        let history = session.document.undo_labels().count();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &session.component_stamp())
            .unwrap();
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.document.undo_labels().count(), history);
        let preview = session.render_document().vmf().clone();
        assert_ne!(preview, original);
        let report = session.component_report(&[id]).unwrap();
        assert!(
            report["brushes"][0]["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|vertex| vertex["position"][2] == 80.0)
        );
        assert!(
            session
                .preview_component_move(DVec3::Z * -64.0, false, false, &session.component_stamp())
                .is_err()
        );
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.components.preview.as_ref().unwrap().token, token);
        session.commit_component_move(Actor::User, token).unwrap();
        assert_eq!(session.document.vmf(), &preview);
        assert_eq!(session.component_selection(), &BTreeSet::from([component]));
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        assert!(session.component_selection().is_empty());
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &preview);
    }

    #[test]
    fn texture_attachment_rejection_keeps_world_preview_and_locked_commit_is_exact() {
        let (mut session, id, _) = fixture();
        let original = session.document.vmf().clone();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &session.component_stamp())
            .unwrap();
        let draft = session.render_document().vmf().clone();
        assert!(
            session
                .preview_component_move(DVec3::Z * 16.0, false, true, &session.component_stamp())
                .is_err()
        );
        assert_eq!(session.render_document().vmf(), &draft);
        assert_eq!(session.components.preview.as_ref().unwrap().token, token);
        assert_eq!(
            session.component_report(&[]).unwrap()["texture_mode"],
            "world"
        );
        session.cancel_component_move(token).unwrap();
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        let top = shape
            .faces
            .iter()
            .find(|face| {
                face.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap();
        session
            .select_components(
                &[Component {
                    solid: id,
                    sides: vec![top.side],
                }],
                SelectMode::Replace,
                &session.component_stamp(),
            )
            .unwrap();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, true, &session.component_stamp())
            .unwrap();
        let draft = session.render_document().vmf().clone();
        assert_eq!(
            session.component_report(&[]).unwrap()["texture_mode"],
            "attached"
        );
        assert_eq!(session.document.vmf(), &original);
        session.commit_component_move(Actor::Agent, token).unwrap();
        assert_eq!(session.document.vmf(), &draft);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &original);
    }

    #[test]
    fn guards_prevent_stale_clients_from_replacing_or_cancelling_drafts() {
        let (mut session, _, component) = fixture();
        let stale = session.component_stamp();
        let first = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &stale)
            .unwrap();
        assert!(
            session
                .preview_component_move(DVec3::Z * 24.0, false, false, &stale)
                .is_err()
        );
        assert!(
            session
                .select_components(&[], SelectMode::Replace, &stale)
                .is_err()
        );
        let second = session
            .preview_component_move(DVec3::Z * 32.0, false, false, &session.component_stamp())
            .unwrap();
        assert!(session.cancel_component_move(first).is_err());
        assert!(session.commit_component_move(Actor::Agent, first).is_err());
        session.cancel_component_move(second).unwrap();
        assert!(session.components.preview.is_none());
        assert_eq!(
            session.component_selection(),
            &BTreeSet::from([component.clone()])
        );
        let third = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &session.component_stamp())
            .unwrap();
        session
            .select_components(&[component], SelectMode::Toggle, &session.component_stamp())
            .unwrap();
        assert!(session.commit_component_move(Actor::Agent, third).is_err());
        assert!(session.components.selection.is_empty());
    }

    #[test]
    fn snapping_source_changes_and_other_previews_invalidate_components() {
        let (mut session, _, _) = fixture();
        let token = session
            .preview_component_move(DVec3::Z * 19.0, true, false, &session.component_stamp())
            .unwrap();
        assert_eq!(
            session.components.preview.as_ref().unwrap().delta,
            DVec3::Z * 16.0
        );
        session
            .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
            .unwrap();
        assert!(session.commit_component_move(Actor::Agent, token).is_err());
        assert!(session.component_selection().is_empty());
        let (mut session, id, _) = fixture();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, false, &session.component_stamp())
            .unwrap();
        session
            .edit(Actor::Agent, "external edit", |tx| {
                tx.solid_mut(id)?.sides[0].material = "new/material".into();
                Ok(())
            })
            .unwrap();
        assert!(session.commit_component_move(Actor::Agent, token).is_err());
        assert!(session.components.preview.is_none());
    }
}
