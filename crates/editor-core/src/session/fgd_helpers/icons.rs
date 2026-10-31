use std::collections::BTreeMap;

use map_core::Document;

use render::icons::{Instance, Report};

use super::super::{Session, working_region::Evaluation};

struct Row {
    icon: Option<Report>,
    sprite: Option<super::sprites::Report>,
    instance: Option<Instance>,
}

impl Session {
    fn image_rows(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
        view: render::ViewOptions,
        admit: bool,
    ) -> BTreeMap<u32, Row> {
        let context = super::classification::Context::new(evaluation, self.selection());
        let entities = context.entities(document, inspected, false);
        let mut displayed = 0;
        let mut rows = BTreeMap::new();
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
            let (sprite, sprite_instance) = if crate::sprite_resource::is_source(Some(class))
                || class.helpers.iter().any(|helper| helper.name == "sprite")
            {
                let facts = context.facts(entity, class);
                let (report, instance) = super::sprites::row(
                    self,
                    class,
                    entity,
                    &facts,
                    (view.sprite_resources, view.additive_sprite_guides, admit),
                    displayed,
                );
                if report.displayed {
                    displayed += 1;
                }
                (Some(report), instance)
            } else {
                (None, None)
            };
            let declarations = class
                .helpers
                .iter()
                .filter(|helper| helper.name == "iconsprite")
                .take(2)
                .collect::<Vec<_>>();
            if declarations.is_empty() {
                if sprite.is_some() && (inspected == Some(id) || inspected.is_none()) {
                    rows.insert(
                        id,
                        Row {
                            icon: None,
                            sprite,
                            instance: sprite_instance,
                        },
                    );
                }
                continue;
            }
            let facts = context.facts(entity, class);
            let reason = if !facts.point {
                Some("brush_or_nonpoint")
            } else if facts.light {
                Some("specialized_light")
            } else if facts.loaded {
                Some("loaded_model")
            } else if facts.rope {
                Some("specialized_rope")
            } else if facts.model_helper
                || facts.sprite_helper
                || facts.declared_model
                || facts.sprite
            {
                Some("model_or_sprite_helper")
            } else if facts.invalid_size {
                Some("invalid_fgd_size")
            } else if facts.hidden {
                Some("authored_hidden")
            } else if facts.editor_hidden {
                Some("editor_hidden")
            } else if facts.outside {
                Some("outside_working_region")
            } else if !facts.valid_origin {
                Some("invalid_origin")
            } else if !view.entity_images {
                Some("view_disabled")
            } else if displayed >= render::icons::MAX_INSTANCES {
                Some("scene_icon_limit")
            } else {
                None
            };
            if reason.is_some() && reason != Some("view_disabled") && inspected != Some(id) {
                if sprite.is_some() && inspected.is_none() {
                    rows.insert(
                        id,
                        Row {
                            icon: None,
                            sprite,
                            instance: sprite_instance,
                        },
                    );
                }
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
                let cached = self
                    .materials
                    .icons
                    .cached_report(&declarations[0].arguments[0])
                    .filter(|report| report.error.is_some());
                (
                    cached.unwrap_or_else(|| {
                        Report::failure(
                            &declarations[0].arguments[0],
                            "suppressed",
                            "Asset admission is deferred while this icon is suppressed",
                        )
                    }),
                    None,
                )
            } else if admit {
                let asset = self
                    .materials
                    .icons
                    .load(&self.game, &declarations[0].arguments[0]);
                (asset.report.clone(), Some(asset))
            } else {
                (
                    self.materials
                        .icons
                        .cached_report(&declarations[0].arguments[0])
                        .unwrap_or_else(|| {
                            Report::failure(
                                &declarations[0].arguments[0],
                                "suppressed",
                                "Resource has not been admitted",
                            )
                        }),
                    None,
                )
            };
            report.suppression_reason =
                reason.or_else(|| (report.status != "ready").then_some("unavailable_icon"));
            report.displayed = admit && report.suppression_reason.is_none();
            if report.displayed {
                displayed += 1;
            }
            let instance = if reason.is_none() {
                asset.map(|asset| Instance {
                    center: entity.origin().unwrap().as_vec3().to_array(),
                    world_size: report.world_size,
                    asset,
                    blend: render::icons::Blend::Alpha,
                })
            } else {
                None
            };
            if inspected == Some(id) || inspected.is_none() {
                rows.insert(
                    id,
                    Row {
                        icon: Some(report),
                        sprite,
                        instance: sprite_instance.or(instance),
                    },
                );
            }
            if inspected == Some(id) {
                break;
            }
        }
        rows
    }
    pub(super) fn inspected_images(
        &self,
        id: u32,
    ) -> (Option<Report>, Option<super::sprites::Report>) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let view = self.view();
        self.image_rows(document, &evaluation, Some(id), view, true)
            .remove(&id)
            .map_or((None, None), |row| (row.icon, row.sprite))
    }
    pub(in crate::session) fn fgd_icon_instances(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        enabled: bool,
        sprite_enabled: bool,
        additive: bool,
    ) -> BTreeMap<u32, Instance> {
        if !enabled && !sprite_enabled {
            return BTreeMap::new();
        }
        self.image_rows(
            document,
            evaluation,
            None,
            render::ViewOptions {
                entity_images: enabled,
                sprite_resources: sprite_enabled,
                additive_sprite_guides: additive,
                ..self.view()
            },
            true,
        )
        .into_iter()
        .filter_map(|(id, row)| row.instance.map(|instance| (id, instance)))
        .collect()
    }

    pub(super) fn image_errors(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        view: render::ViewOptions,
    ) -> BTreeMap<u32, String> {
        self.image_rows(document, evaluation, None, view, false)
            .into_iter()
            .filter_map(|(id, row)| {
                let error = row
                    .icon
                    .and_then(|icon| {
                        matches!(icon.status, "unavailable" | "invalid")
                            .then_some(icon.error)
                            .flatten()
                    })
                    .or_else(|| {
                        row.sprite
                            .and_then(|sprite| sprite.image.and_then(|image| image.error))
                    });
                error.map(|error| (id, error))
            })
            .collect()
    }

    pub fn sprite_guide_preview_state(&self, view: render::ViewOptions) -> serde_json::Value {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let rows = self.image_rows(document, &evaluation, None, view, true);
        let mut total = 0;
        let mut reports = Vec::new();
        for (id, row) in rows {
            if let Some(report) = row.sprite {
                total += 1;
                if reports.len() < render::icons::MAX_INSTANCES {
                    reports.push(serde_json::json!({"id":id,"guide":report}));
                }
            }
        }
        serde_json::json!({"requested":view.additive_sprite_guides,"entity_images":view.entity_images,"sprite_resources":view.sprite_resources,"total":total,"truncated":total>reports.len(),"guides":reports,"limits":super::sprites::LIMITS})
    }
}
