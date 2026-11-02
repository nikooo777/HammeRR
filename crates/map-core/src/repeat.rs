use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DVec3};
use source_formats::fgd::Catalog;

use crate::{Document, EditError, ObjectId, Transaction, connections, edit, references};

pub const MAX_SOURCES: usize = 16;
pub const MAX_COPIES: u32 = 64;
pub const MAX_OBJECTS: usize = 256;
pub const MAX_SIDES: usize = 8192;
pub const MAX_NAME_BYTES: usize = 128;

mod plan;
mod policies;
pub use plan::{Placement, Plan};
pub use policies::{
    ExternalReferences, InternalReferences, MAX_AFFIX_BYTES, MAX_NAME_INDEX, NameStyle, Naming,
    Policies,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub count: u32,
    pub translation: DVec3,
    pub rotation: DVec3,
    pub scale: DVec3,
    pub pivot: DVec3,
    pub basis: DMat3,
    pub texture_lock: bool,
}

impl Options {
    pub fn copy_scale(&self, index: u32) -> DVec3 {
        self.scale.map(|factor| factor.powf(f64::from(index)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub source: u32,
    pub copy: u32,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Copy {
    pub placement: Placement,
    pub mapping: Vec<(ObjectId, ObjectId)>,
    pub index: u32,
    pub objects: Vec<ObjectId>,
    pub names: Vec<Name>,
    pub remapped_references: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Result {
    pub copies: Vec<Copy>,
}

impl Result {
    pub fn objects(&self) -> Vec<ObjectId> {
        self.copies
            .iter()
            .flat_map(|copy| copy.objects.iter().copied())
            .collect()
    }
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub(crate) fn validate(
    document: &Document,
    ids: &[ObjectId],
    options: Options,
    catalog: &Catalog,
) -> std::result::Result<(), EditError> {
    let plan = Plan::array(options, Policies::default())?;
    validate_sources(document, ids, plan.placements.len() as u32, catalog)?;
    validate_placements(document, ids, &plan, catalog)?;
    Ok(())
}

pub fn validate_sources(
    document: &Document,
    ids: &[ObjectId],
    count: u32,
    catalog: &Catalog,
) -> std::result::Result<(), EditError> {
    if ids.is_empty() || ids.len() > MAX_SOURCES {
        return Err(invalid("Choose 1–16 source objects"));
    }
    if count == 0 || count > MAX_COPIES || ids.len() * count as usize > MAX_OBJECTS {
        return Err(invalid(
            "Use 1–64 additional copies and at most 256 new top-level objects",
        ));
    }
    if ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(invalid("Repeat sources must be distinct"));
    }
    let mut sides = 0;
    let mut objects = 0;
    for id in ids {
        match *id {
            ObjectId::Solid(id) => {
                let solid = document
                    .solid(id)
                    .ok_or(EditError::NotFound(ObjectId::Solid(id)))?;
                sides += solid.sides.len();
                objects += 1;
            }
            ObjectId::Entity(id) => {
                let entity = document
                    .entity(id)
                    .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
                if entity
                    .properties
                    .0
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case("classname"))
                    .count()
                    != 1
                {
                    return Err(invalid(format!(
                        "Entity {id} needs one unambiguous classname before repeating"
                    )));
                }
                if entity.classname().is_some_and(|class| {
                    class.eq_ignore_ascii_case("info_overlay")
                        || class.eq_ignore_ascii_case("info_overlay_transition")
                }) {
                    return Err(invalid(
                        "Overlay face-ID references are not supported by repeat arrays",
                    ));
                }
                let class = entity.classname().and_then(|name| catalog.class(name));
                if entity.properties.0.iter().any(|(key, value)| {
                    !value.is_empty()
                        && (key.eq_ignore_ascii_case("nodeid")
                            || class.and_then(|class| class.property(key)).is_some_and(
                                |property| {
                                    matches!(
                                        property.kind.as_str(),
                                        "sidelist"
                                            | "side_list"
                                            | "node_dest"
                                            | "node_id"
                                            | "node_source"
                                    )
                                },
                            ))
                }) {
                    return Err(invalid(format!(
                        "Entity {id} has node/side IDs that repeat arrays cannot safely remap"
                    )));
                }
                sides += entity
                    .solids
                    .iter()
                    .map(|solid| solid.sides.len())
                    .sum::<usize>();
                objects += 1 + entity.solids.len();
            }
        }
    }
    if sides * count as usize > MAX_SIDES || objects * count as usize > MAX_OBJECTS {
        return Err(invalid(
            "Array exceeds 256 new objects including owned brushes, or 8192 new sides",
        ));
    }
    document
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(())
}

pub(crate) fn validate_opaque_links(
    document: &Document,
    sources: &BTreeSet<u32>,
    names: &BTreeMap<String, (u32, String)>,
) -> std::result::Result<(), EditError> {
    if names.is_empty() {
        return Ok(());
    }
    for source in sources {
        let entity = document
            .entity(*source)
            .ok_or(EditError::NotFound(ObjectId::Entity(*source)))?;
        let script = entity.properties.0.iter().any(|(key, value)| {
            !value.trim().is_empty()
                && (key.eq_ignore_ascii_case("vscripts")
                    || key.eq_ignore_ascii_case("thinkfunction"))
        });
        let dynamic = entity.connections.iter().flatten().any(|connection| {
            connection.fields().nth(1).is_some_and(|input| {
                ["AddOutput", "RunScriptCode", "RunScriptFile"]
                    .iter()
                    .any(|dynamic| input.eq_ignore_ascii_case(dynamic))
            })
        });
        if script || dynamic {
            return Err(invalid(format!(
                "Entity {source} has script or dynamic I/O that repeat arrays cannot safely remap while changing names"
            )));
        }
    }
    Ok(())
}

pub(crate) fn source_names(
    document: &Document,
    ids: &[ObjectId],
) -> std::result::Result<BTreeMap<String, (u32, String)>, EditError> {
    let mut names = BTreeMap::new();
    for id in ids {
        let ObjectId::Entity(id) = *id else { continue };
        let entity = document
            .entity(id)
            .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
        let values = entity
            .properties
            .0
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"))
            .collect::<Vec<_>>();
        if values.len() > 1 {
            return Err(invalid(format!(
                "Entity {id} has duplicate targetname fields"
            )));
        }
        let Some(name) = values
            .first()
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if name.trim() != name
            || name.len() > MAX_NAME_BYTES
            || name.starts_with('!')
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '*' | '?' | ',' | '"'))
        {
            return Err(invalid(format!(
                "Entity {id} needs a simple, unambiguous targetname before repeating"
            )));
        }
        let occurrences = document
            .vmf()
            .entities
            .iter()
            .flat_map(|entity| &entity.properties.0)
            .filter(|(key, value)| {
                key.eq_ignore_ascii_case("targetname") && value.eq_ignore_ascii_case(name)
            })
            .count();
        if occurrences != 1 {
            return Err(invalid(format!(
                "Name {name} is shared; repeat arrays require unique source names"
            )));
        }
        names.insert(name.to_ascii_lowercase(), (id, name.to_owned()));
    }
    Ok(names)
}

pub(crate) fn planned_names(
    document: &Document,
    names: &BTreeMap<String, (u32, String)>,
    scan: &[references::Reference],
    count: u32,
) -> std::result::Result<Vec<BTreeMap<String, String>>, EditError> {
    policies::planned_names_with(document, names, scan, count, &Policies::default())
}

pub(crate) fn copied_reference<'a>(
    entity: &'a mut source_formats::vmf::Entity,
    reference: &references::Reference,
) -> std::result::Result<&'a mut String, EditError> {
    let value = match &reference.location {
        references::Location::Property { index, key } => entity
            .properties
            .0
            .get_mut(*index)
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value),
        references::Location::Connection { index, output, .. } => entity
            .connections
            .as_mut()
            .and_then(|connections| connections.get_mut(*index))
            .filter(|connection| &connection.output == output)
            .map(|connection| &mut connection.value),
    };
    value.ok_or_else(|| invalid("Copied entity reference no longer matches its source"))
}

