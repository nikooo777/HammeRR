use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use map_core::ObjectId;
use map_core::query::{self, Hit, Ray};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub const MAX_CANDIDATES: usize = 64;

struct Candidate {
    hit: Hit,
    unit: Vec<ObjectId>,
}

fn compare_hits(a: &Hit, b: &Hit) -> Ordering {
    a.distance
        .total_cmp(&b.distance)
        .then(a.object.cmp(&b.object))
        .then(a.solid.cmp(&b.solid))
        .then(a.side.cmp(&b.side))
}

impl Session {
    fn object_picking_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.face_mode
            || self.component_picking().is_some()
            || !self.component_selection().is_empty()
        {
            return Err("Switch to Objects mode before cycling object hits".into());
        }
        if self.paint_enabled()
            || self.paint_stroke().is_some()
            || self.clip_tool
            || self.clip_points.is_some()
            || self.clip_drag.is_some()
            || self.spatial.draw_block
            || self.block_preview.is_some()
            || self.transform_drag.is_some()
            || self.area_preview.is_some()
            || self.volume_preview().is_some()
            || !self.surface_placement()["token"].is_null()
            || self.terrain_move_handles_json()["active"] == true
            || self.terrain_stroke_json()["active"] == true
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Finish the active tool or preview before cycling object hits".into());
        }
        Ok(())
    }

    fn object_candidates(&mut self, ray: &Ray) -> Result<(Vec<Candidate>, usize), String> {
        self.object_picking_context()?;
        if !ray.origin.is_finite()
            || ray.direction.length_squared() < 1e-24
            || ray.direction.try_normalize().is_none()
        {
            return Err("Object picking needs a finite origin and nonzero finite direction".into());
        }
        self.preflight_instance_contents();
        let references = self.instance_contents_hits(ray, None);
        let hidden = self.hidden_tool_brushes();
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let models = &evaluation.models;
        let model_ids = models
            .instances
            .iter()
            .filter(|instance| instance.has_geometry())
            .map(|instance| instance.id)
            .collect::<HashSet<_>>();
        let mut owners = BTreeMap::<ObjectId, Hit>::new();
        let filters = self.selection_filters();
        let mut retain = |hit: Hit| {
            if !filters.allows(document, hit.object) {
                return;
            }
            let old = owners.entry(hit.object).or_insert(hit);
            if compare_hits(&hit, old).is_lt() {
                *old = hit;
            }
        };
        query::visit_object_hits(
            document,
            ray,
            |solid| !hidden.contains(&solid.id),
            |entity| {
                !model_ids.contains(&ObjectId::Entity(entity.id))
                    && !evaluation
                        .helpers
                        .get(&ObjectId::Entity(entity.id))
                        .is_some_and(|helper| helper.local_bounds.is_some())
                    && evaluation.objects.contains(&ObjectId::Entity(entity.id))
            },
            &mut retain,
        );
        for hit in self.pick_fgd_helpers(document, &evaluation, ray) {
            retain(hit);
        }
        for instance in models
            .instances
            .iter()
            .filter(|instance| evaluation.objects.contains(&instance.id))
        {
            if let Some(hit) = instance.pick(ray) {
                retain(hit);
            }
        }
        if self.view.rope_preview {
            for link in &self.rope_preview_geometry().links {
                if let Some(hit) = link.pick(ray) {
                    retain(hit);
                }
            }
        }
        for hit in references {
            retain(hit);
        }
        let index = self.group_selection_index();
        let mut units = BTreeMap::<map_core::groups::Member, Candidate>::new();
        for hit in owners.into_values() {
            let key = index
                .as_ref()
                .map_or(map_core::groups::Member::Object(hit.object), |index| {
                    index.key(hit.object)
                });
            match units.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if compare_hits(&hit, &entry.get().hit).is_lt() {
                        entry.get_mut().hit = hit;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let mut unit = index
                        .as_ref()
                        .map_or_else(|| vec![hit.object], |index| index.members(hit.object));
                    unit.sort_unstable();
                    unit.dedup();
                    entry.insert(Candidate { hit, unit });
                }
            }
        }
        let total = units.len();
        let mut candidates = units.into_values().collect::<Vec<_>>();
        candidates.sort_by(|a, b| compare_hits(&a.hit, &b.hit));
        candidates.truncate(MAX_CANDIDATES);
        Ok((candidates, total))
    }

    fn object_candidate_report(
        &self,
        candidates: &[Candidate],
        total: usize,
        chosen: Option<usize>,
    ) -> Value {
        let candidates = candidates.iter().map(|candidate| {
            let hit = candidate.hit;
            let label = match hit.object {
                ObjectId::Solid(id) => format!("World brush {id}"),
                ObjectId::Entity(id) => self.document.entity(id).map_or_else(
                    || format!("Entity {id}"),
                    |entity| {
                        let classname = entity.classname().unwrap_or("entity");
                        let name = entity.properties.get("targetname").unwrap_or("");
                        if name.is_empty() { classname.into() } else { format!("{classname} · {name}") }
                    },
                ),
            };
            json!({
                "object":crate::identity::id_string(hit.object),"solid":hit.solid,"side":hit.side,
                "distance":hit.distance,"point":hit.point.to_array(),"label":label,
                "selection_unit":candidate.unit.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>();
        let message = match chosen {
            Some(index) => {
                let label = candidates[index]["label"].as_str().unwrap();
                let suffix = if total > MAX_CANDIDATES {
                    format!(" · nearest {} of {total} hits", candidates.len())
                } else {
                    String::new()
                };
                Some(format!(
                    "Overlap {} of {} · {label}{suffix}",
                    index + 1,
                    candidates.len(),
                ))
            }
            None if total == 0 => Some("No overlapping objects here; selection unchanged.".into()),
            None => None,
        };
        json!({
            "candidates":candidates,"total":total,"truncated":total>MAX_CANDIDATES,
            "limit":MAX_CANDIDATES,"chosen":chosen,"message":message,
            "selection_filters":self.selection_filter_state(),
            "selection":self.selected_ids().into_iter().map(crate::identity::id_string).collect::<Vec<_>>(),
        })
    }

    pub fn object_hit_candidates(&mut self, ray: &Ray) -> Result<Value, String> {
        let (candidates, total) = self.object_candidates(ray)?;
        Ok(self.object_candidate_report(&candidates, total, None))
    }

    pub fn cycle_object_hit(&mut self, actor: Actor, ray: &Ray) -> Result<Value, String> {
        let (candidates, total) = self.object_candidates(ray)?;
        let selected = self.selected_ids();
        let chosen = if candidates.is_empty() {
            None
        } else {
            Some(
                candidates
                    .iter()
                    .position(|candidate| candidate.unit == selected)
                    .map_or(0, |index| (index + 1) % candidates.len()),
            )
        };
        if let Some(index) = chosen {
            self.select(&candidates[index].unit, SelectMode::Replace)?;
        }
        let report = self.object_candidate_report(&candidates, total, chosen);
        if chosen.is_some() {
            self.log(actor, report["message"].as_str().unwrap());
        }
        Ok(report)
    }
}
