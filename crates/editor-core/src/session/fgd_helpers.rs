use std::collections::{BTreeMap, HashSet};

use map_core::entity_helpers::{Appearance, specialized_light};
use map_core::query::{Hit, Ray};
use map_core::{Document, ObjectId};
use serde::Serialize;

use super::{Session, models::Models, working_region::Evaluation};

#[derive(Debug, Serialize)]
pub struct Report {
    pub status: String,
    pub bounds_source: &'static str,
    pub invalid_size: bool,
    pub world_bounds: Option<[[f64; 3]; 2]>,
    pub local_bounds: Option<[[f64; 3]; 2]>,
    pub class_color: Option<[u8; 3]>,
    pub errors: Vec<String>,
    pub unsupported: Vec<String>,
    pub definition_sources: Vec<String>,
    pub limits: &'static str,
}

impl Session {
    pub(super) fn fgd_helper_appearances(
        &self,
        document: &Document,
        models: &Models,
    ) -> BTreeMap<ObjectId, Appearance> {
        let displayed_models = models
            .instances
            .iter()
            .filter(|instance| instance.has_geometry())
            .map(|instance| instance.id)
            .collect::<HashSet<_>>();
        document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                entity.solids.is_empty()
                    && !specialized_light(entity)
                    && !displayed_models.contains(&ObjectId::Entity(entity.id))
            })
            .filter_map(|entity| {
                let class = self.definitions.class(entity.classname()?)?;
                Some((ObjectId::Entity(entity.id), Appearance::resolve(class)))
            })
            .collect()
    }

    pub(super) fn pick_fgd_helpers(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        ray: &Ray,
    ) -> Vec<Hit> {
        evaluation
            .helpers
            .iter()
            .filter(|(id, _)| evaluation.objects.contains(id))
            .filter_map(|(id, helper)| {
                let ObjectId::Entity(id) = id else {
                    return None;
                };
                helper.pick(document.entity(*id)?, ray)
            })
            .collect()
    }

    pub fn entity_helper_report(&self, id: u32) -> Result<Report, String> {
        let document = self.document();
        let entity = document
            .entity(id)
            .ok_or_else(|| format!("no entity:{id}"))?;
        let appearance = entity
            .classname()
            .and_then(|classname| self.definitions.class(classname))
            .map(Appearance::resolve)
            .unwrap_or_default();
        let evaluation = self.working_evaluation(document);
        let model = evaluation
            .models
            .instances
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(id) && instance.has_geometry());
        let bounds_source = if !entity.solids.is_empty() {
            "brush"
        } else if model.is_some() {
            "model"
        } else if specialized_light(entity) {
            "specialized_light"
        } else if appearance.invalid_size {
            "invalid_fgd_size"
        } else if appearance.world_bounds(entity).is_some() {
            "fgd_size"
        } else {
            "default_helper"
        };
        let bounds = model
            .map(|instance| instance.bounds())
            .or_else(|| {
                (bounds_source == "fgd_size")
                    .then(|| appearance.world_bounds(entity))
                    .flatten()
            })
            .or_else(|| map_core::query::entity_bounds(entity));
        let mut status = match bounds_source {
            "fgd_size" => {
                let [mins, maxs] = appearance.local_bounds.unwrap();
                let size = maxs - mins;
                format!(
                    "FGD size: {} × {} × {} units; world-aligned editor box",
                    size.x, size.y, size.z
                )
            }
            "invalid_fgd_size" => "Invalid FGD size; generic editor marker".into(),
            "specialized_light" => "Specialized light helper".into(),
            "model" => "Loaded model geometry".into(),
            "brush" => "Brush geometry".into(),
            _ => "Default editor helper box".into(),
        };
        if !appearance.errors.is_empty() {
            status += &format!(". FGD helper error: {}", appearance.errors.join("; "));
        }
        if !appearance.unsupported.is_empty() {
            status += &format!(
                ". Additional FGD helpers are not evaluated: {}",
                appearance.unsupported.join(", ")
            );
        }
        Ok(Report {
            status,
            bounds_source,
            invalid_size: appearance.invalid_size,
            world_bounds: bounds.map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]),
            local_bounds: appearance
                .local_bounds
                .map(|bounds| bounds.map(|point| point.to_array())),
            class_color: appearance.color,
            errors: appearance.errors,
            unsupported: appearance.unsupported,
            definition_sources: self
                .definitions
                .sources
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            limits: "size() is an origin-relative, world-aligned editor box in Source units; angles and modelscale do not transform it. This is not an engine collision hull. Three integer dimensions or six ordered integer coordinates, with local box coordinates within ±65536; positive volume required. Malformed definitions retain the default box with a visible diagnostic. Includes, inheritance and extensions resolve before evaluation; per-helper file/line provenance is not retained. Other FGD helper declarations are not evaluated here; existing authored model, light and rope previews remain independent.",
        })
    }
}

#[cfg(test)]
mod tests;