fn validate_placements(
    document: &Document,
    ids: &[ObjectId],
    plan: &Plan,
    catalog: &Catalog,
) -> std::result::Result<Vec<Vec<(u32, f32)>>, EditError> {
    if plan.placements.is_empty() || plan.placements.len() > MAX_COPIES as usize {
        return Err(invalid("Use 1–64 additional placements"));
    }
    plan.policies.validate(plan.placements.len() as u32)?;
    plan.placements
        .iter()
        .map(|placement| {
            placement.validate()?;
            let mut scales = Vec::new();
            if placement.scale != DVec3::ONE
                || placement.rotation.map(|angle| angle.rem_euclid(360.0)) != DVec3::ZERO
            {
                for id in ids {
                    match *id {
                        ObjectId::Solid(id) => {
                            let solid = document
                                .solid(id)
                                .ok_or(EditError::NotFound(ObjectId::Solid(id)))?;
                            crate::transfer::placement_solid(solid)?;
                        }
                        ObjectId::Entity(id) => {
                            let entity = document
                                .entity(id)
                                .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
                            if let Some(scale) = crate::transfer::placement_entity_scale(
                                entity,
                                catalog,
                                placement.scale,
                            )? {
                                scales.push((id, scale));
                            }
                        }
                    }
                }
            }
            Ok(scales)
        })
        .collect()
}

pub fn create(
    tx: &mut Transaction,
    ids: &[ObjectId],
    options: Options,
    catalog: &Catalog,
) -> std::result::Result<Result, EditError> {
    create_plan(
        tx,
        ids,
        &Plan::array(options, Policies::default())?,
        catalog,
    )
}

