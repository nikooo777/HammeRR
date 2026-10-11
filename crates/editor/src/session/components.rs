use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::subobject::{self, Component, Replacement, Topology};
use map_core::{Document, ObjectId};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub token: u64,
    delta: DVec3,
    replacements: Vec<Replacement>,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub selection: BTreeSet<Component>,
    pub preview: Option<Preview>,
    serial: u64,
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

    fn check_component_stamp(&self, stamp: &str) -> Result<(), String> {
        if stamp != self.component_stamp() {
            return Err("Map, selection or preview changed; inspect brush components again".into());
        }
        Ok(())
    }

    pub fn component_selection(&self) -> &BTreeSet<Component> {
        &self.components.selection
    }

    pub fn clear_component_edit(&mut self) {
        if !self.components.selection.is_empty() || self.components.preview.is_some() {
            self.components.selection.clear();
            self.components.preview = None;
            self.components.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn component_solids(&self) -> Vec<u32> {
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
        let document = self
            .components
            .preview
            .as_ref()
            .map_or(&self.document, |preview| &preview.document);
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
            let selected = |sides: &[u32]| self.components.selection.contains(&Component { solid: shape.solid, sides: sides.to_vec() });
            json!({
                "solid":shape.solid,
                "vertices":shape.vertices.iter().map(|vertex| json!({"sides":vertex.sides,"position":vertex.position.to_array(),"selected":selected(&vertex.sides)})).collect::<Vec<_>>(),
                "edges":shape.edges.iter().map(|edge| json!({"sides":edge.sides,"vertices":edge.vertices,"positions":edge.vertices.map(|index|shape.vertices[index].position.to_array()),"selected":selected(&edge.sides)})).collect::<Vec<_>>(),
                "faces":shape.faces.iter().map(|face|json!({"side":face.side,"vertices":face.vertices})).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>();
        Ok(json!({
            "snapshot":self.component_stamp(), "generation":self.document_generation, "revision":self.document.revision(),
            "brushes":brushes, "selection":self.components.selection.iter().map(component_json).collect::<Vec<_>>(),
            "preview":self.components.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"delta":preview.delta.to_array(),"brushes":preview.replacements.len()})),
            "texture_mode":"world", "limits":{"brushes":subobject::MAX_SOLIDS,"sides_per_brush":subobject::MAX_SIDES,"selected_components":subobject::MAX_COMPONENTS},
        }))
    }

    pub fn select_components(
        &mut self,
        components: &[Component],
        mode: SelectMode,
        stamp: &str,
    ) -> Result<(), String> {
        self.check_component_stamp(stamp)?;
        if components.len() > subobject::MAX_COMPONENTS {
            return Err("Select at most 512 vertices or edges".into());
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
        if !owners.is_empty() {
            self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        }
        self.cancel_block_preview();
        self.cancel_face_preview();
        self.cancel_clip_preview();
        self.clear_primitive_preview();
        self.clear_volume_selection();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clear_paint_stroke();
        self.clip_tool = false;
        self.paint.enabled = false;
        self.spatial.draw_block = false;
        self.components.selection = next;
        self.components.preview = None;
        self.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn preview_component_move(
        &mut self,
        delta: DVec3,
        snap: bool,
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
        let replacements = subobject::plan_move(&self.document, &components, delta)
            .map_err(|error| error.to_string())?;
        let mut document = self.document.clone();
        document
            .edit("preview component move", |tx| {
                subobject::apply(tx, &replacements)
            })
            .map_err(|error| error.to_string())?;
        self.components.serial += 1;
        let token = self.components.serial;
        self.components.preview = Some(Preview {
            document,
            token,
            delta,
            replacements,
            generation: self.document_generation,
            revision: self.document.revision(),
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
        let replacements = self.checked_component_preview(token)?.replacements.clone();
        let selection = self.components.selection.clone();
        let result = self.edit(actor, "move brush components", |tx| {
            subobject::apply(tx, &replacements)
        })?;
        self.components.selection = selection;
        Ok(result)
    }

    pub fn cancel_component_move(&mut self, token: u64) -> Result<(), String> {
        self.checked_component_preview(token)?;
        self.components.preview = None;
        self.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn append_component_guides(&self, mesh: &mut render::SceneMesh) {
        let ids = self
            .components
            .selection
            .iter()
            .map(|component| component.solid)
            .collect::<Vec<_>>();
        let Ok(shapes) = self.component_topology(&ids) else {
            return;
        };
        for shape in shapes {
            let mut selected_vertices = BTreeSet::new();
            for component in self
                .components
                .selection
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
                if self.components.selection.contains(&Component {
                    solid: shape.solid,
                    sides: edge.sides.clone(),
                }) {
                    for index in edge.vertices {
                        mesh.lines.push(render::scene::LineVertex {
                            position: shape.vertices[index].position.as_vec3().to_array(),
                            color: [255, 190, 65, 255],
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

    #[test]
    fn component_preview_apply_cancel_and_undo_share_exact_geometry() {
        let (mut session, id, component) = fixture();
        let original = session.document.vmf().clone();
        let revision = session.document.revision();
        let history = session.document.undo_labels().count();
        let token = session
            .preview_component_move(DVec3::Z * 16.0, false, &session.component_stamp())
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
                .preview_component_move(DVec3::Z * -64.0, false, &session.component_stamp())
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
    fn guards_prevent_stale_clients_from_replacing_or_cancelling_drafts() {
        let (mut session, _, component) = fixture();
        let stale = session.component_stamp();
        let first = session
            .preview_component_move(DVec3::Z * 16.0, false, &stale)
            .unwrap();
        assert!(
            session
                .preview_component_move(DVec3::Z * 24.0, false, &stale)
                .is_err()
        );
        assert!(
            session
                .select_components(&[], SelectMode::Replace, &stale)
                .is_err()
        );
        let second = session
            .preview_component_move(DVec3::Z * 32.0, false, &session.component_stamp())
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
            .preview_component_move(DVec3::Z * 16.0, false, &session.component_stamp())
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
            .preview_component_move(DVec3::Z * 19.0, true, &session.component_stamp())
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
            .preview_component_move(DVec3::Z * 16.0, false, &session.component_stamp())
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
