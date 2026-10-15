use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use glam::DVec3;
use map_core::{Document, ObjectId, query};
use serde_json::{Value, json};

use super::{Actor, Session, models::Models};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    generation: u64,
    revision: u64,
    scene: u64,
    region: u64,
    materials: u64,
    document: usize,
    document_revision: u64,
    tools: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub enabled: bool,
    pub bounds: Option<[DVec3; 2]>,
    serial: u64,
    cache: RefCell<Option<(Key, Arc<Evaluation>)>>,
}

pub enum Action {
    Set { bounds: [DVec3; 2], enabled: bool },
    SelectionBounds { enabled: bool },
    Clear,
}

#[derive(Clone)]
struct Record {
    id: ObjectId,
    owner: Option<ObjectId>,
    included: bool,
    reasons: Vec<&'static str>,
    bounds: Option<[DVec3; 2]>,
    source: &'static str,
    error: Option<String>,
}

impl Record {
    fn json(&self) -> Value {
        json!({"id":crate::identity::id_string(self.id),"owner":self.owner.map(crate::identity::id_string),"included":self.included,"reasons":self.reasons,"bounds":self.bounds.map(|bounds|bounds.map(|point|point.to_array())),"bounds_source":self.source,"bounds_error":self.error})
    }
}

pub(super) struct Evaluation {
    pub models: Models,
    pub objects: HashSet<ObjectId>,
    pub solids: HashSet<u32>,
    records: Vec<Record>,
    excluded: usize,
}

fn validate([mins, maxs]: [DVec3; 2]) -> Result<(), String> {
    if !mins.is_finite() || !maxs.is_finite() || !mins.cmplt(maxs).all() {
        return Err(
            "Working region needs finite corners and positive width, height and depth".into(),
        );
    }
    if mins.abs().max_element() > crate::spatial::MAX_EXTENT
        || maxs.abs().max_element() > crate::spatial::MAX_EXTENT
    {
        return Err(format!(
            "Working region corners must stay within ±{} world units",
            crate::spatial::MAX_EXTENT
        ));
    }
    Ok(())
}

fn intersects(bounds: [DVec3; 2], region: [DVec3; 2]) -> bool {
    bounds[0].cmple(region[1]).all() && bounds[1].cmpge(region[0]).all()
}

fn checked_bounds(bounds: Option<(DVec3, DVec3)>) -> Option<[DVec3; 2]> {
    bounds
        .filter(|(mins, maxs)| mins.is_finite() && maxs.is_finite())
        .map(|(mins, maxs)| [mins, maxs])
}

fn records(document: &Document, models: &Models, region: Option<[DVec3; 2]>) -> Vec<Record> {
    let instances = models
        .instances
        .iter()
        .map(|instance| (instance.id, instance))
        .collect::<HashMap<_, _>>();
    let mut records = Vec::new();
    for id in document.objects() {
        let mut children = Vec::new();
        let (hidden, bounds, source, error) = match id {
            ObjectId::Solid(id) => {
                let solid = document.solid(id).unwrap();
                (
                    solid.hidden,
                    checked_bounds(query::solid_bounds(solid)),
                    "brush",
                    None,
                )
            }
            ObjectId::Entity(id) => {
                let entity = document.entity(id).unwrap();
                if entity.solids.is_empty() {
                    let instance = instances.get(&ObjectId::Entity(id)).copied();
                    let source = if instance.is_some_and(render::models::Instance::has_geometry) {
                        "model"
                    } else if entity.properties.get("model").is_some() {
                        "model_helper_fallback"
                    } else {
                        "helper"
                    };
                    let bounds = instance
                        .map(|instance| instance.bounds())
                        .or_else(|| query::entity_bounds(entity));
                    (
                        entity.hidden,
                        checked_bounds(bounds),
                        source,
                        models.issues.get(&id).cloned(),
                    )
                } else {
                    children = entity
                        .solids
                        .iter()
                        .map(|solid| (solid, checked_bounds(query::solid_bounds(solid))))
                        .collect();
                    let visible = children
                        .iter()
                        .filter(|(solid, _)| !solid.hidden)
                        .map(|(_, bounds)| *bounds)
                        .collect::<Option<Vec<_>>>();
                    let bounds = visible.and_then(|bounds| {
                        query::union(bounds.into_iter().map(|[mins, maxs]| (mins, maxs)))
                    });
                    (
                        entity.hidden,
                        checked_bounds(bounds),
                        "visible_children",
                        None,
                    )
                }
            }
        };
        let outside =
            region.is_some_and(|region| bounds.is_some_and(|bounds| !intersects(bounds, region)));
        let no_visible_children =
            !children.is_empty() && children.iter().all(|(child, _)| child.hidden);
        let included = !hidden && !outside && !no_visible_children;
        let mut reasons = Vec::new();
        if hidden || no_visible_children {
            reasons.push("authored_hidden");
        }
        if outside {
            reasons.push("outside_working_region");
        }
        if bounds.is_none() && !no_visible_children {
            reasons.push("unresolved_bounds");
        }
        records.push(Record {
            id,
            owner: None,
            included,
            reasons,
            bounds,
            source,
            error,
        });
        for (child, bounds) in children {
            let mut reasons = Vec::new();
            if hidden {
                reasons.push("owner_hidden");
            }
            if child.hidden {
                reasons.push("authored_hidden");
            }
            if outside {
                reasons.push("outside_working_region");
            }
            if bounds.is_none() {
                reasons.push("unresolved_bounds");
            }
            records.push(Record {
                id: ObjectId::Solid(child.id),
                owner: Some(id),
                included: included && !child.hidden,
                reasons,
                bounds,
                source: "owner_unit",
                error: None,
            });
        }
    }
    records
}

