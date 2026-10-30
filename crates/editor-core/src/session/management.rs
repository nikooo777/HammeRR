use std::collections::BTreeSet;

use glam::DVec3;
use map_core::{ObjectId, query};
use serde::{Deserialize, Serialize};
use source_formats::vmf::{Entity, Properties};

use super::{Actor, SelectMode, Session};
use crate::inspector;

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ObjectFilter {
    pub query: Option<String>,
    pub classname: Option<String>,
    #[serde(default)]
    pub exact_class: bool,
    pub targetname: Option<String>,
    pub material: Option<String>,
    pub within: Option<[[f64; 3]; 2]>,
    pub kind: Option<String>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ObjectReport {
    #[serde(skip)]
    pub object: ObjectId,
    pub id: String,
    pub kind: String,
    pub classname: String,
    pub targetname: String,
    pub hidden: bool,
    pub hidden_brushes: usize,
    pub materials: Vec<String>,
    pub bounds: Option<[[f64; 3]; 2]>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PropertyChange {
    Set {
        key: String,
        value: String,
    },
    Remove {
        key: String,
    },
    Default {
        key: String,
    },
    Flag {
        key: String,
        flag: u64,
        enabled: bool,
    },
    Component {
        key: String,
        index: usize,
        value: f64,
    },
}

impl PropertyChange {
    fn key(&self) -> &str {
        match self {
            Self::Set { key, .. }
            | Self::Remove { key }
            | Self::Default { key }
            | Self::Flag { key, .. }
            | Self::Component { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyTarget {
    World,
    Entities(Vec<u32>),
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyDraft {
    pub token: u64,
    pub generation: u64,
    pub revision: u64,
    pub definitions: u64,
    pub target: PropertyTarget,
    pub changes: Vec<PropertyChange>,
}

#[derive(Debug, Serialize)]
pub struct PropertyValue {
    pub object: String,
    pub stored: Option<String>,
    pub default: Option<String>,
    pub effective: String,
}

#[derive(Debug, Serialize)]
pub struct ManagedProperty {
    pub field: inspector::Field,
    pub mixed: bool,
    pub mixed_flags: Vec<String>,
    pub stored_count: usize,
    pub can_reset: bool,
    pub values: Vec<PropertyValue>,
}

#[derive(Debug, Serialize)]
pub struct PropertyPreview {
    pub generation: u64,
    pub revision: u64,
    pub target: PropertyTarget,
    pub count: usize,
    pub properties: Vec<ManagedProperty>,
    pub pending: usize,
}

type Visibility = Vec<(ObjectId, bool, Vec<(u32, bool)>)>;

pub(super) struct VisibilityRestore {
    states: Visibility,
    generation: u64,
}

fn contains(value: &str, needle: &Option<String>) -> bool {
    needle.as_ref().is_none_or(|needle| {
        value
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

impl Session {
    pub fn object_report(&self, filter: &ObjectFilter) -> Result<Vec<ObjectReport>, String> {
        if filter.kind.as_deref().is_some_and(|kind| {
            !["solid", "entity", "point_entity", "brush_entity"].contains(&kind)
        }) {
            return Err("kind must be solid, entity, point_entity or brush_entity".into());
        }
        if let Some([lo, hi]) = filter.within {
            let (lo, hi) = (DVec3::from_array(lo), DVec3::from_array(hi));
            if !lo.is_finite() || !hi.is_finite() || !lo.cmple(hi).all() {
                return Err("Bounds need finite, ordered minimum and maximum coordinates".into());
            }
        }
        let mut rows = Vec::new();
        let model_bounds = self.display_bounds_in(&self.document);
        for id in self.document.objects() {
            let (kind, classname, targetname, hidden, solids) = match id {
                ObjectId::Solid(id) => {
                    let solid = self.document.solid(id).unwrap();
                    ("solid", "", "", solid.hidden, vec![solid])
                }
                ObjectId::Entity(id) => {
                    let entity = self.document.entity(id).unwrap();
                    (
                        if entity.solids.is_empty() {
                            "point_entity"
                        } else {
                            "brush_entity"
                        },
                        entity.classname().unwrap_or(""),
                        entity.properties.get("targetname").unwrap_or(""),
                        entity.hidden,
                        entity.solids.iter().collect(),
                    )
                }
            };
            let materials: Vec<String> = solids
                .iter()
                .flat_map(|solid| solid.sides.iter().map(|side| side.material.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let name = match id {
                ObjectId::Solid(id) => format!("solid:{id}"),
                ObjectId::Entity(id) => format!("entity:{id}"),
            };
            let bounds = model_bounds
                .get(&id)
                .copied()
                .or_else(|| query::bounds(&self.document, id));
            let class_matches = if filter.exact_class {
                filter
                    .classname
                    .as_ref()
                    .is_none_or(|name| classname.eq_ignore_ascii_case(name))
            } else {
                contains(classname, &filter.classname)
            };
            if !class_matches
                || !contains(targetname, &filter.targetname)
                || filter.hidden.is_some_and(|wanted| wanted != hidden)
                || filter.kind.as_deref().is_some_and(|wanted| {
                    wanted != kind && !(wanted == "entity" && kind != "solid")
                })
                || filter.material.is_some()
                    && !materials
                        .iter()
                        .any(|material| contains(material, &filter.material))
                || !contains(
                    &format!(
                        "{name} {kind} {classname} {targetname} {}",
                        materials.join(" ")
                    ),
                    &filter.query,
                )
                || filter.within.is_some_and(|[lo, hi]| {
                    bounds.is_none_or(|(a, b)| {
                        !a.cmple(DVec3::from_array(hi)).all()
                            || !b.cmpge(DVec3::from_array(lo)).all()
                    })
                })
            {
                continue;
            }
            rows.push(ObjectReport {
                object: id,
                id: name,
                kind: kind.into(),
                classname: classname.into(),
                targetname: targetname.into(),
                hidden,
                hidden_brushes: solids.iter().filter(|solid| solid.hidden).count(),
                materials,
                bounds: bounds.map(|(lo, hi)| [lo.to_array(), hi.to_array()]),
            });
        }
        Ok(rows)
    }

    pub fn select_report(
        &mut self,
        actor: Actor,
        filter: &ObjectFilter,
        mode: SelectMode,
        frame: bool,
        aspect: f32,
    ) -> Result<usize, String> {
        let rows = self.object_report(filter)?;
        let ids = rows.iter().map(|row| row.object).collect::<Vec<_>>();
        if frame && (ids.is_empty() || !aspect.is_finite() || aspect <= 0.0) {
            return Err("Framing needs matching objects and a positive viewport aspect".into());
        }
        self.select(&ids, mode)?;
        if frame {
            self.frame_selection(actor, aspect)?;
        }
        Ok(ids.len())
    }

    pub fn visibility_state(&self) -> serde_json::Value {
        let states = self.visibility_snapshot();
        serde_json::json!({
            "hidden_objects": states.iter().filter(|(_, hidden, _)| *hidden).count(),
            "hidden_child_brushes": states.iter().flat_map(|(_,_,solids)| solids).filter(|(_, hidden)| *hidden).count(),
            "can_restore": self.visibility_restore.as_ref().is_some_and(|restore| restore.generation == self.document_generation),
            "compile_behavior": "Hidden objects and hidden child brushes are omitted from compilation."
        })
    }

    fn visibility_snapshot(&self) -> Visibility {
        self.document
            .objects()
            .map(|id| match id {
                ObjectId::Solid(n) => (id, self.document.solid(n).unwrap().hidden, vec![]),
                ObjectId::Entity(n) => {
                    let entity = self.document.entity(n).unwrap();
                    (
                        id,
                        entity.hidden,
                        entity
                            .solids
                            .iter()
                            .map(|solid| (solid.id, solid.hidden))
                            .collect(),
                    )
                }
            })
            .collect()
    }

    pub fn set_visibility(
        &mut self,
        actor: Actor,
        action: &str,
        ids: &[ObjectId],
    ) -> Result<(), String> {
        if !["hide", "show", "isolate", "show_all", "restore"].contains(&action) {
            return Err(
                "Visibility action must be hide, show, isolate, show_all or restore".into(),
            );
        }
        if ["hide", "show", "isolate"].contains(&action) && ids.is_empty() {
            return Err("Select at least one object".into());
        }
        if let Some(id) = ids.iter().find(|&&id| !self.document.contains(id)) {
            return Err(format!("{id} does not exist"));
        }
        let before = self.visibility_snapshot();
        let after = if action == "restore" {
            let restore = self
                .visibility_restore
                .as_ref()
                .ok_or("No visibility state to restore")?;
            if restore.generation != self.document_generation {
                return Err("Map changed; no visibility state to restore".into());
            }
            restore.states.clone()
        } else {
            before
                .iter()
                .map(|(id, hidden, children)| {
                    let selected = ids.contains(id);
                    let value = match action {
                        "hide" if selected => true,
                        "show" if selected => false,
                        "isolate" => !selected,
                        "show_all" => false,
                        _ => *hidden,
                    };
                    let show_children =
                        action == "show_all" || selected && ["show", "isolate"].contains(&action);
                    (
                        *id,
                        value,
                        children
                            .iter()
                            .map(|&(id, hidden)| (id, hidden && !show_children))
                            .collect(),
                    )
                })
                .collect()
        };
        self.edit(actor, &format!("{action} objects"), |tx| {
            for (id, hidden, children) in &after {
                if !tx.document().contains(*id) {
                    continue;
                }
                match *id {
                    ObjectId::Solid(id) => tx.solid_mut(id)?.hidden = *hidden,
                    ObjectId::Entity(id) => {
                        let entity = tx.entity_mut(id)?;
                        entity.hidden = *hidden;
                        for (id, hidden) in children {
                            if let Some(solid) =
                                entity.solids.iter_mut().find(|solid| solid.id == *id)
                            {
                                solid.hidden = *hidden;
                            }
                        }
                    }
                }
            }
            Ok(())
        })?;
        if before != self.visibility_snapshot() {
            self.visibility_restore = Some(VisibilityRestore {
                states: before,
                generation: self.document_generation,
            });
        }
        Ok(())
    }

    pub fn property_draft(&self) -> Option<&PropertyDraft> {
        self.property_draft.as_ref()
    }

    pub fn property_edit_report(&self) -> Result<serde_json::Value, String> {
        let preview = self.preview_property_edit()?;
        Ok(serde_json::json!({
            "handle": Self::edit_handle("property_edit", self.property_draft.as_ref().map(|draft| draft.token)),
            "draft": self.property_draft(),
            "preview": preview,
        }))
    }
    pub fn management_serial(&self) -> u64 {
        self.management_serial
    }

    pub fn check_property_draft(&self, token: u64) -> Result<(), String> {
        let draft = self
            .property_draft
            .as_ref()
            .ok_or("No property draft; open properties first")?;
        if draft.token != token {
            return Err("Property draft replaced; reopen properties".into());
        }
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions != self.definitions_serial
        {
            return Err("Map or definitions changed; cancel and reopen properties".into());
        }
        Ok(())
    }

    fn property_entities(&self, target: &PropertyTarget) -> Result<Vec<Entity>, String> {
        match target {
            PropertyTarget::World => {
                let mut entity = Entity::new(self.document.vmf().world.id, "worldspawn");
                entity.properties = self.document.vmf().world.properties.clone();
                Ok(vec![entity])
            }
            PropertyTarget::Entities(ids) => {
                if ids.is_empty() {
                    return Err("Select at least one entity".into());
                }
                ids.iter()
                    .map(|id| {
                        self.document
                            .entity(*id)
                            .cloned()
                            .ok_or_else(|| format!("entity:{id} does not exist"))
                    })
                    .collect()
            }
        }
    }

    pub fn begin_property_edit(&mut self, target: PropertyTarget) -> Result<(), String> {
        let target = match target {
            PropertyTarget::Entities(ids) => PropertyTarget::Entities(
                ids.into_iter()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            ),
            target => target,
        };
        self.property_entities(&target)?;
        self.management_serial += 1;
        self.property_draft = Some(PropertyDraft {
            token: self.management_serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            target,
            changes: vec![],
        });
        Ok(())
    }

    pub fn cancel_property_edit(&mut self, token: u64) -> Result<(), String> {
        if self
            .property_draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Property draft replaced or closed".into());
        }
        self.property_draft = None;
        self.management_serial += 1;
        Ok(())
    }

    pub fn update_property_edit(
        &mut self,
        token: u64,
        changes: Vec<PropertyChange>,
    ) -> Result<(), String> {
        self.check_property_draft(token)?;
        let target = &self.property_draft.as_ref().unwrap().target;
        let mut entities = self.property_entities(target)?;
        self.apply_property_changes(&mut entities, &changes)?;
        self.property_draft.as_mut().unwrap().changes = changes;
        self.management_serial += 1;
        Ok(())
    }

    pub fn append_property_change(
        &mut self,
        token: u64,
        change: PropertyChange,
    ) -> Result<(), String> {
        self.check_property_draft(token)?;
        let mut changes = self.property_draft.as_ref().unwrap().changes.clone();
        changes.push(change);
        self.update_property_edit(token, changes)
    }

    fn apply_property_changes(
        &self,
        entities: &mut [Entity],
        changes: &[PropertyChange],
    ) -> Result<(), String> {
        for entity in entities {
            let classname = entity.classname().unwrap_or("").to_owned();
            let mut emission_changed = false;
            for change in changes {
                let key = change.key();
                if ["id", "classname", "targetname", "mapversion"]
                    .iter()
                    .any(|name| key.eq_ignore_ascii_case(name))
                {
                    return Err(format!(
                        "{key} cannot be changed here; use References and rename for entity names"
                    ));
                }
                if classname.eq_ignore_ascii_case("worldspawn")
                    && compiler::is_material_emission_key(key)
                {
                    emission_changed = true;
                    if key != compiler::MATERIAL_EMISSION_FILE_KEY {
                        return Err(format!(
                            "World requires one canonical {} key; resolve noncanonical or duplicate keys before editing",
                            compiler::MATERIAL_EMISSION_FILE_KEY
                        ));
                    }
                    compiler::material_emission_file_field(&entity.properties)?;
                    match change {
                        PropertyChange::Set { value, .. } => {
                            compiler::validate_material_emission_file(value)?;
                            source_formats::keyvalues::validate_string(value)
                                .map_err(|error| format!("{key}: {error}"))?;
                            entity.properties.set(key, value);
                        }
                        PropertyChange::Remove { .. } => {
                            entity.properties.remove(key);
                        }
                        _ => {
                            return Err(
                                "Emission file is an optional text path; set a value or remove the override"
                                    .into(),
                            );
                        }
                    }
                    continue;
                }
                let emission_policy = if !classname.eq_ignore_ascii_case("worldspawn") {
                    None
                } else if compiler::is_material_emission_native_direct_key(key) {
                    Some((
                        compiler::MATERIAL_EMISSION_NATIVE_DIRECT_KEY,
                        "Native direct emission",
                        compiler::material_emission_native_direct_field
                            as fn(&Properties) -> Result<Option<&str>, String>,
                        compiler::validate_material_emission_native_direct
                            as fn(&str) -> Result<(), String>,
                    ))
                } else if compiler::is_material_emission_exact_visibility_key(key) {
                    Some((
                        compiler::MATERIAL_EMISSION_EXACT_VISIBILITY_KEY,
                        "Exact emission visibility",
                        compiler::material_emission_exact_visibility_field
                            as fn(&Properties) -> Result<Option<&str>, String>,
                        compiler::validate_material_emission_exact_visibility
                            as fn(&str) -> Result<(), String>,
                    ))
                } else {
                    None
                };
                if let Some((canonical_key, label, validate_fields, validate_value)) =
                    emission_policy
                {
                    if key != canonical_key {
                        return Err(format!(
                            "World requires one canonical {canonical_key} key; resolve noncanonical or duplicate keys before editing"
                        ));
                    }
                    validate_fields(&entity.properties)?;
                    match change {
                        PropertyChange::Set { value, .. } => {
                            validate_value(value)?;
                            entity.properties.set(key, value);
                        }
                        PropertyChange::Remove { .. } => {
                            entity.properties.remove(key);
                        }
                        PropertyChange::Default { .. } => {
                            entity.properties.set(key, "0");
                        }
                        _ => {
                            return Err(format!("{label} accepts a 0/1 value, reset or removal"));
                        }
                    }
                    emission_changed = true;
                    continue;
                }
                if entity.properties.get_all(key).count() > 1 {
                    return Err(format!(
                        "entity:{} has duplicate {key} keys; resolve them before editing",
                        entity.id
                    ));
                }
                let property = self
                    .definitions
                    .class(&classname)
                    .and_then(|class| class.property(key));
                let water_current = classname.eq_ignore_ascii_case("worldspawn")
                    && key.eq_ignore_ascii_case(compiler::WATER_CURRENT_VELOCITY_KEY);
                let water_damping = classname.eq_ignore_ascii_case("worldspawn")
                    && key.eq_ignore_ascii_case(compiler::WATER_DAMPING_KEY);
                let default = property.and_then(|property| property.default_value());
                let current = entity
                    .properties
                    .get(key)
                    .map(str::to_owned)
                    .or_else(|| water_current.then(|| inspector::WATER_CURRENT_DEFAULT.into()))
                    .or_else(|| water_damping.then(|| compiler::DEFAULT_WATER_DAMPING.to_string()))
                    .or_else(|| default.clone())
                    .unwrap_or_default();
                let value = match change {
                    PropertyChange::Set { value, .. } => Some(value.clone()),
                    PropertyChange::Remove { .. } => None,
                    PropertyChange::Default { .. } => Some(
                        default.ok_or_else(|| format!("{classname}.{key} has no FGD default"))?,
                    ),
                    PropertyChange::Flag { flag, enabled, .. } => {
                        if *flag == 0
                            || !flag.is_power_of_two()
                            || property.is_none_or(|property| {
                                property.kind != "flags"
                                    || !property.choices.iter().any(|choice| {
                                        choice.value.parse::<u64>().ok() == Some(*flag)
                                    })
                            })
                        {
                            return Err(format!(
                                "{classname}.{key}: choose a declared single flag"
                            ));
                        }
                        let current: u64 = current
                            .parse()
                            .map_err(|_| format!("{key} is not an unsigned flag value"))?;
                        Some(
                            if *enabled {
                                current | flag
                            } else {
                                current & !flag
                            }
                            .to_string(),
                        )
                    }
                    PropertyChange::Component { index, value, .. } => {
                        let fallback = match key.to_ascii_lowercase().as_str() {
                            "origin" => "origin",
                            "angles" => "vector",
                            "_light" | "_ambient" => "color255",
                            _ => "",
                        };
                        let kind = if water_current {
                            "vector"
                        } else if water_damping {
                            "float"
                        } else {
                            property.map_or(fallback, |property| property.kind.as_str())
                        };
                        if !value.is_finite()
                            || !["origin", "vector", "angle", "angles", "color255", "color1"]
                                .contains(&kind)
                        {
                            return Err(format!("{key} is not a supported vector or color"));
                        }
                        let mut parts = current
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        let part = parts
                            .get_mut(*index)
                            .ok_or_else(|| format!("{key} has no component {index}"))?;
                        *part = value.to_string();
                        let value = parts.join(" ");
                        let components = source_formats::fgd::vector(&value, 3)
                            .or_else(|| {
                                if kind == "color255" {
                                    source_formats::fgd::vector(&value, 4)
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| format!("{key} expects a finite vector or color"))?;
                        if kind == "color255"
                            && components[..3]
                                .iter()
                                .any(|value| !(-1.0..=255.0).contains(value))
                            || kind == "color1"
                                && components.iter().any(|value| !(0.0..=1.0).contains(value))
                        {
                            return Err(format!("{key} color components are out of range"));
                        }
                        Some(value)
                    }
                };
                self.validate_property(&classname, key, value.as_deref())?;
                match value {
                    Some(value) => entity.properties.set(key, value),
                    None => {
                        entity.properties.remove(key);
                    }
                }
            }
            if emission_changed {
                compiler::material_emission_native_direct(&entity.properties)?;
                compiler::material_emission_exact_visibility(&entity.properties)?;
            }
        }
        Ok(())
    }

    pub fn inspect_properties(&self, target: PropertyTarget) -> Result<PropertyPreview, String> {
        self.describe_properties(target.clone(), self.property_entities(&target)?, 0)
    }

    pub fn preview_property_edit(&self) -> Result<PropertyPreview, String> {
        let draft = self.property_draft.as_ref().ok_or("No property draft")?;
        self.check_property_draft(draft.token)?;
        let mut entities = self.property_entities(&draft.target)?;
        self.apply_property_changes(&mut entities, &draft.changes)?;
        self.describe_properties(draft.target.clone(), entities, draft.changes.len())
    }

    fn describe_properties(
        &self,
        target: PropertyTarget,
        entities: Vec<Entity>,
        pending: usize,
    ) -> Result<PropertyPreview, String> {
        let first = &entities[0];
        let mut fields = inspector::inspect(
            first,
            self.definitions.class(first.classname().unwrap_or("")),
        )
        .properties;
        fields.retain(|field| {
            !["id", "classname", "targetname", "mapversion"]
                .iter()
                .any(|key| field.key.eq_ignore_ascii_case(key))
        });
        let mut properties = Vec::new();
        for mut field in fields {
            let native_emission = target == PropertyTarget::World
                && compiler::is_material_emission_native_direct_key(&field.key);
            let exact_emission = target == PropertyTarget::World
                && compiler::is_material_emission_exact_visibility_key(&field.key);
            let material_emission = target == PropertyTarget::World
                && (compiler::is_material_emission_key(&field.key)
                    || native_emission
                    || exact_emission);
            let generated_detail_tint = target == PropertyTarget::World
                && field
                    .key
                    .eq_ignore_ascii_case(compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY);
            let water_current = target == PropertyTarget::World
                && field
                    .key
                    .eq_ignore_ascii_case(compiler::WATER_CURRENT_VELOCITY_KEY);
            let water_damping = target == PropertyTarget::World
                && field.key.eq_ignore_ascii_case(compiler::WATER_DAMPING_KEY);
            let mut values = Vec::new();
            let mut compatible = true;
            for entity in &entities {
                let class = self.definitions.class(entity.classname().unwrap_or(""));
                let property = if material_emission {
                    None
                } else {
                    class.and_then(|class| class.property(&field.key))
                };
                if entity.properties.get(&field.key).is_none()
                    && property.is_none()
                    && !water_current
                    && !water_damping
                    && !material_emission
                    || property.is_some_and(|property| {
                        (!generated_detail_tint
                            && !water_current
                            && !water_damping
                            && property.kind != field.kind)
                            || property.read_only != field.read_only
                    })
                {
                    compatible = false;
                    break;
                }
                let stored = if material_emission {
                    entity
                        .properties
                        .iter()
                        .find(|(name, _)| *name == field.key)
                        .map(|_| field.value.clone())
                } else {
                    entity.properties.get(&field.key).map(str::to_owned)
                };
                let default = if native_emission || exact_emission {
                    Some("0".into())
                } else {
                    property.and_then(|property| property.default_value())
                };
                values.push(PropertyValue {
                    object: if target == PropertyTarget::World {
                        "world".into()
                    } else {
                        format!("entity:{}", entity.id)
                    },
                    effective: stored
                        .clone()
                        .or_else(|| water_current.then(|| inspector::WATER_CURRENT_DEFAULT.into()))
                        .or_else(|| {
                            water_damping.then(|| compiler::DEFAULT_WATER_DAMPING.to_string())
                        })
                        .or_else(|| default.clone())
                        .unwrap_or_default(),
                    stored,
                    default,
                });
            }
            if !compatible {
                continue;
            }
            let mixed = values
                .iter()
                .any(|value| value.effective != values[0].effective);
            let mut mixed_flags = Vec::new();
            for flag in &mut field.flags {
                let bit = flag.value.parse::<u64>().unwrap_or(0);
                let enabled: Vec<bool> = values
                    .iter()
                    .map(|value| value.effective.parse::<u64>().unwrap_or(0) & bit != 0)
                    .collect();
                flag.checked = enabled.iter().all(|value| *value);
                if enabled.iter().any(|value| *value) && !flag.checked {
                    mixed_flags.push(flag.value.clone());
                }
            }
            properties.push(ManagedProperty {
                field,
                mixed,
                mixed_flags,
                stored_count: values.iter().filter(|value| value.stored.is_some()).count(),
                can_reset: values.iter().all(|value| value.default.is_some()),
                values,
            });
        }
        Ok(PropertyPreview {
            generation: self.document_generation,
            revision: self.document.revision(),
            target,
            count: entities.len(),
            properties,
            pending,
        })
    }

    pub fn commit_property_edit(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        self.check_property_draft(token)?;
        let draft = self.property_draft.as_ref().unwrap().clone();
        let mut entities = self.property_entities(&draft.target)?;
        self.apply_property_changes(&mut entities, &draft.changes)?;
        self.edit(
            actor,
            if draft.target == PropertyTarget::World {
                "edit world settings"
            } else {
                "edit entity property batch"
            },
            |tx| {
                for entity in entities {
                    if draft.target == PropertyTarget::World {
                        *tx.world_properties_mut() = entity.properties;
                    } else {
                        tx.entity_mut(entity.id)?.properties = entity.properties;
                    }
                }
                Ok(())
            },
        )?;
        self.cancel_property_edit(token)
    }
}
