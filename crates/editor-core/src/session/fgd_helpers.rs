use std::collections::{BTreeMap, HashSet};

use map_core::entity_helpers::{Appearance, specialized_light};
use map_core::query::{Hit, Ray};
use map_core::{Document, ObjectId};
use serde::Serialize;

use super::{Session, models::Models, working_region::Evaluation};

mod icons;
mod lines;
mod projectors;
mod spheres;

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
    pub spheres: Vec<spheres::Report>,
    pub icon_sprite: Option<render::icons::Report>,
    pub icon_limits: &'static str,
    pub sphere_limits: &'static str,
    pub lines: Vec<lines::Report>,
    pub line_limits: &'static str,
    pub source_projector: Option<projectors::Report>,
    pub projector_limits: &'static str,
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
        let icon_sprite = self.inspected_icon(id);
        if let Some(icon) = &icon_sprite {
            status += &format!(
                ". FGD icon sprite: {}; {}",
                if icon.displayed {
                    "displayed"
                } else {
                    icon.suppression_reason.unwrap_or(icon.status)
                },
                icon.material
            );
            if icon.status == "ready" {
                status += &format!(
                    " ({}×{}; live-camera unlit RGBA; box bounds retained)",
                    icon.width, icon.height
                );
            }
            if let Some(source) = &icon.source {
                status += &format!(". Icon VMT source: {}", source.summary());
            }
            if let Some(source) = &icon.texture_source {
                status += &format!(". Icon VTF source: {}", source.summary());
            }
            if let Some(error) = &icon.error {
                status += &format!(" ({error})");
            }
        }
        let spheres = self.inspected_spheres(id);
        let lines = self.inspected_lines(id);
        let source_projector = self.inspected_projector(id);
        if let Some(projector) = &source_projector {
            let state = if projector.displayed {
                "displayed"
            } else {
                projector.suppression_reason.unwrap_or(projector.state)
            };
            status += &format!(
                ". Source projector guide: {state}; Source Pitch Yaw Roll, positive pitch down; FGD frustum() pitch_scale is not evaluated"
            );
            if !projector.helper_pitch_scales.is_empty() {
                let scales = projector
                    .helper_pitch_scales
                    .iter()
                    .take(3)
                    .map(|scale| scale.raw.as_deref().unwrap_or("unsupported helper form"))
                    .collect::<Vec<_>>()
                    .join(", ");
                status += &format!(" (declared: {scales})");
            }
            if let Some(defaults) = projector.default_summary() {
                status += &format!(". {defaults}");
            }
            if let Some(error) = &projector.error {
                status += &format!(" ({error})");
            }
        } else if entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("env_projectedtexture"))
        {
            status += ". Source projector guide unavailable: loaded FGD definition required";
        }
        if !spheres.is_empty() {
            let displayed = spheres.iter().filter(|sphere| sphere.displayed).count();
            let unavailable = spheres
                .iter()
                .filter(|sphere| sphere.state != "ready")
                .count();
            let suppressed = spheres.len() - displayed - unavailable;
            status += &format!(
                ". Sphere guides: {displayed} displayed, {suppressed} suppressed, {unavailable} unavailable; authored/FGD scalar guides, not runtime influence bounds"
            );
            if spheres.iter().any(|sphere| sphere.implicit_property) {
                status += ". sphere() uses the implicit radius binding";
            }
        }
        if !lines.is_empty() {
            let displayed = lines.iter().filter(|line| line.displayed).count();
            let unavailable = lines.iter().filter(|line| line.state != "ready").count();
            status += &format!(
                ". Line guides: {displayed} displayed, {} suppressed, {unavailable} unavailable",
                lines.len() - displayed - unavailable
            );
            for line in lines.iter().filter(|line| !line.displayed).take(3) {
                let properties = line
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.property.as_deref().unwrap_or("self"))
                    .collect::<Vec<_>>()
                    .join(" → ");
                let state = if line.state == "ready" {
                    line.suppression_reason.unwrap_or("suppressed")
                } else {
                    line.state
                };
                status += &format!(". {properties}: {}", state.replace('_', " "));
                if let Some(error) = &line.error {
                    status += &format!(" ({error})");
                }
            }
        }
        Ok(Report {
            status,
            spheres,
            sphere_limits: spheres::LIMITS,
            icon_sprite,
            icon_limits: render::icons::LIMITS,
            lines,
            line_limits: lines::LIMITS,
            source_projector,
            projector_limits: projectors::LIMITS,
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
            limits: "size() is an origin-relative, world-aligned editor box in Source units; angles and modelscale do not transform it. This is not an engine collision hull. Three integer dimensions or six ordered integer coordinates, with local box coordinates within ±65536; positive volume required. Malformed definitions retain the default box with a visible diagnostic. Includes, inheritance and extensions resolve before evaluation; per-helper file/line provenance is not retained. Property sphere and literal targetname line guides are reported separately and never change these object bounds; other FGD helper declarations are not evaluated here. Existing authored model, light and rope previews remain independent.",
        })
    }
}

#[cfg(test)]
mod tests;
