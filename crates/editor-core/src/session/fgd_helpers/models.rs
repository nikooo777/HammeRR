use std::collections::BTreeMap;

use map_core::Document;
#[cfg(test)]
use map_core::ObjectId;
use render::models::Instance;
use serde::Serialize;

use super::super::{Session, working_region::Evaluation};

const MAX_GUIDES: usize = 64;
const MAX_VERTICES: usize = 65_536;
const MAX_INDICES: usize = 393_216;
const MAX_PATH_BYTES: usize = 256;
pub(super) const LIMITS: &str = "One literal studio(models/*.mdl) or studioprop(models/*.mdl) declaration on a visible generic point entity; inherited declarations use the loaded catalog. Editor mesh only: ordinary helper bounds, picking, selection, framing, placement, snapping and working-region membership stay unchanged. Visual bounds are reported separately. Shared mounted model cache, LOD0 static/reference pose, authored angles/legacy angle, skin/body and unit-modelscale guards; no animation or engine collision claim. At most 64 eligible source evaluations, 65536 vertices and 393216 indices per scene in entity-ID order, within the existing model cache budget. Paths cap at 256 bytes and errors at 512 bytes. Authored model fields, model properties, specialized lights/ropes, sprite/decal/overlay helpers and brush sources retain precedence. Multiple declarations are ambiguous; zero-argument, property-driven and other helper forms remain unsupported. Reload models refreshes these shared assets; definition reload re-evaluates declarations. Deliberately untextured flat_editor meshes use the existing editor material and geometry renderer; no guide VMT/VTF reads, shader fidelity or animated-pose claim. Guide-only cache entries defer material-name resolution and remain excluded from material reload prefetch; a later ordinary model preview can resolve bindings on the same geometry. Authored rendercolor tints the flat mesh. Origins and complete visual bounds must remain finite within ±65536 units.";

#[derive(Debug, Serialize)]
pub struct Report {
    pub source: &'static str,
    pub helper: String,
    pub literal: String,
    pub literal_truncated: bool,
    pub path: Option<String>,
    pub declaration_supported: bool,
    pub state: &'static str,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
    pub error: Option<String>,
    pub visual_bounds: Option<[[f64; 3]; 2]>,
    pub pose: Option<&'static str>,
    pub skin: Option<usize>,
    pub body: Option<u32>,
    pub material_policy: &'static str,
}