impl Session {
    pub fn working_region_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.working_region.serial,
            self.materials.generation(),
            self.view.tool_textures
        )
    }

    pub fn working_region_enabled(&self) -> bool {
        self.working_region.enabled
    }

    pub(super) fn working_evaluation(&self, document: &Document) -> Arc<Evaluation> {
        let key = Key {
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            region: self.working_region.serial,
            materials: self.materials.generation(),
            document: document as *const Document as usize,
            document_revision: document.revision(),
            tools: self.view.tool_textures,
        };
        if let Some((old, evaluation)) = self.working_region.cache.borrow().as_ref()
            && old == &key
        {
            return evaluation.clone();
        }
        let models = self.model_instances(document);
        let mut objects = HashSet::new();
        let mut solids = HashSet::new();
        let records = if self.working_region.enabled {
            let records = records(document, &models, self.working_region.bounds);
            for record in &records {
                if record.included {
                    if record.owner.is_none() {
                        objects.insert(record.id);
                    }
                    if let ObjectId::Solid(id) = record.id {
                        solids.insert(id);
                    }
                }
            }
            records
        } else {
            for solid in document
                .vmf()
                .world
                .solids
                .iter()
                .filter(|solid| !solid.hidden)
            {
                objects.insert(ObjectId::Solid(solid.id));
                solids.insert(solid.id);
            }
            for entity in document
                .vmf()
                .entities
                .iter()
                .filter(|entity| !entity.hidden)
            {
                objects.insert(ObjectId::Entity(entity.id));
                solids.extend(
                    entity
                        .solids
                        .iter()
                        .filter(|solid| !solid.hidden)
                        .map(|solid| solid.id),
                );
            }
            Vec::new()
        };
        let excluded = records.iter().filter(|record| !record.included).count();
        let evaluation = Arc::new(Evaluation {
            models,
            objects,
            solids,
            records,
            excluded,
        });
        *self.working_region.cache.borrow_mut() = Some((key, evaluation.clone()));
        evaluation
    }

    pub(super) fn working_object_visible(&self, document: &Document, id: ObjectId) -> bool {
        !self.working_region.enabled || self.working_evaluation(document).objects.contains(&id)
    }

    pub(super) fn working_solid_visible(&self, document: &Document, id: u32) -> bool {
        !self.working_region.enabled || self.working_evaluation(document).solids.contains(&id)
    }

    pub(super) fn working_selection_visible(&self) -> bool {
        if !self.working_region.enabled {
            return true;
        }
        let evaluation = self.working_evaluation(self.render_document());
        if let Some(preview) = &self.geometry.clip_preview {
            return preview
                .result
                .objects
                .iter()
                .all(|id| evaluation.objects.contains(id));
        }
        self.selection
            .iter()
            .all(|id| evaluation.objects.contains(id))
            && self
                .face_selection
                .iter()
                .all(|face| evaluation.solids.contains(&face.solid))
    }

    pub fn working_region_selection_bounds(&self) -> Result<[DVec3; 2], String> {
        if self.selection.is_empty() && self.face_selection.is_empty() {
            return Err("Select objects or faces before using selection bounds".into());
        }
        let (mut mins, mut maxs) = self
            .selection_bounds()
            .ok_or("Selection has no valid bounds")?;
        for axis in 0..3 {
            if maxs[axis] <= mins[axis] {
                mins[axis] -= self.spatial.grid_size.max(1.0);
                maxs[axis] += self.spatial.grid_size.max(1.0);
            }
        }
        validate([mins, maxs])?;
        Ok([mins, maxs])
    }

    pub fn working_region_summary(&self) -> Value {
        let counts = self.working_region.enabled.then(|| {
            let evaluation = self.working_evaluation(self.render_document());
            let selected = self
                .selection
                .iter()
                .filter(|id| !evaluation.objects.contains(id))
                .count()
                + self
                    .face_selection
                    .iter()
                    .filter(|face| !evaluation.solids.contains(&face.solid))
                    .count();
            (evaluation.excluded, selected)
        });
        json!({"snapshot":self.working_region_stamp(),"enabled":self.working_region.enabled,"mins":self.working_region.bounds.map(|bounds|bounds[0].to_array()),"maxs":self.working_region.bounds.map(|bounds|bounds[1].to_array()),"excluded":counts.map(|counts|counts.0),"selected_excluded":counts.map(|counts|counts.1),"counts_scope":"working_region_and_authored_visibility","policy":"Session-only whole-owner bounds intersection; source, selection, history and compilation are unchanged."})
    }

    pub fn set_working_region(
        &mut self,
        actor: Actor,
        action: Action,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if snapshot != self.working_region_stamp() {
            return Err("Map, selection or view changed; inspect the working region again".into());
        }
        let next = match action {
            Action::Clear => (false, None),
            Action::Set { bounds, enabled } => (enabled, Some(bounds)),
            Action::SelectionBounds { enabled } => {
                (enabled, Some(self.working_region_selection_bounds()?))
            }
        };
        if let Some(bounds) = next.1 {
            validate(bounds)?;
        }
        if (self.working_region.enabled, self.working_region.bounds) != next {
            self.interaction_camera_changed();
            self.clear_paint_stroke();
            self.working_region_cancel_terrain_drag();
            self.working_region_end_terrain_stroke();
            self.clear_volume_selection();
            self.cancel_block_preview();
            self.clip_epoch += 1;
            self.interaction_epoch += 1;
            self.working_region.enabled = next.0;
            self.working_region.bounds = next.1;
            self.working_region.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
            self.log(
                actor,
                if next.0 {
                    "Set working region visibility"
                } else {
                    "Cleared or disabled working region visibility"
                },
            );
        }
        self.get_working_region(0, 50)
    }

    pub fn get_working_region(&mut self, offset: usize, limit: usize) -> Result<Value, String> {
        if !(1..=200).contains(&limit) {
            return Err("Working region page limit must be between 1 and 200".into());
        }
        self.hidden_tool_brushes();
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let mut records = if self.working_region.enabled {
            evaluation.records.clone()
        } else {
            records(document, &evaluation.models, None)
        };
        if !self.view.tool_textures {
            let tools = document
                .vmf()
                .solids()
                .filter(|solid| {
                    !solid.sides.is_empty()
                        && solid.sides.iter().all(|side| {
                            self.materials
                                .find(&side.material)
                                .is_some_and(|info| info.tool)
                        })
                })
                .map(|solid| solid.id)
                .collect::<HashSet<_>>();
            let owners = document
                .vmf()
                .entities
                .iter()
                .filter(|entity| {
                    entity.solids.iter().any(|solid| !solid.hidden)
                        && entity
                            .solids
                            .iter()
                            .filter(|solid| !solid.hidden)
                            .all(|solid| tools.contains(&solid.id))
                })
                .map(|entity| ObjectId::Entity(entity.id))
                .collect::<HashSet<_>>();
            for record in &mut records {
                if owners.contains(&record.id)
                    || matches!(record.id,ObjectId::Solid(id) if tools.contains(&id))
                {
                    record.included = false;
                    record.reasons.push("tool_filter");
                }
            }
        }
        let excluded = records
            .iter()
            .filter(|record| !record.included)
            .map(|record| record.id)
            .collect::<HashSet<_>>();
        let selected_excluded = self
            .selection
            .iter()
            .filter(|id| excluded.contains(id))
            .count()
            + self
                .face_selection
                .iter()
                .filter(|face| excluded.contains(&ObjectId::Solid(face.solid)))
                .count();
        let mut result = self.working_region_summary();
        result["counts_scope"] = json!("all_display_filters");
        result
            .as_object_mut()
            .unwrap()
            .extend(serde_json::Map::from_iter([
                ("total".into(), json!(records.len())),
                ("excluded".into(), json!(excluded.len())),
                ("selected_excluded".into(), json!(selected_excluded)),
                ("offset".into(), json!(offset)),
                ("limit".into(), json!(limit)),
                (
                    "next_offset".into(),
                    json!(
                        (offset.saturating_add(limit) < records.len())
                            .then_some(offset.saturating_add(limit))
                    ),
                ),
                (
                    "objects".into(),
                    json!(
                        records
                            .iter()
                            .skip(offset)
                            .take(limit)
                            .map(Record::json)
                            .collect::<Vec<_>>()
                    ),
                ),
                (
                    "limits".into(),
                    json!({"coordinate":crate::spatial::MAX_EXTENT,"page":200}),
                ),
            ]));
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
