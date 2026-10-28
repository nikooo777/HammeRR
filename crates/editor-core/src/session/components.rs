use super::previews;
use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::subobject::{
    self, Component, ComponentTransform, Crease, CreaseChange, CreasePair, Replacement,
    SplitChange, Topology, WeldChange, WeldTarget,
};
use map_core::{Document, EditError, ObjectId};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

mod extrusion;
mod partitions;
pub use partitions::PartitionRef;

pub const SOURCE_PAGE_SIZE: usize = 100;
pub const MAX_SOURCE_PAGE_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeAction {
    Replace,
    Add,
    Remove,
}

pub struct Preview {
    pub document: Document,
    pub token: u64,
    pub(super) delta: DVec3,
    pub(super) texture_lock: bool,
    replacements: Vec<Replacement>,
    generation: u64,
    revision: u64,
    pub(super) weld: Option<Vec<WeldChange>>,
    weld_target: Option<WeldTarget>,
    split: Option<SplitDraft>,
    crease: Option<CreaseDraft>,
    partition: Option<partitions::Draft>,
    extrusion: Option<extrusion::Draft>,
    pub(super) transform: Option<TransformDraft>,
    selection: BTreeSet<Component>,
}

impl Preview {
    fn new(document: Document, selection: BTreeSet<Component>) -> Self {
        Self {
            document,
            selection,
            token: 0,
            generation: 0,
            revision: 0,
            delta: DVec3::ZERO,
            texture_lock: false,
            replacements: Vec::new(),
            weld: None,
            weld_target: None,
            split: None,
            crease: None,
            partition: None,
            extrusion: None,
            transform: None,
        }
    }
}

pub(super) struct TransformDraft {
    spec: ComponentTransform,
    pivot: Option<DVec3>,
    resolved_pivot: DVec3,
}

struct SplitDraft {
    rise: f64,
    changes: Vec<SplitChange>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, schemars::JsonSchema)]
pub struct CreaseEdges {
    pub solid: u32,
    pub edges: [u32; 2],
}