pub fn create_plan(
    tx: &mut Transaction,
    ids: &[ObjectId],
    plan: &Plan,
    catalog: &Catalog,
) -> std::result::Result<Result, EditError> {
    if plan.placements.is_empty() || plan.placements.len() > MAX_COPIES as usize {
        return Err(invalid("Use 1–64 additional placements"));
    }
    let count = plan.placements.len() as u32;
    validate_sources(tx.document(), ids, count, catalog)?;
    let model_scales = validate_placements(tx.document(), ids, plan, catalog)?;
    let names = source_names(tx.document(), ids)?;
    let scan = references::scan(tx.document(), catalog);
    let sources = ids
        .iter()
        .filter_map(|id| match id {
            ObjectId::Entity(id) => Some(*id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    validate_opaque_links(tx.document(), &sources, &names)?;
    let mut internal = Vec::new();
    for reference in &scan {
        if !sources.contains(&reference.source) {
            continue;
        }
        if names
            .keys()
            .any(|name| connections::name_matches(name, &reference.target))
        {
            if !reference.certain || !names.contains_key(&reference.target.to_ascii_lowercase()) {
                return Err(invalid(format!(
                    "Entity {} has an ambiguous internal reference to {}; inspect its references before repeating",
                    reference.source, reference.target
                )));
            }
            if plan.policies.internal == InternalReferences::Remap {
                internal.push(reference);
            }
        } else if plan.policies.external == ExternalReferences::Reject
            && reference.declared
            && !reference.target.starts_with('!')
        {
            return Err(invalid(format!(
                "Entity {} has an external or unresolved reference to {}; external references are rejected by this placement policy",
                reference.source, reference.target
            )));
        }
    }
    let batches =
        policies::planned_names_with(tx.document(), &names, &scan, count, &plan.policies)?;
    let mut copies = Vec::with_capacity(count as usize);
    for (offset, ((batch, placement), scales)) in batches
        .iter()
        .zip(&plan.placements)
        .zip(model_scales)
        .enumerate()
    {
        let index = offset as u32 + 1;
        let objects = edit::duplicate(tx, ids, DVec3::ZERO)?;
        if objects.len() != ids.len() {
            return Err(invalid(
                "Placement copies no longer match their source objects",
            ));
        }
        let source_mapping = ids
            .iter()
            .copied()
            .zip(objects.iter().copied())
            .collect::<Vec<_>>();
        let mut mapping = BTreeMap::new();
        for (source, copy) in &source_mapping {
            match (*source, *copy) {
                (ObjectId::Entity(source), ObjectId::Entity(copy)) => {
                    mapping.insert(source, copy);
                }
                (ObjectId::Solid(_), ObjectId::Solid(_)) => {}
                _ => {
                    return Err(invalid(
                        "Placement copied object kind differs from its source",
                    ));
                }
            }
        }
        let mapped_entity = |source: u32| {
            mapping
                .get(&source)
                .copied()
                .ok_or_else(|| invalid("Copied entity no longer matches its original source"))
        };
        let mut renamed = Vec::new();
        for (key, (source, before)) in &names {
            let copy = mapped_entity(*source)?;
            let after = &batch[key];
            let entity = tx.entity_mut(copy)?;
            let (_, name) = entity
                .properties
                .0
                .iter_mut()
                .find(|(key, _)| key.eq_ignore_ascii_case("targetname"))
                .ok_or_else(|| invalid("Copied name is missing"))?;
            name.clone_from(after);
            renamed.push(Name {
                source: *source,
                copy,
                before: before.clone(),
                after: after.clone(),
            });
        }
        for reference in internal.iter().rev() {
            let entity = tx.entity_mut(mapped_entity(reference.source)?)?;
            let value = copied_reference(entity, reference)?;
            if value.get(reference.span.clone()) != Some(reference.target.as_str()) {
                return Err(invalid("Copied reference span changed"));
            }
            value.replace_range(
                reference.span.clone(),
                &batch[&reference.target.to_ascii_lowercase()],
            );
        }
        edit::transform(tx, &objects, placement.transform())?;
        for (source, scale) in scales {
            tx.entity_mut(mapped_entity(source)?)?
                .properties
                .set("modelscale", scale.to_string());
        }
        for id in &objects {
            let (mins, maxs) = crate::query::bounds_of(tx.document(), &[*id])
                .ok_or_else(|| invalid("A placement copy has no finite bounds"))?;
            if !mins.is_finite()
                || !maxs.is_finite()
                || mins.abs().max(maxs.abs()).max_element() > crate::primitive::MAX_COORDINATE
            {
                return Err(invalid("A placement copy exceeds ±65536 world units"));
            }
        }
        copies.push(Copy {
            index,
            placement: *placement,
            mapping: source_mapping,
            objects,
            names: renamed,
            remapped_references: internal.len(),
        });
    }
    Ok(Result { copies })
}
