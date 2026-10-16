use std::collections::{BTreeMap, HashSet};

use map_core::{Document, ObjectId, entity_helpers::specialized_light};
use render::icons::{Instance, Report};
use source_formats::fgd::ClassKind;

use super::super::{Session, working_region::Evaluation};

impl Session {
    fn icon_rows(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> BTreeMap<u32, (Report, Option<Instance>)> {
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
            .map(|entity| (entity.id, entity))
            .collect::<BTreeMap<_, _>>();
        let mut displayed = 0;
        let mut rows = BTreeMap::new();
        for (id, entity) in entities {
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let declarations = class
                .helpers
                .iter()
                .filter(|helper| helper.name == "iconsprite")
                .take(2)
                .collect::<Vec<_>>();
            if declarations.is_empty() {
                continue;
            }
            let reason = if class.kind != ClassKind::Point || !entity.solids.is_empty() {
                Some("brush_or_nonpoint")
            } else if specialized_light(entity) {
                Some("specialized_light")
            } else if models.contains(&ObjectId::Entity(id)) {
                Some("loaded_model")
            } else if entity.classname().is_some_and(|name| {
                ["move_rope", "keyframe_rope"]
                    .iter()
                    .any(|special| name.eq_ignore_ascii_case(special))
            }) {
                Some("specialized_rope")
            } else if class.helpers.iter().any(|helper| {
                ["studio", "studioprop", "model", "sprite"].contains(&helper.name.as_str())
            }) || class.property("model").is_some()
                || entity.properties.get("model").is_some()
                || entity.classname().is_some_and(|name| {
                    ["env_sprite", "env_sprite_oriented"]
                        .iter()
                        .any(|special| name.eq_ignore_ascii_case(special))
                })
            {
                Some("model_or_sprite_helper")
            } else if evaluation
                .helpers
                .get(&ObjectId::Entity(id))
                .is_some_and(|appearance| appearance.invalid_size)
            {
                Some("invalid_fgd_size")
            } else if entity.hidden {
                Some("authored_hidden")
            } else if !evaluation.objects.contains(&ObjectId::Entity(id)) {
                Some("outside_working_region")
            } else if !entity
                .origin()
                .is_some_and(|origin| origin.is_finite() && origin.abs().max_element() <= 65536.0)
            {
                Some("invalid_origin")
            } else if displayed >= render::icons::MAX_INSTANCES {
                Some("scene_icon_limit")
            } else {
                None
            };
            if reason.is_some() && inspected != Some(id) {
                continue;
            }
            let (mut report, asset) = if declarations.len() != 1
                || declarations[0].arguments.len() != 1
                || declarations[0].arguments[0].is_empty()
            {
                (
                    Report::failure(
                        "",
                        "unsupported",
                        "Exactly one iconsprite declaration with one literal material argument is required",
                    ),
                    None,
                )
            } else if reason.is_some() {
                (
                    Report::failure(
                        &declarations[0].arguments[0],
                        "suppressed",
                        "Asset admission is deferred while this icon is suppressed",
                    ),
                    None,
                )
            } else {
                let asset = self
                    .materials
                    .icons
                    .load(&self.game, &declarations[0].arguments[0]);
                (asset.report.clone(), Some(asset))
            };
            report.suppression_reason =
                reason.or_else(|| (report.status != "ready").then_some("unavailable_icon"));
            report.displayed = report.suppression_reason.is_none();
            if report.displayed {
                displayed += 1;
            }
            let instance = if reason.is_none() {
                asset.map(|asset| Instance {
                    center: entity.origin().unwrap().as_vec3().to_array(),
                    asset,
                })
            } else {
                None
            };
            if inspected == Some(id) || inspected.is_none() {
                rows.insert(id, (report, instance));
            }
            if inspected == Some(id) {
                break;
            }
        }
        rows
    }
    pub(super) fn inspected_icon(&self, id: u32) -> Option<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        self.icon_rows(document, &evaluation, Some(id))
            .remove(&id)
            .map(|(report, _)| report)
    }
    pub(in crate::session) fn fgd_icon_instances(
        &self,
        document: &Document,
        evaluation: &Evaluation,
    ) -> BTreeMap<u32, Instance> {
        self.icon_rows(document, evaluation, None)
            .into_iter()
            .filter_map(|(id, (_, instance))| instance.map(|instance| (id, instance)))
            .collect()
    }
}

#[cfg(test)]
mod tests;
