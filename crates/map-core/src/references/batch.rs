use crate::connections::event_name_matches as native_match;
use std::collections::{BTreeMap, BTreeSet};

use source_formats::fgd::Catalog;
use source_formats::keyvalues::Node;

use super::{Change, Location, Reference, name_error, record_key};
use crate::{Document, EditError, Transaction};

pub const MAX_BULK_RENAME_ENTITIES: usize = 256;
pub const MAX_BULK_RENAME_MAP_ENTITIES: usize = 8192;
const MAX_WORK: usize = 64 * 1024 * 1024;
const MAX_REFERENCE_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityNameChange {
    pub entity: u32,
    pub old_name: Option<String>,
    pub new_name: String,
}

#[derive(Debug, Clone)]
pub struct BulkRenamePlan {
    pub revision: u64,
    pub names: Vec<EntityNameChange>,
    pub references: Vec<Change>,
    pub error: Option<String>,
}

struct Work(usize);

impl Work {
    fn charge(&mut self, amount: usize) -> Result<(), String> {
        self.0 = self
            .0
            .checked_sub(amount)
            .ok_or("Bulk rename exceeds its reference work limit")?;
        Ok(())
    }
}

fn source_label(reference: &Reference) -> String {
    match &reference.location {
        Location::Property { key, .. } => format!("Entity {} property {key}", reference.source),
        Location::Connection { index, field, .. } => format!(
            "Entity {} connection {} {}",
            reference.source,
            index + 1,
            if *field == 0 { "target" } else { "parameter" }
        ),
    }
}

fn resolved(
    document: &Document,
    names: &BTreeMap<u32, &str>,
    query: &str,
    classes: bool,
    hidden: bool,
    work: &mut Work,
) -> Result<BTreeSet<u32>, String> {
    let mut result = BTreeSet::new();
    for entity in &document.vmf().entities {
        if !hidden && entity.hidden {
            continue;
        }
        work.charge(super::properties_work(entity, "targetname"))?;
        let name = names
            .get(&entity.id)
            .copied()
            .unwrap_or_else(|| entity.properties.get("targetname").unwrap_or_default());
        work.charge(name.len().saturating_add(query.len()).saturating_add(1))?;
        if !name.is_empty() && native_match(name, query) {
            result.insert(entity.id);
        }
    }
    if classes && result.is_empty() {
        for entity in &document.vmf().entities {
            if !hidden && entity.hidden {
                continue;
            }
            work.charge(super::properties_work(entity, "classname"))?;
            let class = entity.classname().unwrap_or_default();
            work.charge(class.len().saturating_add(query.len()).saturating_add(1))?;
            if !class.is_empty() && native_match(class, query) {
                result.insert(entity.id);
            }
        }
    }
    Ok(result)
}

fn classes_allowed(document: &Document, catalog: &Catalog, reference: &Reference) -> bool {
    match &reference.location {
        Location::Connection { field, .. } => *field == 0,
        Location::Property { key, .. } => document
            .entity(reference.source)
            .and_then(|entity| entity.classname())
            .and_then(|name| catalog.class(name))
            .and_then(|class| class.property(key))
            .is_some_and(|property| property.kind == "target_name_or_class"),
    }
}