struct CreaseDraft {
    rise: f64,
    roof_texture_lock: bool,
    requests: Vec<Crease>,
    changes: Vec<CreaseChange>,
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

pub fn weld_target_json(target: &WeldTarget) -> Value {
    match target {
        WeldTarget::Mean => json!({"mode":"mean"}),
        WeldTarget::Position(position) => json!({"mode":"position","position":position.to_array()}),
        WeldTarget::Corner(component) => {
            json!({"mode":"corner","component":component_json(component)})
        }
    }
}

pub fn transform_json(
    spec: &ComponentTransform,
    pivot: Option<DVec3>,
    resolved_pivot: DVec3,
) -> Value {
    let (kind, value) = match spec {
        ComponentTransform::Scale(value) => ("scale", value),
        ComponentTransform::Rotate(value) => ("rotate", value),
    };
    json!({"kind":kind,"value":value.to_array(),"pivot":pivot.map(|point|point.to_array()),"resolved_pivot":resolved_pivot.to_array()})
}

impl Session {
    pub fn component_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.components.serial
        )
    }

    pub(super) fn check_component_stamp(&self, stamp: &str) -> Result<(), String> {
        if stamp != self.component_stamp() {
            return Err("Map, selection or preview changed; inspect brush components again".into());
        }
        Ok(())
    }

    pub fn component_selection(&self) -> &BTreeSet<Component> {
        &self.geometry.components.selection
    }

    pub fn displayed_component_selection(&self) -> &BTreeSet<Component> {
        self.geometry
            .components
            .preview
            .as_ref()
            .map_or(&self.geometry.components.selection, |preview| {
                &preview.selection
            })
    }

    pub fn component_topology_preview(&self) -> bool {
        self.geometry
            .components
            .preview
            .as_ref()
            .is_some_and(|preview| {
                preview.weld.is_some()
                    || preview.split.is_some()
                    || preview.crease.is_some()
                    || preview.partition.is_some()
            })
    }

    pub fn clear_component_edit(&mut self) {
        if !self.geometry.components.selection.is_empty()
            || self.geometry.components.preview.is_some()
            || self.geometry.components.picking.is_some()
            || self.geometry.components.drag.is_some()
        {
            self.geometry.components.drag = None;
            self.geometry.components.origin = None;
            self.geometry.components.picking = None;
            self.geometry.components.selection.clear();
            self.geometry.components.preview = None;
            self.geometry.components.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn component_solids(&self) -> Vec<u32> {
        if let Some(picking) = &self.geometry.components.picking {
            return picking.solids.iter().copied().collect();
        }
        self.selected_component_solids()
    }

    fn selected_component_solids(&self) -> Vec<u32> {
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

    pub fn update_component_scope(
        &mut self,
        kind: super::component_picking::Kind,
        solid: u32,
        action: ScopeAction,
        stamp: &str,
    ) -> Result<(), String> {
        self.check_component_stamp(stamp)?;
        if !self.selected_component_solids().contains(&solid) {
            return Err("Choose a source brush belonging to the selected owners".into());
        }
        let mut ids = self
            .geometry
            .components
            .picking
            .as_ref()
            .map_or_else(BTreeSet::new, |picking| picking.solids.clone());
        match action {
            ScopeAction::Replace => ids = BTreeSet::from([solid]),
            ScopeAction::Add => {
                if ids.contains(&solid)
                    && self
                        .geometry
                        .components
                        .picking
                        .as_ref()
                        .is_some_and(|picking| picking.kind == kind)
                {
                    return Ok(());
                }
                ids.insert(solid);
            }
            ScopeAction::Remove => {
                if !ids.remove(&solid) {
                    return Err("That brush is not in the captured scope".into());
                }
            }
        }
        self.set_component_picking(
            (!ids.is_empty()).then_some(kind),
            &ids.into_iter().collect::<Vec<_>>(),
            stamp,
        )
    }

    pub fn component_sources(&self, offset: usize, limit: usize) -> Result<Value, String> {
        if !(1..=MAX_SOURCE_PAGE_SIZE).contains(&limit) {
            return Err("Brush source pages contain between 1 and 256 entries".into());
        }
        let ids = self
            .selected_component_solids()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let sources = ids.iter().skip(offset).take(limit).map(|id| {
            let (owner,solid) = subobject::solid(&self.document,*id).map_err(|error|error.to_string())?;
            let owner = match owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")};
            Ok(json!({"solid":id,"owner":owner,"material":solid.sides.first().map(|side|side.material.to_ascii_lowercase()),"bounds":map_core::query::solid_bounds(solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()}))}))
        }).collect::<Result<Vec<Value>,String>>()?;
        Ok(
            json!({"snapshot":self.component_stamp(),"offset":offset,"limit":limit,"total":ids.len(),"sources":sources,"active_solids":self.component_solids(),"inspection_limit":subobject::MAX_SOLIDS}),
        )
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
            "move_options":self.geometry.components.options, "drag":self.component_drag_report(),
            "picking":self.geometry.components.picking, "snapshot":self.component_stamp(), "generation":self.document_generation, "revision":self.document.revision(),
            "brushes":brushes, "selection":self.displayed_component_selection().iter().map(component_json).collect::<Vec<_>>(),
            "source_selection":self.geometry.components.selection.iter().map(component_json).collect::<Vec<_>>(),
            "component_center":self.component_center()?.map(|point|point.to_array()),
            "weld_center":self.component_weld_center()?.map(|center|center.to_array()),
            "crease_choices":self.component_crease_choices()?.iter().map(|(component,pairs)|json!({"component":component_json(component),"pairs":pairs.iter().map(|pair|json!({"edges":pair.edges,"midpoints":pair.midpoints.map(|point|point.to_array())})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "partition_choices":self.component_partition_choices()?.iter().map(|(component,pairs)|json!({"component":component_json(component),"pairs":pairs.iter().map(|pair|json!({"edges":pair.edges,"midpoints":pair.midpoints.map(|point|point.to_array())})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "weld_targets":self.component_weld_targets()?.iter().map(|(component,position)|json!({"component":component_json(component),"position":position.to_array()})).collect::<Vec<_>>(),
            "preview":self.geometry.components.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"operation":if preview.extrusion.is_some() {"extrusion"} else if preview.partition.is_some() {"partition"} else if preview.weld.is_some() {"weld"} else if preview.split.is_some() {"split"} else if preview.crease.is_some() {"crease"} else if let Some(transform) = &preview.transform { match transform.spec { ComponentTransform::Scale(_) => "scale", ComponentTransform::Rotate(_) => "rotate" } } else {"move"},"delta":preview.delta.to_array(),"texture_lock":preview.texture_lock,"brushes":preview.partition.as_ref().map_or(preview.replacements.len(), |draft| draft.partitions.len()),"can_apply":preview.extrusion.as_ref().map_or_else(||preview.partition.as_ref().is_none_or(|draft| draft.rise != 0.0),|extrusion|preview.partition.as_ref().is_some_and(|draft|draft.selection==extrusion.selection)),"extrusion":preview.extrusion.as_ref().map(extrusion::Draft::report),"partition":preview.partition.as_ref().map(|draft|draft.report(&self.document)),
                "weld_target":preview.weld_target.as_ref().map(weld_target_json),
                "transform":preview.transform.as_ref().map(|transform|transform_json(&transform.spec,transform.pivot,transform.resolved_pivot)),
                "weld":preview.weld.as_ref().map(|changes|changes.iter().map(|change|json!({"solid":change.solid,"position":change.position.to_array(),"merged_vertices":change.merged_vertices,"removed_sides":change.removed_sides,"selection":component_json(&change.selection)})).collect::<Vec<_>>()),
                "crease":preview.crease.as_ref().map(|crease|json!({"rise":crease.rise,"roof_texture_lock":crease.roof_texture_lock,"pairs":crease.requests.iter().map(|request|json!({"solid":request.solid,"edges":request.edges})).collect::<Vec<_>>(),"changes":crease.changes.iter().map(|change|json!({"solid":change.source.solid,"source_side":change.source.side,"edges":change.source.edges,"positions":change.positions.map(|point|point.to_array()),"new_side":change.new_side,"selection":change.selection.iter().map(component_json).collect::<Vec<_>>()})).collect::<Vec<_>>()})),
                "split":preview.split.as_ref().map(|split|json!({"rise":split.rise,"changes":split.changes.iter().map(|change|json!({"solid":change.solid,"position":change.position.to_array(),"source_sides":change.source_sides,"new_sides":change.new_sides,"faces":change.faces.iter().map(|face|json!({"side":face.side,"source":face.source})).collect::<Vec<_>>(),"selection":component_json(&change.selection)})).collect::<Vec<_>>()}))})),
            "texture_mode":if self.geometry.components.preview.as_ref().is_some_and(|preview|preview.extrusion.as_ref().is_some_and(|draft|draft.spec.texture_lock)) {if self.geometry.components.preview.as_ref().unwrap().extrusion.as_ref().unwrap().spec.height < 0.0 {"floor_attached"} else {"top_attached"}} else if self.geometry.components.preview.as_ref().is_some_and(|preview|preview.crease.as_ref().is_some_and(|crease|crease.roof_texture_lock)) {"roof_attached"} else if self.geometry.components.preview.as_ref().is_some_and(|preview|preview.texture_lock && preview.weld.is_some()) {"weld_attached"} else if self.geometry.components.preview.as_ref().is_some_and(|preview|preview.texture_lock) {"attached"} else {"world"}, "limits":{"brushes":subobject::MAX_SOLIDS,"sides_per_brush":subobject::MAX_SIDES,"selected_components":subobject::MAX_COMPONENTS,"split_rise":[subobject::MIN_SPLIT_RISE,subobject::MAX_SPLIT_RISE],"crease_rise":[subobject::MIN_SPLIT_RISE,subobject::MAX_SPLIT_RISE],"partition_source_corners":subobject::MAX_BOUNDARY_SOURCE_CORNERS,"partition_vertices":subobject::MAX_PARTITION_VERTICES,"partition_loop_vertices":subobject::MAX_PARTITION_LOOP_VERTICES,"extrusion_source_vertices":subobject::MAX_EXTRUSION_SOURCE_VERTICES,"extrusion_pieces":subobject::MAX_EXTRUSION_PIECES,"extrusion_planes":subobject::MAX_EXTRUSION_PLANES,"extrusion_fragments":subobject::MAX_EXTRUSION_FRAGMENTS},
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
        let mut next = self.geometry.components.selection.clone();
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
        let picking = self.geometry.components.picking.clone().filter(|picking| {
            next.iter()
                .all(|component| picking.solids.contains(&component.solid))
        });
        if !owners.is_empty() {
            self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        }
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[previews::Kind::Components]);

        self.clear_volume_selection();

        self.cancel_area_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.components.picking = picking;
        self.geometry.components.selection = next;
        self.geometry.components.preview = None;
        self.geometry.components.drag = None;
        self.refresh_component_origin();
        self.geometry.components.serial += 1;
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
            .geometry
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
        self.geometry.components.options.snap = snap;
        self.geometry.components.options.texture_lock = texture_lock;
        Ok(self.install_component_preview(Preview {
            delta,
            texture_lock,
            replacements,
            ..Preview::new(document, self.geometry.components.selection.clone())
        }))
    }

    pub fn component_center(&self) -> Result<Option<DVec3>, String> {
        if self.geometry.components.selection.is_empty() {
            return Ok(None);
        }
        subobject::component_center(
            &self.document,
            &self
                .geometry
                .components
                .selection
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        )
        .map(Some)
        .map_err(|error| error.to_string())
    }

    pub fn preview_component_transform(
        &mut self,
        spec: ComponentTransform,
        pivot: Option<DVec3>,
        texture_lock: bool,
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let components = self
            .geometry
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let plan =
            subobject::plan_transform(&self.document, &components, &spec, pivot, texture_lock)
                .map_err(|error| error.to_string())?;
        let mut document = self.document.preview_snapshot();
        document
            .edit("preview component transform", |tx| {
                subobject::apply(tx, &plan.replacements)
            })
            .map_err(|error| error.to_string())?;
        self.geometry.components.options.texture_lock = texture_lock;
        Ok(self.install_component_preview(Preview {
            texture_lock,
            replacements: plan.replacements,
            transform: Some(TransformDraft {
                spec,
                pivot,
                resolved_pivot: plan.pivot,
            }),
            ..Preview::new(document, self.geometry.components.selection.clone())
        }))
    }

    pub fn component_weld_center(&self) -> Result<Option<DVec3>, String> {
        let selected = self
            .component_weld_targets()?
            .into_iter()
            .filter(|(component, _)| self.geometry.components.selection.contains(component))
            .map(|(_, position)| position)
            .collect::<Vec<_>>();
        Ok((!selected.is_empty()).then(|| selected.iter().sum::<DVec3>() / selected.len() as f64))
    }

    pub fn component_weld_targets(&self) -> Result<Vec<(Component, DVec3)>, String> {
        let ids = self
            .geometry
            .components
            .selection
            .iter()
            .map(|component| component.solid)
            .collect::<BTreeSet<_>>();
        let mut targets = Vec::new();
        for id in ids {
            let (_, source) =
                subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
            let shape = subobject::topology(source).map_err(|error| error.to_string())?;
            targets.extend(shape.vertices.into_iter().map(|vertex| {
                (
                    Component {
                        solid: id,
                        sides: vertex.sides,
                    },
                    vertex.position,
                )
            }));
        }
        Ok(targets)
    }

    pub fn preview_component_weld(&mut self, stamp: &str) -> Result<u64, String> {
        self.preview_component_weld_target(WeldTarget::Mean, stamp)
    }

    pub fn preview_component_weld_target(
        &mut self,
        target: WeldTarget,
        stamp: &str,
    ) -> Result<u64, String> {
        self.preview_component_weld_with_texture(target, false, stamp)
    }

    pub fn preview_component_weld_with_texture(
        &mut self,
        mut target: WeldTarget,
        texture_lock: bool,
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let components = self
            .geometry
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if let WeldTarget::Corner(component) = &mut target {
            component.sides.sort_unstable();
        }
        let plan =
            subobject::plan_weld_with_textures(&self.document, &components, &target, texture_lock)
                .map_err(|error| error.to_string())?;
        let mut document = self.document.preview_snapshot();
        document
            .edit("preview corner weld", |tx| {
                subobject::apply(tx, &plan.replacements)
            })
            .map_err(|error| error.to_string())?;
        let selection = plan
            .changes
            .iter()
            .map(|change| change.selection.clone())
            .collect();
        Ok(self.install_component_preview(Preview {
            texture_lock,
            replacements: plan.replacements,
            weld: Some(plan.changes),
            weld_target: Some(target),
            ..Preview::new(document, selection)
        }))
    }

    pub fn preview_component_split(&mut self, rise: f64, stamp: &str) -> Result<u64, String> {
        self.preview_component_split_with_texture(rise, false, stamp)
    }

    pub fn preview_component_split_with_texture(
        &mut self,
        rise: f64,
        texture_lock: bool,
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let components = self
            .geometry
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut document = self.document.preview_snapshot();
        let plan = document
            .edit("preview component split", |tx| {
                subobject::split_with_textures(tx, &components, rise, texture_lock)
            })
            .map_err(|error| error.to_string())?;
        let selection = plan
            .changes
            .iter()
            .map(|change| change.selection.clone())
            .collect();
        Ok(self.install_component_preview(Preview {
            texture_lock,
            replacements: plan.replacements,
            split: Some(SplitDraft {
                rise,
                changes: plan.changes,
            }),
            ..Preview::new(document, selection)
        }))
    }

    pub fn component_crease_choices(&self) -> Result<Vec<(Component, Vec<CreasePair>)>, String> {
        self.component_cut_choices(false)
    }

    pub fn component_partition_choices(&self) -> Result<Vec<(Component, Vec<CreasePair>)>, String> {
        self.component_cut_choices(true)
    }

    fn component_cut_choices(
        &self,
        boundary: bool,
    ) -> Result<Vec<(Component, Vec<CreasePair>)>, String> {
        let mut choices = Vec::new();
        let mut shapes = BTreeMap::new();
        for component in &self.geometry.components.selection {
            if component.sides.len() != 1 {
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = shapes.entry(component.solid)
            {
                let (_, source) = subobject::solid(&self.document, component.solid)
                    .map_err(|error| error.to_string())?;
                entry.insert(subobject::topology(source).map_err(|error| error.to_string())?);
            }
            let shape = &shapes[&component.solid];
            if shape.faces.iter().any(|face| {
                face.side == component.sides[0]
                    && if boundary {
                        (3..=subobject::MAX_BOUNDARY_SOURCE_CORNERS).contains(&face.vertices.len())
                    } else {
                        face.vertices.len() == 4
                    }
            }) {
                choices.push((
                    component.clone(),
                    if boundary {
                        shape.partition_pairs(component.sides[0])
                    } else {
                        shape.crease_pairs(component.sides[0])
                    }
                    .map_err(|error| error.to_string())?,
                ));
            }
        }
        Ok(choices)
    }

    pub fn preview_component_crease(
        &mut self,
        pairs: &[CreaseEdges],
        rise: f64,
        stamp: &str,
    ) -> Result<u64, String> {
        self.preview_component_crease_with_texture(pairs, rise, false, stamp)
    }

    pub fn preview_component_crease_with_texture(
        &mut self,
        pairs: &[CreaseEdges],
        rise: f64,
        roof_texture_lock: bool,
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let requests = self.crease_requests(pairs)?;
        self.install_crease_preview(requests, rise, roof_texture_lock)
    }

    fn crease_requests(&self, pairs: &[CreaseEdges]) -> Result<Vec<Crease>, String> {
        if pairs.is_empty() || pairs.len() > subobject::MAX_SOLIDS {
            return Err("Choose boundary edge pairs for 1–16 source brushes".into());
        }
        let mut inputs = BTreeMap::new();
        for pair in pairs {
            let mut edges = pair.edges;
            edges.sort_unstable();
            if inputs.insert(pair.solid, edges).is_some() {
                return Err("Choose exactly one boundary edge pair per brush".into());
            }
        }
        let mut requests = Vec::new();
        for component in &self.geometry.components.selection {
            if component.sides.len() != 1 {
                return Err("Select one face per brush".into());
            }
            let edges = inputs
                .remove(&component.solid)
                .ok_or("Choose one boundary edge pair for every selected face")?;
            requests.push(Crease {
                solid: component.solid,
                side: component.sides[0],
                edges,
            });
        }
        if !inputs.is_empty() {
            return Err("Boundary edge pairs must belong to the selected faces".into());
        }
        Ok(requests)
    }

    fn install_crease_preview(
        &mut self,
        requests: Vec<Crease>,
        rise: f64,
        roof_texture_lock: bool,
    ) -> Result<u64, String> {
        let mut document = self.document.preview_snapshot();
        let plan = document
            .edit("preview opposing-edge crease", |tx| {
                subobject::crease_with_roof_textures(tx, &requests, rise, roof_texture_lock)
            })
            .map_err(|error| error.to_string())?;
        let selection = plan
            .changes
            .iter()
            .flat_map(|change| change.selection.iter().cloned())
            .collect();
        Ok(self.install_component_preview(Preview {
            replacements: plan.replacements,
            crease: Some(CreaseDraft {
                rise,
                roof_texture_lock,
                requests,
                changes: plan.changes,
            }),
            ..Preview::new(document, selection)
        }))
    }

    fn install_component_preview(&mut self, mut preview: Preview) -> u64 {
        self.geometry.components.serial += 1;
        preview.token = self.geometry.components.serial;
        preview.generation = self.document_generation;
        preview.revision = self.document.revision();
        self.geometry.components.drag = None;
        self.geometry.components.preview = Some(preview);
        self.scene_serial += 1;
        self.view_serial += 1;
        self.geometry.components.serial
    }

    fn checked_component_preview(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .components
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Component preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the component edit again".into());
        }
        Ok(preview)
    }

    pub fn commit_component_move(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self.checked_component_preview(token)?.weld.is_some()
            || self.checked_component_preview(token)?.split.is_some()
            || self.checked_component_preview(token)?.crease.is_some()
            || self.checked_component_preview(token)?.transform.is_some()
            || self.checked_component_preview(token)?.partition.is_some()
        {
            return Err("Use Apply component edit for this operation".into());
        }
        self.commit_component_edit(actor, token)
    }

    pub fn commit_component_edit(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self
            .geometry
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
        if preview.extrusion.is_some() {
            return self.commit_partition_extrusion(actor, token);
        }
        if preview
            .partition
            .as_ref()
            .is_some_and(|draft| draft.rise == 0.0)
        {
            return Err("Flat partitions are temporary drafts. Raise the shared ridge before Apply, or Cancel.".into());
        }
        let label = if preview.partition.is_some() {
            "raise flat face partitions"
        } else if preview.weld.is_some() {
            "weld brush corners"
        } else if preview.crease.is_some() {
            "crease opposing edges"
        } else if preview.split.is_some() {
            "split brush component"
        } else if let Some(transform) = &preview.transform {
            match transform.spec {
                ComponentTransform::Scale(_) => "scale brush components",
                ComponentTransform::Rotate(_) => "rotate brush components",
            }
        } else {
            "move brush components"
        };
        let replacements = preview.replacements.clone();
        let preview_partition = preview.partition.clone();
        let selection = preview.selection.clone();
        let rise = preview.split.as_ref().map(|split| split.rise);
        let texture_lock = preview.texture_lock;
        let crease = preview.crease.as_ref().map(|crease| {
            (
                crease.requests.clone(),
                crease.rise,
                crease.roof_texture_lock,
            )
        });
        let components = self
            .geometry
            .components
            .selection
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let picking = self.geometry.components.picking.clone().map(|mut picking| {
            if rise.is_some() || crease.is_some() {
                picking.kind = super::component_picking::Kind::Vertex;
            }
            picking
        });
        let result = self.edit(actor, label, |tx| {
            if let Some((requests, rise, roof_texture_lock)) = &crease {
                let actual = if let Some(draft) = &preview_partition {
                    subobject::crease_partitions_with_surfaces(
                        tx,
                        &draft.partitions,
                        *rise,
                        *roof_texture_lock,
                        &draft.material_plan(),
                        &draft.texture_plan(),
                    )?
                } else {
                    subobject::crease_with_roof_textures(tx, requests, *rise, *roof_texture_lock)?
                };
                if actual.replacements != replacements {
                    return Err(EditError::Invalid(
                        "Face allocation changed; preview the crease again".into(),
                    ));
                }
                if preview_partition.is_some() {
                    partitions::consume_saved(tx, requests)?;
                }
                Ok(actual.replacements.len())
            } else if let Some(rise) = rise {
                let actual = subobject::split_with_textures(tx, &components, rise, texture_lock)?;
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
        self.geometry.components.selection = selection;
        self.geometry.components.picking = picking;
        self.refresh_component_origin();
        Ok(result)
    }

    pub fn cancel_component_move(&mut self, token: u64) -> Result<(), String> {
        self.checked_component_preview(token)?;
        self.geometry.components.preview = None;
        self.geometry.components.drag = None;
        self.geometry.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn append_component_guides(&mut self, mesh: &mut render::SceneMesh) {
        self.append_partition_guides(mesh);
        let hidden = self.hidden_tool_brushes();
        let ids = if self.geometry.components.picking.is_some() {
            self.component_solids()
        } else {
            self.geometry
                .components
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
                if selected || self.geometry.components.picking.is_some() {
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

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        (self.preview.is_some() || self.picking.is_some() || self.drag.is_some()).then(|| {
            serde_json::json!([
                self.serial,
                self.preview.as_ref().map(|preview| preview.token),
                self.picking.is_some(),
                self.drag.is_some()
            ])
        })
    }
}