fn preview(value: &str, maximum: usize) -> String {
    let mut end = value.len().min(maximum);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

impl Session {
    fn fgd_model_rows(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
        enabled: bool,
    ) -> BTreeMap<u32, (Report, Option<Instance>)> {
        let mut rows = BTreeMap::new();
        let mut preceding_assets = Vec::new();
        let mut evaluated = 0;
        let mut vertices = 0;
        let mut indices = 0;
        let context = super::classification::Context::new(evaluation, self.selection());
        let entities = context.entities(document, inspected, false);
        for (id, entity) in entities {
            if inspected.is_some_and(|target| id > target) {
                break;
            }
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let declarations = class
                .helpers
                .iter()
                .filter(|helper| ["studio", "studioprop"].contains(&helper.name.as_str()))
                .take(2)
                .collect::<Vec<_>>();
            let Some(first) = declarations.first() else {
                continue;
            };
            let facts = context.facts(entity, class);
            let literal = first.arguments.first().map_or("", String::as_str);
            let mut report = Report {
                source: "fgd_literal",
                helper: first.name.clone(),
                literal: preview(literal, MAX_PATH_BYTES),
                literal_truncated: literal.len() > MAX_PATH_BYTES,
                path: None,
                declaration_supported: false,
                state: "unsupported",
                displayed: false,
                suppression_reason: None,
                error: None,
                visual_bounds: None,
                pose: None,
                skin: None,
                body: None,
                material_policy: "flat_editor",
            };
            let path = if declarations.len() != 1 {
                Err("Multiple studio/studioprop declarations are ambiguous".into())
            } else if first.arguments.len() != 1 || literal.is_empty() {
                Err("Only one literal models/*.mdl argument is supported".into())
            } else if literal.len() > MAX_PATH_BYTES || literal.trim() != literal {
                Err("Literal model paths must be exact and at most 256 bytes".into())
            } else {
                render::models::model_path(literal)
            };
            if let Ok(path) = path.as_ref() {
                report.path = Some(path.clone());
                report.declaration_supported = true;
                report.state = "not_loaded";
            } else if let Err(error) = path.as_ref() {
                report.error = Some(error.clone());
            }
            report.suppression_reason = if !facts.point {
                Some("brush_or_nonpoint")
            } else if facts.light {
                Some("specialized_light")
            } else if facts.rope {
                Some("specialized_rope")
            } else if facts.authored_model {
                Some("authored_model")
            } else if facts.declared_model {
                Some("model_property")
            } else if facts.other_specialized {
                Some("specialized_helper")
            } else if facts.invalid_size {
                Some("invalid_fgd_size")
            } else if facts.hidden {
                Some("authored_hidden")
            } else if facts.outside {
                Some("outside_working_region")
            } else if !facts.valid_origin {
                Some("invalid_origin")
            } else if !enabled {
                Some("view_disabled")
            } else if evaluated >= MAX_GUIDES {
                Some("scene_model_limit")
            } else {
                None
            };
            let mut displayed = None;
            if report.declaration_supported && report.suppression_reason.is_none() {
                evaluated += 1;
                let result = self
                    .models
                    .borrow_mut()
                    .get_geometry(&self.game, report.path.as_deref().unwrap())
                    .and_then(|asset| Instance::editor_guide(entity, asset));
                match result {
                    Ok(instance) if instance.has_geometry() => {
                        let vertex_count = instance.asset.model.vertices.len();
                        let index_count = instance
                            .asset
                            .model
                            .parts_for_body(instance.body)
                            .map(|part| part.indices.len())
                            .sum::<usize>();
                        if vertices + vertex_count > MAX_VERTICES
                            || indices + index_count > MAX_INDICES
                        {
                            report.state = "unavailable";
                            report.suppression_reason = Some("scene_geometry_limit");
                            report.error = Some(
                                "FGD model guides exceed the shared scene geometry budget".into(),
                            );
                        } else {
                            vertices += vertex_count;
                            indices += index_count;
                            let (mins, maxs) = instance.bounds();
                            report.visual_bounds = Some([mins.to_array(), maxs.to_array()]);
                            report.pose = Some(if instance.asset.model.header.is_static_prop() {
                                "static"
                            } else {
                                "reference"
                            });
                            report.skin = Some(instance.skin);
                            report.body = Some(instance.body);
                            report.state = "ready";
                            report.displayed = true;
                            displayed = Some(instance);
                        }
                    }
                    Ok(_) => {
                        report.state = "unavailable";
                        report.error = Some("The selected model body has no geometry".into());
                    }
                    Err(error) => {
                        report.state = "unavailable";
                        report.error = Some(error);
                    }
                }
            }
            report.error = report.error.map(|error| preview(&error, 512));
            if inspected.is_some()
                && inspected != Some(id)
                && let Some(instance) = &displayed
            {
                preceding_assets.push(instance.asset.clone());
            }
            if inspected == Some(id) || inspected.is_none() && displayed.is_some() {
                rows.insert(id, (report, displayed));
            }
        }
        rows
    }

    pub(super) fn inspected_fgd_model(&self, id: u32) -> Option<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        self.fgd_model_rows(
            document,
            &evaluation,
            Some(id),
            self.view().entity_model_guides,
        )
        .remove(&id)
        .map(|(report, _)| report)
    }

    pub(in crate::session) fn fgd_model_guides(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        enabled: bool,
    ) -> Vec<Instance> {
        if !enabled {
            return Vec::new();
        }
        self.fgd_model_rows(document, evaluation, None, enabled)
            .into_values()
            .filter_map(|(_, instance)| instance)
            .collect()
    }
}

#[cfg(test)]
mod tests;
