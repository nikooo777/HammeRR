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
    pub(super) serial: u64,
    pub(super) editor_hidden: Arc<HashSet<ObjectId>>,
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
    pub helpers: std::collections::BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
    pub ropes: Arc<render::ropes::Preview>,
    pub objects: HashSet<ObjectId>,
    pub solids: HashSet<u32>,
    pub editor_hidden: Arc<HashSet<ObjectId>>,
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

fn display_solid_bounds(solid: &source_formats::vmf::Solid) -> Option<[DVec3; 2]> {
    if !solid.sides.iter().any(|side| side.displacement.is_some()) {
        return checked_bounds(query::solid_bounds(solid));
    }
    let geometry = map_core::BrushGeometry::from_solid(solid).ok()?;
    let mut bounds: Option<(DVec3, DVec3)> = None;
    for (index, side) in solid.sides.iter().enumerate() {
        let Some(info) = &side.displacement else {
            continue;
        };
        let face = geometry.faces.iter().find(|face| face.side == index)?;
        let mesh = map_core::DisplacementMesh::build(face, info).ok()?;
        for point in mesh.positions {
            if !point.is_finite() {
                return None;
            }
            bounds = Some(match bounds {
                Some((mins, maxs)) => (mins.min(point), maxs.max(point)),
                None => (point, point),
            });
        }
    }
    checked_bounds(bounds)
}

