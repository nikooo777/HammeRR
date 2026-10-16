use std::collections::{BTreeMap, HashSet};

use map_core::{
    Document, ObjectId,
    entity_helpers::{
        specialized_light,
        spheres::{self, Sphere},
    },
};
use serde::Serialize;
use source_formats::fgd::ClassKind;

use super::super::{Session, working_region::Evaluation};

pub(super) const MAX_DISPLAYED_SPHERES: usize = 128;
pub(super) const LIMITS: &str = "Selected visible generic point entities only; sphere(property) uses authored values or labeled FGD defaults. sphere() implicitly binds the declared numeric radius property and reports implicit_property; equivalent effective property names share one guide in declaration order. Three world-axis guide rings, 32 segments each; at most 16 declarations per entity and 128 displayed spheres in entity-ID order. Property-name/value previews are bounded to 128/256 bytes; oversized values are never parsed. Positive finite native radii up to 65536 units. Nonpositive values have no finite guide and retain entity-specific runtime meaning. Guides do not affect collision, picking, placement, framing or working regions. Brush, model helpers including missing assets, specialized-light and rope helpers are outside this scope. Model fade distances may use screen pixels rather than world units.";

#[derive(Debug, Serialize)]
pub struct Report {
    pub property: String,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub property_truncated: bool,
    pub implicit_property: bool,
    pub value_source: &'static str,
    pub property_type: Option<String>,
    pub radius: Option<f64>,
    pub center: Option<[f64; 3]>,
    pub guide_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
    #[serde(skip)]
    geometry: Sphere,
}

impl Report {
    fn new(sphere: Sphere, reason: Option<&'static str>) -> Self {
        Self {
            property: sphere.property.clone(),
            raw: sphere.raw.clone(),
            raw_truncated: sphere.raw_truncated,
            property_truncated: sphere.property_truncated,
            implicit_property: sphere.implicit_property,
            value_source: sphere.value_source,
            property_type: sphere.property_type.clone(),
            radius: sphere.radius,
            center: sphere.center,
            guide_bounds: sphere.guide_bounds,
            state: sphere.state,
            error: sphere.error.clone(),
            displayed: reason.is_none(),
            suppression_reason: reason,
            geometry: sphere,
        }
    }
}

impl Session {
    fn sphere_reports(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> BTreeMap<u32, Vec<Report>> {
        let selected = self.selection();
        let models = evaluation
            .models
            .instances
            .iter()
            .filter(|instance| instance.has_geometry())
            .map(|instance| instance.id)
            .collect::<HashSet<_>>();
        let entities = document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                selected.contains(&ObjectId::Entity(entity.id)) || inspected == Some(entity.id)
            })
            .map(|entity| (entity.id, entity))
            .collect::<BTreeMap<_, _>>();
        let mut displayed = 0;
        let mut result = BTreeMap::new();
        for (id, entity) in entities {
            let Some(class) = entity
                .classname()
                .and_then(|class| self.definitions().class(class))
            else {
                continue;
            };
            let object = ObjectId::Entity(id);
            let reason = if class.kind != ClassKind::Point || !entity.solids.is_empty() {
                Some("brush_or_nonpoint")
            } else if specialized_light(entity) {
                Some("specialized_light")
            } else if models.contains(&object) {
                Some("loaded_model")
            } else if entity.classname().is_some_and(|class| {
                ["move_rope", "keyframe_rope"]
                    .iter()
                    .any(|rope| class.eq_ignore_ascii_case(rope))
            }) {
                Some("specialized_rope")
            } else if class
                .helpers
                .iter()
                .any(|helper| ["studio", "studioprop", "model"].contains(&helper.name.as_str()))
                || class
                    .property("model")
                    .is_some_and(|property| property.kind == "studio")
                || entity.properties.get("model").is_some()
            {
                Some("model_helper")
            } else if entity.hidden {
                Some("authored_hidden")
            } else if !evaluation.objects.contains(&object) {
                Some("outside_working_region")
            } else if !selected.contains(&object) {
                Some("not_selected")
            } else {
                None
            };
            let rows: Vec<_> = spheres::resolve(class, entity)
                .into_iter()
                .map(|sphere| {
                    let reason = reason.or_else(|| {
                        if sphere.state != "ready" {
                            Some("unavailable_radius")
                        } else if displayed == MAX_DISPLAYED_SPHERES {
                            Some("scene_guide_limit")
                        } else {
                            None
                        }
                    });
                    if reason.is_none() {
                        displayed += 1;
                    }
                    Report::new(sphere, reason)
                })
                .collect();
            if inspected == Some(id)
                || (inspected.is_none() && rows.iter().any(|row| row.displayed))
            {
                result.insert(id, rows);
            }
            if inspected == Some(id) || (inspected.is_none() && displayed == MAX_DISPLAYED_SPHERES)
            {
                break;
            }
        }
        result
    }

    pub(super) fn inspected_spheres(&self, id: u32) -> Vec<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        self.sphere_reports(document, &evaluation, Some(id))
            .remove(&id)
            .unwrap_or_default()
    }

    pub(in crate::session) fn append_fgd_sphere_guides(&self, mesh: &mut render::SceneMesh) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        for row in self
            .sphere_reports(document, &evaluation, None)
            .into_values()
            .flatten()
            .filter(|row| row.displayed)
        {
            for segment in row.geometry.lines() {
                for point in segment {
                    mesh.lines.push(render::scene::LineVertex {
                        position: glam::DVec3::from_array(point).as_vec3().to_array(),
                        color: [80, 220, 255, 255],
                    });
                }
            }
        }
    }
}
