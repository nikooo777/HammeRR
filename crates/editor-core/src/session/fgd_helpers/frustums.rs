use std::collections::BTreeMap;

use map_core::{
    Document,
    entity_helpers::frustums::{self, Frustum},
};
use serde::Serialize;

use super::super::{Session, working_region::Evaluation};

pub(super) const MAX_DISPLAYED_FRUSTUMS: usize = 128;
pub(super) const LIMITS: &str = "Selected visible generic point FGD frustum authoring guides use proven installed Hammer2009 bindings and T(origin)*Rz(yaw)*Ry(-pitch_scale*raw_pitch)*Rx(roll). Zero to five arguments; omitted bindings _fov/_NearPlane/_FarPlane/_light and omitted pitch_scale=1. Distinct nonreserved binding names require 1–79 UTF-8 bytes; first three bind declared float/integer properties, fourth color255. Authored values precede labeled FGD defaults; missing values never use constructor substitutes. At most 16 declarations per entity and 128 displayed guides in entity-ID/declaration order; values/previews cap at 256 bytes. Nominal FOV 0.02–178 degrees, 0 < near < far <= 65536 and finite native scale/product/corners, with pairwise distinct corners and nonzero native plane areas; no clamp fallback. Fourth color component is retained as intensity, not opacity. Equal displayed Source projector corners/RGB share one presentation; otherwise that specialized guide retains precedence. Models, specialized lights/ropes/sprites, visibility and working regions retain precedence. Ordinary bounds, picking, framing and document/history stay unchanged. No projected texture, illumination, shadow, parent/runtime-state or universal Hammer++ compatibility claim.";

#[derive(Debug, Serialize)]
pub struct Input {
    pub property: String,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub declaration_index: usize,
    pub arguments: Vec<String>,
    pub arguments_truncated: bool,
    pub inputs: Vec<Input>,
    pub orientation_policy: &'static str,
    pub pitch_scale_evaluated: bool,
    pub pitch_scale: Option<f64>,
    pub pitch_scale_source: &'static str,
    pub fov_degrees: Option<f64>,
    pub near_z: Option<f64>,
    pub far_z: Option<f64>,
    pub color: Option<[u8; 3]>,
    pub intensity: Option<f64>,
    pub origin: Option<[f64; 3]>,
    pub angles: Option<[f64; 3]>,
    pub effective_angles: Option<[f64; 3]>,
    pub corners: Option<[[f64; 3]; 8]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
    pub displayed: bool,
    pub presentation: &'static str,
    pub suppression_reason: Option<&'static str>,
    #[serde(skip)]
    geometry: Frustum,
}

impl Report {
    fn new(index: usize, geometry: Frustum, reason: Option<&'static str>, shared: bool) -> Self {
        Self {
            declaration_index: index,
            arguments: geometry.arguments.clone(),
            arguments_truncated: geometry.arguments_truncated,
            inputs: geometry
                .inputs
                .iter()
                .map(|v| Input {
                    property: v.property.clone(),
                    property_type: v.property_type.clone(),
                    raw: v.raw.clone(),
                    raw_truncated: v.raw_truncated,
                    value_source: v.value_source,
                    error: v.error.clone(),
                })
                .collect(),
            orientation_policy: frustums::ORIENTATION_POLICY,
            pitch_scale_evaluated: geometry.pitch_scale.is_some(),
            pitch_scale: geometry.pitch_scale,
            pitch_scale_source: geometry.pitch_scale_source,
            fov_degrees: geometry.fov_degrees,
            near_z: geometry.near_z,
            far_z: geometry.far_z,
            color: geometry.color,
            intensity: geometry.intensity,
            origin: geometry.origin,
            angles: geometry.angles,
            effective_angles: geometry.effective_angles,
            corners: geometry.corners,
            guide_bounds: geometry.guide_bounds,
            state: geometry.state,
            error: geometry.error.clone(),
            displayed: reason.is_none(),
            presentation: if reason.is_some() {
                "suppressed"
            } else if shared {
                "shared_with_source_projector"
            } else {
                "separate"
            },
            suppression_reason: reason,
            geometry,
        }
    }
}

impl Session {
    fn frustum_reports(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> (BTreeMap<u32, Vec<Report>>, usize) {
        let context = super::classification::Context::new(evaluation, self.selection());
        let source = self.projector_reports(document, evaluation, None);
        let entities = context.entities(document, inspected, true);
        let mut displayed = 0;
        let mut omitted = 0;
        let mut result: BTreeMap<u32, Vec<Report>> = BTreeMap::new();
        for (id, entity) in entities {
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let facts = context.facts(entity, class);
            let helpers = class
                .helpers
                .iter()
                .filter(|helper| helper.name == "frustum");
            if inspected == Some(id) {
                omitted = helpers
                    .clone()
                    .count()
                    .saturating_sub(frustums::MAX_DECLARATIONS);
            }
            for (index, helper) in helpers.take(frustums::MAX_DECLARATIONS).enumerate() {
                let geometry = frustums::resolve(class, entity, helper);
                let shared = source.get(&id).is_some_and(|source| {
                    source.displayed
                        && geometry.state == "ready"
                        && source.corners == geometry.corners
                        && source.color == geometry.color
                });
                let reason = if !facts.point {
                    Some("brush_or_nonpoint")
                } else if facts.loaded {
                    Some("loaded_model")
                } else if facts.light || facts.rope || facts.sprite {
                    Some("specialized_helper")
                } else if facts.model_helper || facts.sprite_helper {
                    Some("model_or_sprite_helper")
                } else if facts.hidden {
                    Some("authored_hidden")
                } else if facts.editor_hidden {
                    Some("editor_hidden")
                } else if facts.outside {
                    Some("outside_working_region")
                } else if !facts.selected {
                    Some("not_selected")
                } else if geometry.state != "ready" {
                    Some("unavailable_frustum")
                } else if entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("env_projectedtexture"))
                    && !shared
                {
                    Some("source_projector_precedence")
                } else if displayed == MAX_DISPLAYED_FRUSTUMS {
                    Some("scene_guide_limit")
                } else {
                    None
                };
                if reason.is_none() {
                    displayed += 1;
                }
                if inspected == Some(id) || reason.is_none() {
                    result
                        .entry(id)
                        .or_default()
                        .push(Report::new(index, geometry, reason, shared));
                }
            }
            if inspected == Some(id) {
                break;
            }
            if inspected.is_none() && displayed == MAX_DISPLAYED_FRUSTUMS {
                break;
            }
        }
        (result, omitted)
    }

    pub(super) fn inspected_frustums(&self, id: u32) -> (Vec<Report>, usize) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let (mut reports, omitted) = self.frustum_reports(document, &evaluation, Some(id));
        let mut rows = reports.remove(&id).unwrap_or_default();
        if !self.view.entity_helpers {
            for row in &mut rows {
                if row.displayed {
                    row.displayed = false;
                    row.suppression_reason = Some("view_disabled");
                }
            }
        }
        (rows, omitted)
    }

    pub(in crate::session) fn append_fgd_frustum_guides(&self, mesh: &mut render::SceneMesh) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        for row in self
            .frustum_reports(document, &evaluation, None)
            .0
            .into_values()
            .flatten()
            .filter(|row| row.displayed && row.presentation == "separate")
        {
            let [r, g, b] = row.color.unwrap();
            for segment in row.geometry.lines() {
                for point in segment {
                    mesh.lines.push(render::scene::LineVertex {
                        position: glam::DVec3::from_array(point).as_vec3().to_array(),
                        color: [r, g, b, 255],
                    });
                }
            }
        }
    }
}
