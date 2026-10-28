use std::collections::BTreeMap;
use std::ops::Range;

use source_formats::fgd::{Catalog, Class};

mod checked;
pub use checked::{
    CollapseCatalog, admit_collapse_snapshot, admit_snapshot_storage, has_incoming_checked,
};

use crate::{Document, EditError, Transaction, connections};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Property {
        index: usize,
        key: String,
    },
    Connection {
        index: usize,
        output: String,
        field: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub source: u32,
    pub location: Location,
    pub value: String,
    pub target: String,
    pub span: Range<usize>,
    pub declared: bool,
    pub certain: bool,
    pub reason: String,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub reference: Reference,
    pub after: String,
    pub changed: bool,
    pub review: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub revision: u64,
    pub entity: u32,
    pub old_name: String,
    pub new_name: String,
    pub entities: Vec<u32>,
    pub references: Vec<Change>,
    pub error: Option<String>,
}

fn reference_kind(kind: &str) -> bool {
    matches!(kind, "target_destination" | "target_name_or_class")
}

#[derive(Clone, Copy)]
enum BorrowedLocation<'a> {
    Property {
        index: usize,
        key: &'a str,
    },
    Connection {
        index: usize,
        output: &'a str,
        field: usize,
    },
}

struct BorrowedReference<'a> {
    source: u32,
    location: BorrowedLocation<'a>,
    value: &'a str,
    target: &'a str,
    span: Range<usize>,
    declared: bool,
    certain: bool,
    reason: &'a str,
    hidden: bool,
}

impl BorrowedReference<'_> {
    fn to_owned(&self) -> Reference {
        Reference {
            source: self.source,
            location: match self.location {
                BorrowedLocation::Property { index, key } => Location::Property {
                    index,
                    key: key.into(),
                },
                BorrowedLocation::Connection {
                    index,
                    output,
                    field,
                } => Location::Connection {
                    index,
                    output: output.into(),
                    field,
                },
            },
            value: self.value.into(),
            target: self.target.into(),
            span: self.span.clone(),
            declared: self.declared,
            certain: self.certain,
            reason: self.reason.into(),
            hidden: self.hidden,
        }
    }
}

fn incoming_match(source: u32, value: &str, declared: bool, target: u32, name: &str) -> bool {
    if value == "!self" && declared {
        source == target
    } else if declared {
        connections::name_matches(name, value)
    } else {
        !name.is_empty() && value.eq_ignore_ascii_case(name)
    }
}

fn properties_work(entity: &source_formats::vmf::Entity, key: &str) -> usize {
    entity.properties.0.iter().fold(0usize, |work, (name, _)| {
        work.saturating_add(name.len())
            .saturating_add(key.len())
            .saturating_add(1)
    })
}

struct ScanWork<'a> {
    remaining: usize,
    checkpoint: Option<&'a mut dyn FnMut() -> Result<(), String>>,
}

impl ScanWork<'_> {
    fn properties(
        &mut self,
        entity: &source_formats::vmf::Entity,
        key: &str,
    ) -> Result<(), String> {
        if self.checkpoint.is_some() {
            self.charge(properties_work(entity, key))?;
        }
        Ok(())
    }

    fn charge(&mut self, amount: usize) -> Result<(), String> {
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint()?;
            self.remaining = self
                .remaining
                .checked_sub(amount)
                .ok_or("Reference inspection exceeds its work limit")?;
        }
        Ok(())
    }
}

