use std::collections::BTreeMap;

use map_core::{
    Document,
    entity_helpers::projectors::{self, Projector},
};
use serde::Serialize;

use super::super::{Session, working_region::Evaluation};

pub(super) const MAX_DISPLAYED_PROJECTORS: usize = 128;
const MAX_SUMMARY_FIELDS: usize = 4;
const MAX_SUMMARY_CHARS: usize = 24;
pub(super) const LIMITS: &str = "Independent Source projector guide for selected visible env_projectedtexture point entities with loaded FGD definitions. Authored values precede labeled FGD defaults. Square horizontal/vertical FOV; near/far distances lie along the forward axis. Eight corners and twelve edges; at most 128 displayed projectors in entity-ID order. Values and helper pitch-scale previews cap at 256 bytes; at most 16 helper declarations are reported. Requires 0 < FOV < 180 and 0 < nearz < farz <= 65536, with eight pairwise distinct native corners, nonzero near/far plane areas and each coordinate offset within ±65536 units of the origin; invalid values are never clamped or replaced. Target, parentname, legacy angle and nonzero cameraspace branches are unavailable. RGB colors the guide; optional intensity is reported without exposure/opacity conversion. FGD frustum() and its pitch_scale are not evaluated. This guide does not preview projected textures, shadows, illumination or runtime state, and does not affect bounds, picking, selection, placement, framing or working regions. Loaded models, model helpers, brush sources and existing light/rope helpers retain precedence.";

#[derive(Debug, Serialize)]
pub struct Input {
    pub property: &'static str,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PitchScale {
    pub argument_count: usize,
    pub declaration_state: &'static str,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub evaluated: bool,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub orientation_policy: &'static str,
    pub helper_pitch_scale_evaluated: bool,
    pub helper_pitch_scales: Vec<PitchScale>,
    pub omitted_helper_declarations: usize,
    pub inputs: Vec<Input>,
    pub fov_degrees: Option<f64>,
    pub near_z: Option<f64>,
    pub far_z: Option<f64>,
    pub color: Option<[u8; 3]>,
    pub intensity: Option<f64>,
    pub origin: Option<[f64; 3]>,
    pub angles: Option<[f64; 3]>,
    pub corners: Option<[[f64; 3]; 8]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
    #[serde(skip)]
    geometry: Projector,
}

impl Report {
    pub(super) fn default_summary(&self) -> Option<String> {
        let defaults = self
            .inputs
            .iter()
            .filter(|input| input.value_source == "fgd_default")
            .collect::<Vec<_>>();
        if defaults.is_empty() {
            return None;
        }
        let mut fields = defaults
            .iter()
            .take(MAX_SUMMARY_FIELDS)
            .map(|input| {
                let raw = input.raw.as_deref().unwrap_or_default();
                let mut preview = raw.chars().take(MAX_SUMMARY_CHARS).collect::<String>();
                if raw.chars().nth(MAX_SUMMARY_CHARS).is_some() {
                    preview.push('…');
                }
                format!("{}={preview}", input.property)
            })
            .collect::<Vec<_>>();
        if defaults.len() > MAX_SUMMARY_FIELDS {
            fields.push(format!("+{} more", defaults.len() - MAX_SUMMARY_FIELDS));
        }
        Some(format!(
            "FGD defaults: {}; engine defaults may differ",
            fields.join(", ")
        ))
    }

    fn new(projector: Projector, reason: Option<&'static str>) -> Self {
        Self {
            orientation_policy: projectors::ORIENTATION_POLICY,
            helper_pitch_scale_evaluated: false,
            helper_pitch_scales: projector
                .helper_pitch_scales
                .iter()
                .map(|scale| PitchScale {
                    argument_count: scale.argument_count,
                    declaration_state: scale.declaration_state,
                    raw: scale.raw.clone(),
                    raw_truncated: scale.raw_truncated,
                    evaluated: false,
                })
                .collect(),
            omitted_helper_declarations: projector.omitted_helper_declarations,
            inputs: projector
                .inputs
                .iter()
                .map(|input| Input {
                    property: input.property,
                    property_type: input.property_type.clone(),
                    raw: input.raw.clone(),
                    raw_truncated: input.raw_truncated,
                    value_source: input.value_source,
                    error: input.error.clone(),
                })
                .collect(),
            fov_degrees: projector.fov_degrees,
            near_z: projector.near_z,
            far_z: projector.far_z,
            color: projector.color,
            intensity: projector.intensity,
            origin: projector.origin,
            angles: projector.angles,
            corners: projector.corners,
            guide_bounds: projector.guide_bounds,
            state: projector.state,
            error: projector.error.clone(),
            displayed: reason.is_none(),
            suppression_reason: reason,
            geometry: projector,
        }
    }
}

impl Session {
    pub(super) fn projector_reports(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> BTreeMap<u32, Report> {
        let context = super::classification::Context::new(evaluation, self.selection());
        let entities = context.entities_where(document, inspected, true, |entity| {
            entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("env_projectedtexture"))
        });
        let mut displayed = 0;
        let mut result = BTreeMap::new();
        for (id, entity) in entities {
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let facts = context.facts(entity, class);
            let geometry = projectors::resolve(class, entity);
            let reason = if !facts.point {
                Some("brush_or_nonpoint")
            } else if facts.loaded {
                Some("loaded_model")
            } else if facts.model_helper {
                Some("model_helper")
            } else if facts.hidden {
                Some("authored_hidden")
            } else if facts.editor_hidden {
                Some("editor_hidden")
            } else if facts.outside {
                Some("outside_working_region")
            } else if !facts.selected {
                Some("not_selected")
            } else if geometry.state != "ready" {
                Some("unavailable_projector")
            } else if displayed == MAX_DISPLAYED_PROJECTORS {
                Some("scene_guide_limit")
            } else {
                None
            };
            if reason.is_none() {
                displayed += 1;
            }
            if inspected == Some(id) || reason.is_none() {
                result.insert(id, Report::new(geometry, reason));
            }
            if inspected == Some(id)
                || (inspected.is_none() && displayed == MAX_DISPLAYED_PROJECTORS)
            {
                break;
            }
        }
        result
    }

    pub(super) fn inspected_projector(&self, id: u32) -> Option<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let mut row = self
            .projector_reports(document, &evaluation, Some(id))
            .remove(&id);
        if !self.view.entity_helpers
            && let Some(row) = &mut row
            && row.displayed
        {
            row.displayed = false;
            row.suppression_reason = Some("view_disabled");
        }
        row
    }

    pub(in crate::session) fn append_source_projector_guides(&self, mesh: &mut render::SceneMesh) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        for row in self
            .projector_reports(document, &evaluation, None)
            .into_values()
            .filter(|row| row.displayed)
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
