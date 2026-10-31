use std::collections::{BTreeMap, HashSet};

use map_core::entity_helpers::{Appearance, specialized_light};
use map_core::query::{Hit, Ray};
use map_core::{Document, ObjectId};
use serde::Serialize;

use super::{Session, models::Models, working_region::Evaluation};

mod classification;
mod frustums;
mod icons;
mod lines;
mod models;
mod projectors;
mod spheres;
mod sprites;

#[derive(Debug, Serialize)]
pub struct Report {
    pub status: String,
    pub base_displayed: bool,
    pub authored_model_error: Option<String>,
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
    pub sprite_resource: Option<sprites::Report>,
    pub sprite_resource_limits: &'static str,
    pub model_guide: Option<models::Report>,
    pub model_guide_limits: &'static str,
    pub sphere_limits: &'static str,
    pub lines: Vec<lines::Report>,
    pub line_limits: &'static str,
    pub frustums: Vec<frustums::Report>,
    pub omitted_frustum_declarations: usize,
    pub frustum_limits: &'static str,
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
        let document = self.render_document();
        let entity = document
            .entity(id)
            .ok_or_else(|| format!("no entity:{id}"))?;
        let mut appearance = entity
            .classname()
            .and_then(|classname| self.definitions.class(classname))
            .map(Appearance::resolve)
            .unwrap_or_default();
        let model_guide = self.inspected_fgd_model(id);
        if let Some(guide) = &model_guide
            && guide.declaration_supported
        {
            appearance.unsupported.retain(|name| name != &guide.helper);
        }
        let (icon_sprite, sprite_resource) = self.inspected_images(id);
        if sprite_resource
            .as_ref()
            .is_some_and(|report| report.declaration_supported)
        {
            appearance.unsupported.retain(|name| name != "sprite");
        }
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
        let base_displayed = if bounds_source == "brush" {
            true
        } else if model.is_some() {
            self.view.authored_models
        } else {
            self.view.entity_helpers || !appearance.errors.is_empty()
        };
        if model.is_some() && !self.view.authored_models {
            status = "Authored model hidden; exact model bounds retained as a wire marker".into();
        } else if !base_displayed {
            status += ". Entity helpers hidden; selection outlines and picking bounds retained";
        }
        let authored_model_error = evaluation.models.issues.get(&id).cloned();
        if let Some(error) = &authored_model_error {
            status += &format!(". Authored model error: {error}");
        }
        if !appearance.errors.is_empty() {
            status += &format!(". FGD helper error: {}", appearance.errors.join("; "));
        }
        if !appearance.unsupported.is_empty() {
            status += &format!(
                ". Additional FGD helpers are not evaluated: {}",
                appearance.unsupported.join(", ")
            );
        }
        if let Some(guide) = &model_guide {
            status += &format!(
                ". FGD model guide: {}; {}({}); flat editor mesh only, ordinary helper bounds and picking",
                if guide.displayed {
                    "displayed"
                } else {
                    guide.suppression_reason.unwrap_or(guide.state)
                },
                guide.helper,
                guide.path.as_deref().unwrap_or(&guide.literal)
            );
            if let Some(error) = &guide.error {
                status += &format!(" ({error})");
            }
        }
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
        if let Some(sprite) = &sprite_resource {
            status += &format!(
                ". Sprite resource guide: {}; {}; {}; runtime color/FX/HDR unevaluated, box bounds retained",
                if sprite.displayed {
                    "displayed"
                } else {
                    sprite.suppression_reason.unwrap_or(sprite.state)
                },
                sprite.resolved_vmt.as_deref().unwrap_or("unresolved VMT"),
                if sprite.blend_preview.applied {
                    "authored Additive LDR editor image, SRC_ALPHA/ONE RGB"
                } else {
                    "raw independent live-camera unlit RGBA image"
                }
            );
            if sprite.blend_preview.requested {
                for input in [
                    &sprite.blend_preview.rendermode,
                    &sprite.blend_preview.rendercolor,
                    &sprite.blend_preview.renderamt,
                    &sprite.blend_preview.renderfx,
                ] {
                    status += &format!(
                        ". {} {}={}",
                        input.value_source,
                        input.property,
                        input.raw.as_deref().unwrap_or("missing")
                    );
                }
                if let Some(error) = &sprite.blend_preview.error {
                    status += &format!(". Additive guide unavailable: {error}");
                }
            }
            for input in [&sprite.model, &sprite.scale] {
                if input.value_source == "fgd_default" {
                    status += &format!(
                        ". FGD default {}={}",
                        input.property,
                        input.raw.as_deref().unwrap_or_default()
                    );
                }
            }
            if let Some(size) = sprite.world_size {
                status += &format!(" ({} × {} Source units)", size[0], size[1]);
            }
            if let Some(image) = &sprite.image {
                if let Some(source) = &image.source {
                    status += &format!(". Sprite VMT source: {}", source.summary());
                }
                if let Some(source) = &image.texture_source {
                    status += &format!(". Sprite VTF source: {}", source.summary());
                }
            }
            if let Some(error) = &sprite.error {
                status += &format!(" ({error})");
            }
        }
        let spheres = self.inspected_spheres(id);
        let lines = self.inspected_lines(id);
        let (frustums, omitted_frustum_declarations) = self.inspected_frustums(id);
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
        for frustum in &frustums {
            status += &format!(
                ". FGD frustum guide: {}; pitch_scale={} ({})",
                if frustum.displayed {
                    frustum.presentation
                } else {
                    frustum.suppression_reason.unwrap_or(frustum.state)
                },
                frustum
                    .pitch_scale
                    .map_or_else(|| "unavailable".into(), |v| v.to_string()),
                frustum.pitch_scale_source
            );
            let defaults = frustum
                .inputs
                .iter()
                .filter(|v| v.value_source == "fgd_default")
                .map(|v| v.property.as_str())
                .take(6)
                .collect::<Vec<_>>();
            if !defaults.is_empty() {
                status += &format!("; FGD defaults: {}", defaults.join(", "));
            }
            if let Some(error) = &frustum.error {
                status += &format!(" ({error})");
            }
        }
        if omitted_frustum_declarations > 0 {
            status += &format!(
                ". {omitted_frustum_declarations} further FGD frustum declarations omitted"
            );
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
            if lines.iter().any(|line| {
                line.endpoints
                    .iter()
                    .any(|endpoint| endpoint.lookup_property == Some("nodeid"))
            }) {
                status += "; authored nodeid bindings; no generated IDs";
            }
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
            base_displayed,
            authored_model_error,
            spheres,
            sphere_limits: spheres::LIMITS,
            icon_sprite,
            icon_limits: render::icons::LIMITS,
            sprite_resource,
            sprite_resource_limits: sprites::LIMITS,
            model_guide,
            model_guide_limits: models::LIMITS,
            lines,
            line_limits: lines::LIMITS,
            frustums,
            omitted_frustum_declarations,
            frustum_limits: frustums::LIMITS,
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
            limits: "size() is an origin-relative, world-aligned editor box in Source units; angles and modelscale do not transform it. This is not an engine collision hull. Three integer dimensions or six ordered integer coordinates, with local box coordinates within ±65536; positive volume required. Malformed definitions retain the default box with a visible diagnostic. Includes, inheritance and extensions resolve before evaluation; per-helper file/line provenance is not retained. Sphere, targetname line, literal icon and literal studio/studioprop model guides are reported separately and never change these object bounds; other FGD helper declarations are not evaluated here. Existing authored model, light and rope previews remain independent.",
        })
    }
}

impl Session {
    pub(super) fn helper_presentation_errors(
        &self,
        document: &Document,
        view: render::ViewOptions,
    ) -> BTreeMap<u32, String> {
        let evaluation = self.working_evaluation(document);
        let mut errors = evaluation.models.issues.clone();
        for (&object, appearance) in &evaluation.helpers {
            if let ObjectId::Entity(id) = object
                && !appearance.errors.is_empty()
            {
                errors
                    .entry(id)
                    .or_insert_with(|| appearance.errors.join("; "));
            }
        }
        for (id, error) in self
            .image_errors(document, &evaluation, view)
            .into_iter()
            .chain(self.model_guide_errors(document, &evaluation, view.entity_model_guides))
        {
            errors.entry(id).or_insert(error);
        }
        errors
    }
}
