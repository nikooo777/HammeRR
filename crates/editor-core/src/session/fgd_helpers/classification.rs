use super::super::working_region::Evaluation;
use map_core::{
    Document, ObjectId,
    entity_helpers::{MAX_HELPER_EXTENT, specialized_light},
};
use source_formats::{
    fgd::{Class, ClassKind},
    vmf::Entity,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(super) struct Context<'a> {
    evaluation: &'a Evaluation,
    selected: &'a BTreeSet<ObjectId>,
    loaded: HashSet<ObjectId>,
}

pub(super) struct Facts {
    pub(super) point: bool,
    pub(super) light: bool,
    pub(super) rope: bool,
    pub(super) sprite: bool,
    pub(super) loaded: bool,
    pub(super) model_helper: bool,
    pub(super) sprite_helper: bool,
    pub(super) declared_model: bool,
    pub(super) authored_model: bool,
    pub(super) other_specialized: bool,
    pub(super) invalid_size: bool,
    pub(super) hidden: bool,
    pub(super) outside: bool,
    pub(super) editor_hidden: bool,
    pub(super) selected: bool,
    pub(super) valid_origin: bool,
}

impl<'a> Context<'a> {
    pub(super) fn new(evaluation: &'a Evaluation, selected: &'a BTreeSet<ObjectId>) -> Self {
        Self {
            evaluation,
            selected,
            loaded: evaluation
                .models
                .instances
                .iter()
                .filter(|instance| instance.has_geometry())
                .map(|instance| instance.id)
                .collect(),
        }
    }

    pub(super) fn entities<'d>(
        &self,
        document: &'d Document,
        inspected: Option<u32>,
        selected_only: bool,
    ) -> BTreeMap<u32, &'d Entity> {
        self.entities_where(document, inspected, selected_only, |_| true)
    }

    pub(super) fn entities_where<'d>(
        &self,
        document: &'d Document,
        inspected: Option<u32>,
        selected_only: bool,
        predicate: impl Fn(&Entity) -> bool,
    ) -> BTreeMap<u32, &'d Entity> {
        document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                predicate(entity)
                    && (!selected_only
                        || self.selected.contains(&ObjectId::Entity(entity.id))
                        || inspected == Some(entity.id))
            })
            .map(|entity| (entity.id, entity))
            .collect()
    }

    pub(super) fn facts(&self, entity: &Entity, class: &Class) -> Facts {
        let object = ObjectId::Entity(entity.id);
        let named = |names: &[&str]| {
            entity.classname().is_some_and(|name| {
                names
                    .iter()
                    .any(|candidate| name.eq_ignore_ascii_case(candidate))
            })
        };
        let helper = |names: &[&str]| {
            class
                .helpers
                .iter()
                .any(|helper| names.contains(&helper.name.as_str()))
        };
        let authored_model = entity.properties.get("model").is_some();
        Facts {
            point: class.kind == ClassKind::Point && entity.solids.is_empty(),
            light: specialized_light(entity),
            rope: named(&["move_rope", "keyframe_rope"]),
            sprite: named(&["env_sprite", "env_sprite_oriented"]),
            loaded: self.loaded.contains(&object),
            model_helper: helper(&["studio", "studioprop", "model"])
                || class
                    .property("model")
                    .is_some_and(|property| property.kind == "studio")
                || authored_model,
            sprite_helper: helper(&["sprite"]),
            declared_model: class.property("model").is_some(),
            authored_model,
            other_specialized: helper(&["model", "sprite", "decal", "overlay"])
                || named(&[
                    "info_overlay",
                    "info_overlay_transition",
                    "env_sprite",
                    "env_sprite_oriented",
                ]),
            invalid_size: self
                .evaluation
                .helpers
                .get(&object)
                .is_some_and(|appearance| appearance.invalid_size),
            hidden: entity.hidden,
            outside: !self.evaluation.objects.contains(&object),
            editor_hidden: self.evaluation.editor_hidden.contains(&object),
            selected: self.selected.contains(&object),
            valid_origin: entity.origin().is_some_and(|origin| {
                origin.is_finite() && origin.abs().max_element() <= MAX_HELPER_EXTENT
            }),
        }
    }
}