fn validate_names(
    document: &Document,
    catalog: &Catalog,
    names: &[EntityNameChange],
    work: &mut Work,
) -> Result<(), String> {
    let targets = names
        .iter()
        .map(|name| name.entity)
        .collect::<BTreeSet<_>>();
    for (index, name) in names.iter().enumerate() {
        if let Some(error) = name_error(&name.new_name) {
            return Err(format!("Entity {}: {error}", name.entity));
        }
        if name.new_name.len() > crate::repeat::MAX_NAME_BYTES {
            return Err(format!(
                "Entity {}: names must fit {} UTF-8 bytes",
                name.entity,
                crate::repeat::MAX_NAME_BYTES
            ));
        }
        for other in &names[..index] {
            work.charge(
                other
                    .new_name
                    .len()
                    .saturating_add(name.new_name.len())
                    .saturating_add(1),
            )?;
            if native_match(&other.new_name, &name.new_name)
                || native_match(&name.new_name, &other.new_name)
            {
                return Err(format!(
                    "More than one captured entity would be named {}",
                    name.new_name
                ));
            }
        }
        let entity = document
            .entity(name.entity)
            .ok_or("Entity no longer exists")?;
        work.charge(super::properties_work(entity, "targetname"))?;
        work.charge(super::properties_work(entity, "classname"))?;
        if entity.properties.get_all("targetname").count() > 1
            || entity.properties.get_all("classname").count() != 1
        {
            return Err(format!(
                "Entity {} needs unambiguous classname and targetname fields",
                name.entity
            ));
        }
        if entity
            .classname()
            .and_then(|name| catalog.class(name))
            .and_then(|class| class.property("targetname"))
            .is_some_and(|property| {
                property.read_only || property.validate(&name.new_name).is_err()
            })
        {
            return Err(format!(
                "Entity {} definition does not permit the proposed name",
                name.entity
            ));
        }
        if let Some(old) = name.old_name.as_deref().filter(|name| !name.is_empty()) {
            if name_error(old).is_some() {
                return Err(format!(
                    "Entity {} has a special or malformed old name {old:?}; resolve it individually before bulk rename",
                    name.entity
                ));
            }
            let mut shared = Vec::new();
            for entity in &document.vmf().entities {
                work.charge(super::properties_work(entity, "targetname"))?;
                for value in entity.properties.get_all("targetname") {
                    work.charge(old.len().saturating_add(value.len()).saturating_add(1))?;
                    if native_match(value, old) || native_match(old, value) {
                        shared.push(entity.id);
                        break;
                    }
                }
            }
            if shared != [name.entity] {
                return Err(format!(
                    "Name {old} is shared by entities {shared:?}; use the group rename operation instead of splitting its references"
                ));
            }
        }
        for other in &document.vmf().entities {
            if targets.contains(&other.id) {
                continue;
            }
            work.charge(super::properties_work(other, "targetname"))?;
            for value in other.properties.get_all("targetname") {
                work.charge(
                    name.new_name
                        .len()
                        .saturating_add(value.len())
                        .saturating_add(1),
                )?;
                if native_match(value, &name.new_name) || native_match(&name.new_name, value) {
                    return Err(format!(
                        "Entity {} already uses {}; it is outside the captured rename set",
                        other.id, name.new_name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn opaque_nodes(nodes: &[Node], names: &[EntityNameChange], work: &mut Work) -> Result<(), String> {
    for node in nodes {
        if let Some(value) = node.as_str() {
            for name in names {
                work.charge(
                    value
                        .len()
                        .saturating_add(name.new_name.len())
                        .saturating_add(1),
                )?;
                if name
                    .old_name
                    .as_deref()
                    .is_some_and(|old| !old.is_empty() && value.eq_ignore_ascii_case(old))
                    || value.eq_ignore_ascii_case(&name.new_name)
                {
                    return Err(format!(
                        "Opaque field {} contains affected name {value}; its binding cannot be rewritten safely",
                        node.key
                    ));
                }
            }
        } else if let Some(children) = node.children() {
            opaque_nodes(children, names, work)?;
        }
    }
    Ok(())
}

fn admit_scope(
    document: &Document,
    names: &[EntityNameChange],
    work: &mut Work,
) -> Result<(), String> {
    opaque_nodes(&document.vmf().extra, names, work)?;
    opaque_nodes(&document.vmf().world.extra, names, work)?;
    for entity in &document.vmf().entities {
        if entity.classname().is_some_and(|name| {
            [
                "func_instance",
                "func_instance_io_proxy",
                "func_instance_parms",
            ]
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
        }) {
            return Err(format!(
                "Entity {} has instance-scoped name bindings; bulk rename requires an authored map without unresolved instance scopes",
                entity.id
            ));
        }
        if let Some((key, _)) = entity.properties.0.iter().find(|(key, value)| {
            !value.trim().is_empty()
                && (key.eq_ignore_ascii_case("vscripts")
                    || key.eq_ignore_ascii_case("thinkfunction"))
        }) {
            return Err(format!(
                "Entity {} has {key}; script name lookups cannot be preserved by bulk rename",
                entity.id
            ));
        }
        if let Some(connection) = entity.connections.iter().flatten().find(|connection| {
            connection.fields().nth(1).is_some_and(|input| {
                ["AddOutput", "RunScriptCode", "RunScriptFile", "Command"]
                    .iter()
                    .any(|known| input.eq_ignore_ascii_case(known))
            })
        }) {
            return Err(format!(
                "Entity {} output {} uses dynamic name bindings; resolve those before bulk rename",
                entity.id, connection.output
            ));
        }
        opaque_nodes(&entity.extra, names, work)?;
    }
    Ok(())
}

fn planned_references(
    document: &Document,
    catalog: &Catalog,
    names: &[EntityNameChange],
    scan: Vec<Reference>,
    work: &mut Work,
) -> Result<Vec<Change>, String> {
    admit_scope(document, names, work)?;
    let after_names = names
        .iter()
        .map(|name| (name.entity, name.new_name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut changes = Vec::new();
    for reference in scan {
        if reference.target.len() > MAX_REFERENCE_BYTES {
            return Err("Reference exceeds the bulk rename field limit".into());
        }
        if reference.target.starts_with('!') {
            continue;
        }
        let mut affected = false;
        for name in names {
            let old = name.old_name.as_deref().unwrap_or_default();
            work.charge(
                old.len()
                    .saturating_add(name.new_name.len())
                    .saturating_add(reference.target.len())
                    .saturating_add(1),
            )?;
            affected |= !old.is_empty() && native_match(old, &reference.target)
                || native_match(&name.new_name, &reference.target);
        }
        if !affected {
            continue;
        }
        let replacement = (reference.certain && !reference.target.contains('*'))
            .then(|| {
                names.iter().find(|name| {
                    name.old_name.as_deref().is_some_and(|old| {
                        !old.is_empty() && old.eq_ignore_ascii_case(&reference.target)
                    })
                })
            })
            .flatten()
            .map(|name| name.new_name.as_str())
            .unwrap_or(&reference.target);
        let classes = classes_allowed(document, catalog, &reference);
        for hidden in [false, true] {
            let before = resolved(
                document,
                &BTreeMap::new(),
                &reference.target,
                classes,
                hidden,
                work,
            )?;
            let after = resolved(document, &after_names, replacement, classes, hidden, work)?;
            if before != after {
                return Err(format!(
                    "{} ({}) would change target identities {before:?} → {after:?}; {}",
                    source_label(&reference),
                    reference.target,
                    reference.reason
                ));
            }
        }
        let mut after = reference.value.clone();
        after.replace_range(reference.span.clone(), replacement);
        let changed = after != reference.value;
        changes.push(Change {
            reference,
            after,
            changed,
            review: false,
            reason: if changed {
                "Exact reference rewritten from the original name mapping".into()
            } else {
                "Unchanged reference preserves its original target identities".into()
            },
        });
    }
    let mut final_values = BTreeMap::new();
    for change in changes.iter().rev().filter(|change| change.changed) {
        let replacement = names
            .iter()
            .find(|name| {
                name.old_name
                    .as_deref()
                    .is_some_and(|old| old.eq_ignore_ascii_case(&change.reference.target))
            })
            .unwrap();
        final_values
            .entry(record_key(&change.reference))
            .or_insert_with(|| change.reference.value.clone())
            .replace_range(change.reference.span.clone(), &replacement.new_name);
    }
    for change in &mut changes {
        if let Some(value) = final_values.get(&record_key(&change.reference)) {
            change.after = value.clone();
        }
    }
    Ok(changes)
}

pub fn plan_many(
    document: &Document,
    catalog: &Catalog,
    renames: &[(u32, String)],
) -> Result<BulkRenamePlan, String> {
    if renames.is_empty()
        || renames.len() > MAX_BULK_RENAME_ENTITIES
        || document.vmf().entities.len() > MAX_BULK_RENAME_MAP_ENTITIES
    {
        return Err(format!(
            "Capture 1–{MAX_BULK_RENAME_ENTITIES} entities in a map with at most {MAX_BULK_RENAME_MAP_ENTITIES} entities"
        ));
    }
    if renames
        .iter()
        .any(|(_, name)| name.len() > crate::repeat::MAX_NAME_BYTES)
    {
        return Err(format!(
            "Names must fit {} UTF-8 bytes",
            crate::repeat::MAX_NAME_BYTES
        ));
    }
    if renames
        .iter()
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>()
        .len()
        != renames.len()
    {
        return Err("Capture each entity once".into());
    }
    let names = renames
        .iter()
        .map(|(id, new_name)| {
            let entity = document
                .entity(*id)
                .ok_or_else(|| format!("Entity {id} no longer exists"))?;
            Ok(EntityNameChange {
                entity: *id,
                old_name: entity.properties.get("targetname").map(str::to_owned),
                new_name: new_name.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut work = Work(MAX_WORK);
    let validated = super::checked::scan_checked(document, catalog).and_then(|scan| {
        validate_names(document, catalog, &names, &mut work)?;
        planned_references(document, catalog, &names, scan, &mut work)
    });
    let (references, error) = match validated {
        Ok(references) => (references, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(BulkRenamePlan {
        revision: document.revision(),
        names,
        references,
        error,
    })
}

pub fn apply_many(tx: &mut Transaction, plan: &BulkRenamePlan) -> Result<(), EditError> {
    if tx.document().revision() != plan.revision {
        return Err(EditError::Invalid(
            "Map changed; preview all names again".into(),
        ));
    }
    if let Some(error) = &plan.error {
        return Err(EditError::Invalid(error.clone()));
    }
    for name in &plan.names {
        let entity = tx
            .document()
            .entity(name.entity)
            .ok_or_else(|| EditError::Invalid("Renamed entity no longer exists".into()))?;
        if entity.properties.get("targetname") != name.old_name.as_deref() {
            return Err(EditError::Invalid(
                "Entity name changed; preview all names again".into(),
            ));
        }
    }
    let records = super::checked_changes(tx.document(), &plan.references)?;
    for name in &plan.names {
        tx.entity_mut(name.entity)?
            .properties
            .set("targetname", &name.new_name);
    }
    super::apply_changes(tx, &records)
}
