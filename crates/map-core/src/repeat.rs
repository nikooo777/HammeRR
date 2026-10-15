use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::fgd::Catalog;

use crate::{Document, EditError, ObjectId, Transaction, connections, edit, references};

pub const MAX_SOURCES: usize = 16;
pub const MAX_COPIES: u32 = 64;
pub const MAX_OBJECTS: usize = 256;
pub const MAX_SIDES: usize = 8192;
pub const MAX_NAME_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub count: u32,
    pub translation: DVec3,
    pub rotation: DVec3,
    pub pivot: DVec3,
    pub texture_lock: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub source: u32,
    pub copy: u32,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub index: u32,
    pub objects: Vec<ObjectId>,
    pub names: Vec<Name>,
    pub remapped_references: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    if ids.is_empty() || ids.len() > MAX_SOURCES {
        return Err(invalid("Choose 1–16 source objects"));
    }
    if options.count == 0
        || options.count > MAX_COPIES
        || ids.len() * options.count as usize > MAX_OBJECTS
    {
        return Err(invalid(
            "Use 1–64 additional copies and at most 256 new top-level objects",
        ));
    }
    if ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(invalid("Repeat sources must be distinct"));
    }
    let limit = crate::primitive::MAX_COORDINATE;
    if [options.translation, options.rotation, options.pivot]
        .iter()
        .any(|value| !value.is_finite() || value.abs().max_element() > limit)
    {
        return Err(invalid(
            "Array steps and pivot must be finite and within ±65536",
        ));
    }
    if options.translation == DVec3::ZERO
        && options.rotation.map(|angle| angle.rem_euclid(360.0)) == DVec3::ZERO
    {
        return Err(invalid("Choose a nonzero translation or rotation step"));
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
    if sides * options.count as usize > MAX_SIDES || objects * options.count as usize > MAX_OBJECTS
    {
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
    let mut occupied = document
        .vmf()
        .entities
        .iter()
        .flat_map(|entity| &entity.properties.0)
        .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"))
        .map(|(_, name)| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut batches = Vec::new();
    for index in 1..=count {
        let mut batch = BTreeMap::new();
        for (key, (_, name)) in names {
            let stem = format!("{name}_copy{index}");
            let mut candidate = stem.clone();
            let mut suffix = 2;
            while occupied.contains(&candidate.to_ascii_lowercase()) {
                candidate = format!("{stem}_{suffix}");
                suffix += 1;
            }
            if candidate.len() > MAX_NAME_BYTES {
                return Err(invalid(format!(
                    "Generated name for {name} exceeds 128 UTF-8 bytes; shorten the source name"
                )));
            }
            if scan
                .iter()
                .any(|reference| connections::name_matches(&candidate, &reference.target))
            {
                return Err(invalid(format!(
                    "Generated name {candidate} would acquire an existing reference; rename the source before repeating"
                )));
            }
            occupied.insert(candidate.to_ascii_lowercase());
            batch.insert(key.clone(), candidate);
        }
        batches.push(batch);
    }
    Ok(batches)
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

pub fn create(
    tx: &mut Transaction,
    ids: &[ObjectId],
    options: Options,
    catalog: &Catalog,
) -> std::result::Result<Result, EditError> {
    validate(tx.document(), ids, options, catalog)?;
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
            internal.push(reference);
        }
    }
    let batches = planned_names(tx.document(), &names, &scan, options.count)?;
    let mut copies = Vec::new();
    for (batch, index) in batches.iter().zip(1..=options.count) {
        let objects = edit::duplicate(tx, ids, DVec3::ZERO)?;
        let mapping = ids
            .iter()
            .zip(&objects)
            .filter_map(|(old, new)| match (old, new) {
                (ObjectId::Entity(old), ObjectId::Entity(new)) => Some((*old, *new)),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut renamed = Vec::new();
        for (key, (source, before)) in &names {
            let copy = mapping[source];
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
            let entity = tx.entity_mut(mapping[&reference.source])?;
            let value = copied_reference(entity, reference)?;
            if value.get(reference.span.clone()) != Some(reference.target.as_str()) {
                return Err(invalid("Copied reference span changed"));
            }
            value.replace_range(
                reference.span.clone(),
                &batch[&reference.target.to_ascii_lowercase()],
            );
        }
        let factor = f64::from(index);
        edit::transform(
            tx,
            &objects,
            edit::Transform {
                translation: options.translation * factor,
                rotation: options.rotation * factor,
                pivot: options.pivot,
                texture_lock: options.texture_lock,
                ..edit::Transform::default()
            },
        )?;
        for id in &objects {
            let (mins, maxs) = crate::query::bounds_of(tx.document(), &[*id])
                .ok_or_else(|| invalid("An array copy has no finite bounds"))?;
            if !mins.is_finite()
                || !maxs.is_finite()
                || mins.abs().max(maxs.abs()).max_element() > crate::primitive::MAX_COORDINATE
            {
                return Err(invalid("An array copy exceeds ±65536 world units"));
            }
        }
        copies.push(Copy {
            index,
            objects,
            names: renamed,
            remapped_references: internal.len(),
        });
    }
    Ok(Result { copies })
}

#[cfg(test)]
mod tests;