fn scan_visit<'a>(
    document: &'a Document,
    class: &impl Fn(&str) -> Option<&'a Class>,
    catalog_work: (usize, usize),
    work: &mut ScanWork<'_>,
    visit: &mut impl FnMut(BorrowedReference<'_>, &mut ScanWork<'_>) -> Result<bool, String>,
) -> Result<bool, String> {
    let lookup = |name: &str, work: &mut ScanWork<'_>| {
        work.charge(
            catalog_work
                .0
                .saturating_add(catalog_work.1.saturating_mul(name.len())),
        )?;
        Ok::<_, String>(class(name))
    };
    let checked = work.checkpoint.is_some();
    let target_work = if checked {
        document.vmf().entities.iter().fold(0usize, |sum, entity| {
            sum.saturating_add(properties_work(entity, "targetname"))
                .saturating_add(1)
        })
    } else {
        0
    };
    for entity in &document.vmf().entities {
        work.properties(entity, "classname")?;
        let class = entity
            .classname()
            .map(|name| lookup(name, work))
            .transpose()?
            .flatten();
        for (index, (key, value)) in entity.properties.0.iter().enumerate() {
            if let Some(class) = class.filter(|_| checked) {
                work.charge(class.properties.iter().fold(0usize, |sum, property| {
                    sum.saturating_add(property.name.len())
                        .saturating_add(key.len())
                        .saturating_add(1)
                }))?;
            } else {
                work.charge(1)?;
            }
            let property = class.and_then(|class| class.property(key));
            if key.eq_ignore_ascii_case("targetname")
                || key.eq_ignore_ascii_case("classname")
                || property.is_some_and(|property| property.kind == "target_source")
                || value.is_empty()
            {
                continue;
            }
            work.charge(value.len().saturating_add(key.len()))?;
            let parent = key.eq_ignore_ascii_case("parentname");
            let target = if parent {
                value
                    .split_once(',')
                    .map_or(value.as_str(), |(name, _)| name)
            } else {
                value
            };
            let declared =
                parent || property.is_some_and(|property| reference_kind(&property.kind));
            let mut certain = declared;
            let mut reason = if certain {
                property.map_or("Parent reference", |property| property.label.as_str())
            } else {
                "Text match; this property is not declared as an entity reference"
            };
            if property.is_some_and(|property| property.read_only) {
                certain = false;
                reason = "Read-only FGD property; review manually";
            }
            if property.is_some_and(|property| property.kind == "target_name_or_class") {
                for other in &document.vmf().entities {
                    work.properties(other, "classname")?;
                    work.charge(target.len())?;
                    if other
                        .classname()
                        .is_some_and(|name| name.eq_ignore_ascii_case(target))
                    {
                        certain = false;
                        reason = "May address an entity class rather than a name; review manually";
                        break;
                    }
                }
            }
            if visit(
                BorrowedReference {
                    source: entity.id,
                    location: BorrowedLocation::Property { index, key },
                    value,
                    target,
                    span: 0..target.len(),
                    declared,
                    certain,
                    reason,
                    hidden: entity.hidden,
                },
                work,
            )? {
                return Ok(true);
            }
        }
        for (index, connection) in entity.connections.iter().flatten().enumerate() {
            work.charge(
                connection
                    .value
                    .len()
                    .saturating_add(connection.output.len())
                    .saturating_add(1),
            )?;
            let separator = connections::separator(connection);
            let fields: Vec<_> = connection.value.split(separator).take(6).collect();
            for field in [0, 2] {
                let Some(target) = fields.get(field).filter(|target| !target.is_empty()) else {
                    continue;
                };
                let certain = if field == 0 {
                    fields.len() == 5
                } else if fields.len() != 5 {
                    false
                } else if fields[1].eq_ignore_ascii_case("SetParent") {
                    true
                } else {
                    work.charge(
                        target_work.saturating_add(
                            document
                                .vmf()
                                .entities
                                .len()
                                .saturating_mul(fields[0].len()),
                        ),
                    )?;
                    let targets = connections::targets(document, entity.id, fields[0]);
                    let mut certain = !targets.is_empty();
                    for id in targets {
                        work.charge(document.vmf().entities.len())?;
                        let Some(target_entity) = document.entity(id) else {
                            certain = false;
                            break;
                        };
                        work.properties(target_entity, "classname")?;
                        let target_class = target_entity
                            .classname()
                            .map(|name| lookup(name, work))
                            .transpose()?
                            .flatten();
                        if let Some(target_class) = target_class.filter(|_| checked) {
                            work.charge(target_class.inputs.iter().fold(0usize, |sum, input| {
                                sum.saturating_add(input.name.len())
                                    .saturating_add(fields[1].len())
                                    .saturating_add(1)
                            }))?;
                        }
                        if !target_class
                            .and_then(|class| {
                                class
                                    .inputs
                                    .iter()
                                    .find(|input| input.name.eq_ignore_ascii_case(fields[1]))
                            })
                            .is_some_and(|input| reference_kind(&input.kind))
                        {
                            certain = false;
                            break;
                        }
                    }
                    certain
                };
                let start = fields[..field]
                    .iter()
                    .map(|value| value.len() + separator.len_utf8())
                    .sum();
                if visit(
                    BorrowedReference {
                        source: entity.id,
                        location: BorrowedLocation::Connection {
                            index,
                            output: &connection.output,
                            field,
                        },
                        value: &connection.value,
                        target,
                        span: start..start + target.len(),
                        declared: field == 0 || certain,
                        certain,
                        reason: if fields.len() != 5 {
                            "Malformed imported connection; review manually"
                        } else if field == 0 {
                            "Output target"
                        } else if certain {
                            "Entity reference in the input parameter"
                        } else {
                            "Parameter text match; its reference semantics are unknown"
                        },
                        hidden: entity.hidden,
                    },
                    work,
                )? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub fn scan(document: &Document, catalog: &Catalog) -> Vec<Reference> {
    let mut references = Vec::new();
    let mut work = ScanWork {
        remaining: 0,
        checkpoint: None,
    };
    scan_visit(
        document,
        &|name| catalog.class(name),
        (0, 0),
        &mut work,
        &mut |reference, _| {
            references.push(reference.to_owned());
            Ok(false)
        },
    )
    .expect("Unbounded reference collection is infallible");
    references
}

pub fn incoming(
    document: &Document,
    catalog: &Catalog,
    target: u32,
) -> Result<Vec<Reference>, String> {
    let entity = document.entity(target).ok_or("Entity no longer exists")?;
    let name = entity.properties.get("targetname").unwrap_or_default();
    Ok(scan(document, catalog)
        .into_iter()
        .filter(|reference| {
            incoming_match(
                reference.source,
                &reference.target,
                reference.declared,
                target,
                name,
            )
        })
        .collect())
}

pub fn plan(
    document: &Document,
    catalog: &Catalog,
    entity: u32,
    new_name: &str,
    group: bool,
) -> Result<RenamePlan, String> {
    let source = document.entity(entity).ok_or("Entity no longer exists")?;
    let old_name = source
        .properties
        .get("targetname")
        .unwrap_or_default()
        .to_owned();
    let shared: Vec<_> = document
        .vmf()
        .entities
        .iter()
        .filter(|entity| {
            !old_name.is_empty()
                && entity
                    .properties
                    .get("targetname")
                    .is_some_and(|name| name.eq_ignore_ascii_case(&old_name))
        })
        .map(|entity| entity.id)
        .collect();
    let entities = if group && !shared.is_empty() {
        shared.clone()
    } else {
        vec![entity]
    };
    let error = if new_name.trim().is_empty() || new_name != new_name.trim() {
        Some("Enter a nonempty name without leading or trailing whitespace".into())
    } else if new_name.starts_with('!')
        || new_name.contains(['*', ',', '"'])
        || new_name.chars().any(char::is_control)
    {
        Some("Names cannot contain wildcards, commas, quotes or controls, or start with !".into())
    } else if shared.len() > 1 && !group && new_name != old_name {
        Some(format!(
            "{old_name} is used by {} entities. Choose to rename all of them to preserve their links.",
            shared.len()
        ))
    } else if new_name != old_name
        && document.vmf().entities.iter().any(|entity| {
            !entities.contains(&entity.id)
                && entity
                    .properties
                    .get("targetname")
                    .is_some_and(|name| name.eq_ignore_ascii_case(new_name))
        })
    {
        Some("Another entity already uses that name. Choose an unused name.".into())
    } else if entities.iter().any(|id| {
        let entity = document.entity(*id).unwrap();
        entity.properties.get_all("targetname").count() > 1
            || entity
                .classname()
                .and_then(|name| catalog.class(name))
                .and_then(|class| class.property("targetname"))
                .is_some_and(|property| property.read_only || property.validate(new_name).is_err())
    }) {
        Some("An entity has duplicate name fields or a name its definition does not allow. Fix it before renaming.".into())
    } else {
        None
    };
    let mut references = Vec::new();
    for reference in scan(document, catalog) {
        let exact = !old_name.is_empty() && reference.target.eq_ignore_ascii_case(&old_name);
        let old_match = connections::name_matches(&old_name, &reference.target);
        let new_match = connections::name_matches(new_name, &reference.target);
        let self_match = reference.declared
            && reference.target == "!self"
            && entities.contains(&reference.source);
        if !(exact || (reference.declared && (old_match || new_match)) || self_match) {
            continue;
        }
        let mut after = reference.value.clone();
        let mut review = false;
        let reason = if reference.certain
            && exact
            && !reference.target.contains('*')
            && !reference.target.starts_with('!')
            && new_name != old_name
        {
            after.replace_range(reference.span.clone(), new_name);
            reference.reason.clone()
        } else if new_name == old_name || self_match {
            "Unchanged; this reference continues to resolve".into()
        } else if !reference.certain {
            review = true;
            reference.reason.clone()
        } else if reference.target.contains('*') {
            review = old_match != new_match;
            if review {
                format!(
                    "Wildcard left unchanged; the renamed group will {} this match",
                    if new_match { "join" } else { "leave" }
                )
            } else {
                "Wildcard left unchanged; it still matches the renamed group".into()
            }
        } else {
            review = true;
            "Existing reference to the new name will start addressing the renamed group".into()
        };
        references.push(Change {
            changed: after != reference.value,
            reference,
            after,
            review,
            reason,
        });
    }
    let mut final_values = BTreeMap::new();
    for change in references.iter().rev().filter(|change| change.changed) {
        let key = record_key(&change.reference);
        final_values
            .entry(key)
            .or_insert_with(|| change.reference.value.clone())
            .replace_range(change.reference.span.clone(), new_name);
    }
    for change in &mut references {
        if let Some(value) = final_values.get(&record_key(&change.reference)) {
            change.after = value.clone();
        }
    }
    Ok(RenamePlan {
        revision: document.revision(),
        entity,
        old_name,
        new_name: new_name.into(),
        entities,
        references,
        error,
    })
}

fn record_key(reference: &Reference) -> (u32, bool, usize) {
    match reference.location {
        Location::Property { index, .. } => (reference.source, true, index),
        Location::Connection { index, .. } => (reference.source, false, index),
    }
}

pub fn apply(
    tx: &mut Transaction,
    plan: &RenamePlan,
    accept_review: bool,
) -> Result<(), EditError> {
    if tx.document().revision() != plan.revision {
        return Err(EditError::Invalid(
            "Map changed; refresh the rename preview".into(),
        ));
    }
    if let Some(error) = &plan.error {
        return Err(EditError::Invalid(error.clone()));
    }
    if !accept_review && plan.references.iter().any(|change| change.review) {
        return Err(EditError::Invalid(
            "Review the unchanged references before applying".into(),
        ));
    }
    for id in &plan.entities {
        let entity = tx.entity_mut(*id)?;
        if !entity
            .properties
            .get("targetname")
            .unwrap_or_default()
            .eq_ignore_ascii_case(&plan.old_name)
        {
            return Err(EditError::Invalid(
                "Entity name changed; refresh the preview".into(),
            ));
        }
        if entity.properties.get("targetname") != Some(plan.new_name.as_str()) {
            entity.properties.set("targetname", plan.new_name.clone());
        }
    }
    for change in plan.references.iter().rev() {
        if !change.changed {
            continue;
        }
        let entity = tx.entity_mut(change.reference.source)?;
        let value = match &change.reference.location {
            Location::Property { index, key } => entity
                .properties
                .0
                .get_mut(*index)
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value),
            Location::Connection { index, output, .. } => entity
                .connections
                .as_mut()
                .and_then(|connections| connections.get_mut(*index))
                .filter(|connection| &connection.output == output)
                .map(|connection| &mut connection.value),
        }
        .ok_or_else(|| EditError::Invalid("Reference changed; refresh the preview".into()))?;
        // Multiple fields in one connection are replaced from right to left.
        if value.get(change.reference.span.clone()) != Some(change.reference.target.as_str()) {
            return Err(EditError::Invalid(
                "Reference changed; refresh the preview".into(),
            ));
        }
        value.replace_range(change.reference.span.clone(), &plan.new_name);
    }
    Ok(())
}