fn records(
    document: &Document,
    models: &Models,
    helpers: &std::collections::BTreeMap<ObjectId, map_core::entity_helpers::Appearance>,
    region: Option<[DVec3; 2]>,
) -> Vec<Record> {
    enum Source<'a> {
        Solid(&'a source_formats::vmf::Solid),
        Entity(&'a source_formats::vmf::Entity),
    }

    let instances = models
        .instances
        .iter()
        .map(|instance| (instance.id, instance))
        .collect::<HashMap<_, _>>();
    let vmf = document.vmf();
    let sources = vmf
        .world
        .solids
        .iter()
        .map(|solid| (ObjectId::Solid(solid.id), Source::Solid(solid)))
        .chain(
            vmf.entities
                .iter()
                .map(|entity| (ObjectId::Entity(entity.id), Source::Entity(entity))),
        );
    let mut records = Vec::new();
    for (id, object) in sources {
        let mut children = Vec::new();
        let (hidden, bounds, source, error) = match object {
            Source::Solid(solid) => (solid.hidden, display_solid_bounds(solid), "brush", None),
            Source::Entity(entity) => {
                if entity.solids.is_empty() {
                    let instance = instances.get(&id).copied();
                    let helper = helpers.get(&id);
                    let source = if instance.is_some_and(render::models::Instance::has_geometry) {
                        "model"
                    } else if helper.is_some_and(|helper| helper.invalid_size) {
                        "invalid_fgd_size"
                    } else if helper.is_some_and(|helper| helper.local_bounds.is_some()) {
                        "fgd_size"
                    } else if entity.properties.get("model").is_some() {
                        "model_helper_fallback"
                    } else {
                        "helper"
                    };
                    let bounds = instance
                        .filter(|instance| instance.has_geometry())
                        .map(|instance| instance.bounds())
                        .or_else(|| helper.and_then(|helper| helper.world_bounds(entity)))
                        .or_else(|| instance.map(|instance| instance.bounds()))
                        .or_else(|| query::entity_bounds(entity));
                    let bounds = if source == "invalid_fgd_size" {
                        None
                    } else {
                        bounds
                    };
                    (
                        entity.hidden,
                        checked_bounds(bounds),
                        source,
                        models.issues.get(&entity.id).cloned().or_else(|| {
                            helper
                                .filter(|helper| !helper.errors.is_empty())
                                .map(|helper| helper.errors.join("; "))
                        }),
                    )
                } else {
                    children = entity
                        .solids
                        .iter()
                        .map(|solid| (solid, display_solid_bounds(solid)))
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

fn apply_editor_hidden(records: &mut [Record], hidden: &HashSet<ObjectId>) {
    for record in records {
        if hidden.contains(&record.owner.unwrap_or(record.id)) {
            record.included = false;
            record.reasons.push(if record.owner.is_some() {
                "owner_editor_hidden"
            } else {
                "editor_hidden"
            });
        }
    }
}

impl Session {
    pub(super) fn display_visibility_filtered(&self) -> bool {
        self.working_region.enabled || !self.working_region.editor_hidden.is_empty()
    }

    pub(super) fn editor_object_hidden(&self, id: ObjectId) -> bool {
        self.working_region.editor_hidden.contains(&id)
    }

    pub(super) fn invalidate_display_visibility_gestures(&mut self) {
        self.cancel_geometry_drags();
        self.cancel_selection_previews();
        self.working_region_cancel_terrain_drag();
        self.working_region_end_terrain_stroke();
        self.cancel_block_preview();
        self.clip_epoch += 1;
        self.interaction_epoch += 1;
    }

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
        self.working_region.cache.borrow_mut().take();
        let models = self.model_instances(document);
        let helpers = self.fgd_helper_appearances(document, &models);
        let mut objects = HashSet::new();
        let mut solids = HashSet::new();
        let records = if self.working_region.enabled {
            let mut records = records(document, &models, &helpers, self.working_region.bounds);
            apply_editor_hidden(&mut records, &self.working_region.editor_hidden);
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
            for solid in document.vmf().world.solids.iter().filter(|solid| {
                !solid.hidden && !self.editor_object_hidden(ObjectId::Solid(solid.id))
            }) {
                objects.insert(ObjectId::Solid(solid.id));
                solids.insert(solid.id);
            }
            for entity in document.vmf().entities.iter().filter(|entity| {
                !entity.hidden && !self.editor_object_hidden(ObjectId::Entity(entity.id))
            }) {
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
        let excluded = if self.working_region.enabled {
            records.iter().filter(|record| !record.included).count()
        } else {
            document
                .vmf()
                .world
                .solids
                .iter()
                .filter(|solid| !objects.contains(&ObjectId::Solid(solid.id)))
                .count()
                + document
                    .vmf()
                    .entities
                    .iter()
                    .map(|entity| {
                        usize::from(!objects.contains(&ObjectId::Entity(entity.id)))
                            + entity
                                .solids
                                .iter()
                                .filter(|solid| !solids.contains(&solid.id))
                                .count()
                    })
                    .sum::<usize>()
        };
        let ropes = Arc::new(render::ropes::prepare(document.vmf(), |id| {
            objects.contains(&id)
        }));
        let evaluation = Arc::new(Evaluation {
            ropes,
            models,
            helpers,
            objects,
            solids,
            records,
            excluded,
            editor_hidden: self.working_region.editor_hidden.clone(),
        });
        *self.working_region.cache.borrow_mut() = Some((key, evaluation.clone()));
        evaluation
    }

    pub(super) fn working_object_visible(&self, document: &Document, id: ObjectId) -> bool {
        !self.display_visibility_filtered()
            || self.working_evaluation(document).objects.contains(&id)
    }

    pub(super) fn working_solid_visible(&self, document: &Document, id: u32) -> bool {
        !self.display_visibility_filtered()
            || self.working_evaluation(document).solids.contains(&id)
    }

    pub(super) fn working_selection_visible(&self) -> bool {
        if !self.display_visibility_filtered() {
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
        let counts = self.display_visibility_filtered().then(|| {
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
        json!({"snapshot":self.working_region_stamp(),"enabled":self.working_region.enabled,"mins":self.working_region.bounds.map(|bounds|bounds[0].to_array()),"maxs":self.working_region.bounds.map(|bounds|bounds[1].to_array()),"excluded":counts.map(|counts|counts.0),"selected_excluded":counts.map(|counts|counts.1),"counts_scope":"working_region_authored_and_editor_visibility","policy":"Session-only whole-owner bounds intersection; source, selection, history and compilation are unchanged."})
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
            self.invalidate_display_visibility_gestures();
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
            records(document, &evaluation.models, &evaluation.helpers, None)
        };
        if !self.working_region.enabled {
            apply_editor_hidden(&mut records, &self.working_region.editor_hidden);
        }
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
