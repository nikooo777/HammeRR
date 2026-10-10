use std::collections::BTreeMap;
use std::ops::Range;

use source_formats::fgd::Catalog;

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

pub fn scan(document: &Document, catalog: &Catalog) -> Vec<Reference> {
    let mut references = Vec::new();
    for entity in &document.vmf().entities {
        let class = entity.classname().and_then(|name| catalog.class(name));
        for (index, (key, value)) in entity.properties.0.iter().enumerate() {
            let property = class.and_then(|class| class.property(key));
            if key.eq_ignore_ascii_case("targetname")
                || key.eq_ignore_ascii_case("classname")
                || property.is_some_and(|property| property.kind == "target_source")
                || value.is_empty()
            {
                continue;
            }
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
                property.map_or_else(
                    || "Parent reference".into(),
                    |property| property.label.clone(),
                )
            } else {
                "Text match; this property is not declared as an entity reference".into()
            };
            if property.is_some_and(|property| property.read_only) {
                certain = false;
                reason = "Read-only FGD property; review manually".into();
            }
            if property.is_some_and(|property| property.kind == "target_name_or_class")
                && document.vmf().entities.iter().any(|entity| {
                    entity
                        .classname()
                        .is_some_and(|name| name.eq_ignore_ascii_case(target))
                })
            {
                certain = false;
                reason = "May address an entity class rather than a name; review manually".into();
            }
            references.push(Reference {
                source: entity.id,
                location: Location::Property {
                    index,
                    key: key.clone(),
                },
                value: value.clone(),
                target: target.into(),
                span: 0..target.len(),
                declared,
                certain,
                reason,
                hidden: entity.hidden,
            });
        }
        for (index, connection) in entity.connections.iter().flatten().enumerate() {
            let separator = connections::separator(connection);
            let fields: Vec<_> = connection.value.split(separator).collect();
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
                    let targets = connections::targets(document, entity.id, fields[0]);
                    !targets.is_empty()
                        && targets.iter().all(|id| {
                            document
                                .entity(*id)
                                .and_then(|entity| entity.classname())
                                .and_then(|name| catalog.class(name))
                                .and_then(|class| {
                                    class
                                        .inputs
                                        .iter()
                                        .find(|input| input.name.eq_ignore_ascii_case(fields[1]))
                                })
                                .is_some_and(|input| reference_kind(&input.kind))
                        })
                };
                let start = fields[..field]
                    .iter()
                    .map(|value| value.len() + separator.len_utf8())
                    .sum();
                references.push(Reference {
                    source: entity.id,
                    location: Location::Connection {
                        index,
                        output: connection.output.clone(),
                        field,
                    },
                    value: connection.value.clone(),
                    target: (*target).into(),
                    span: start..start + target.len(),
                    declared: field == 0 || certain,
                    certain,
                    reason: if fields.len() != 5 {
                        "Malformed imported connection; review manually".into()
                    } else if field == 0 {
                        "Output target".into()
                    } else if certain {
                        "Entity reference in the input parameter".into()
                    } else {
                        "Parameter text match; its reference semantics are unknown".into()
                    },
                    hidden: entity.hidden,
                });
            }
        }
    }
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
            if reference.target == "!self" && reference.declared {
                reference.source == target
            } else if reference.declared {
                connections::name_matches(name, &reference.target)
            } else {
                !name.is_empty() && reference.target.eq_ignore_ascii_case(name)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::{keyvalues::WriteOptions, vmf::Vmf};

    const FGD: &str = r#"
@BaseClass = Named [ targetname(target_source) : "Name" ]
@PointClass base(Named) = logic_relay [
 target(target_destination) : "Target"
 parentname(target_destination) : "Parent"
 classlink(target_name_or_class) : "Entity or class"
 input SetParent(string) : "Parent"
 input CustomLink(target_destination) : "Link"
]
@SolidClass base(Named) = func_door []
"#;

    fn fixture() -> (Document, Catalog) {
        let vmf = Vmf::parse(
            r#"
world { "id" "1" "classname" "worldspawn" }
entity { "id" "2" "classname" "logic_relay" "targetname" "relay"
 "target" "Door" "target" "DOOR" "parentname" "door,hinge"
 "notes" "Door" "other" "door_extra"
 connections {
  "OnTrigger" "Door,SetParent,DOOR,0.00,01"
  "OnTrigger" "Door,Open,,0.000,-1"
  "OnTrigger" "Door,Open,,0.000,-1"
  "OnTrigger" "door*,Open,,0,-1"
  "OnTrigger" "!self,SetParent,Door,0,-1"
  "Malformed" "Door,broken"
  "Custom" "unchanged"
 }
 custom { "keep" "Door" }
}
entity { "id" "3" "classname" "func_door" "targetname" "Door" }
entity { "id" "4" "classname" "func_door" "targetname" "door_extra" }
hidden { entity { "id" "5" "classname" "logic_relay" "target" "Door" } }
"#,
        )
        .unwrap();
        (Document::new(vmf), Catalog::parse(FGD).unwrap())
    }

    #[test]
    fn atomic_rename_preserves_duplicates_raw_fields_unknown_data_and_history() {
        let (mut document, catalog) = fixture();
        let original = document.vmf().write(&WriteOptions::default());
        let plan = plan(&document, &catalog, 3, "gate", false).unwrap();
        assert!(plan.error.is_none());
        assert!(plan.references.iter().any(|change| matches!(
            change.reference.location,
            Location::Connection {
                index: 0,
                field: 0,
                ..
            }
        ) && change.after
            == "gate,SetParent,gate,0.00,01"));
        assert_eq!(
            plan.references
                .iter()
                .filter(|change| change.review)
                .count(),
            3
        );
        assert!(
            plan.references
                .iter()
                .any(|change| change.reference.hidden && change.after == "gate")
        );
        assert!(
            document
                .edit("unreviewed", |tx| apply(tx, &plan, false))
                .is_err()
        );
        assert_eq!(document.revision(), 0);
        document
            .edit("rename", |tx| apply(tx, &plan, true))
            .unwrap();
        assert_eq!(document.undo_labels().count(), 1);
        let relay = document.entity(2).unwrap();
        assert_eq!(
            relay.properties.get_all("target").collect::<Vec<_>>(),
            ["gate", "gate"]
        );
        assert_eq!(relay.properties.get("parentname"), Some("gate,hinge"));
        assert_eq!(relay.properties.get("notes"), Some("Door"));
        assert_eq!(relay.properties.get("other"), Some("door_extra"));
        let connections = relay.connections.as_ref().unwrap();
        assert_eq!(connections[0].value, "gate,SetParent,gate,0.00,01");
        assert_eq!(connections[1].value, "gate,Open,,0.000,-1");
        assert_eq!(connections[1], connections[2]);
        assert_eq!(connections[3].value, "door*,Open,,0,-1");
        assert_eq!(connections[4].value, "!self,SetParent,gate,0,-1");
        assert_eq!(connections[5].value, "Door,broken");
        assert_eq!(connections[6].value, "unchanged");
        let saved = document.vmf().write(&WriteOptions::default());
        assert!(saved.contains("\"keep\" \"Door\""));
        let reopened = Vmf::parse(&saved).unwrap();
        assert_eq!(reopened.entities, document.vmf().entities);
        document.undo().unwrap();
        assert_eq!(document.vmf().write(&WriteOptions::default()), original);
        document.redo().unwrap();
        assert_eq!(document.vmf().write(&WriteOptions::default()), saved);
    }

    #[test]
    fn shared_names_collisions_and_invalid_names_do_not_mutate_the_map() {
        let (mut document, catalog) = fixture();
        document
            .edit("share", |tx| {
                tx.entity_mut(4)?.properties.set("targetname", "door");
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        for (name, group) in [
            ("gate", false),
            ("relay", true),
            ("!self", true),
            ("gate*", true),
            ("gate,hinge", true),
            ("bad\nname", true),
            ("", true),
            (" gate", true),
        ] {
            let preview = plan(&document, &catalog, 3, name, group).unwrap();
            assert!(preview.error.is_some(), "{name}");
            assert!(
                document
                    .edit("invalid", |tx| apply(tx, &preview, true))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
        }
        let preview = plan(&document, &catalog, 3, "gate", true).unwrap();
        assert_eq!(preview.entities, [3, 4]);
        document
            .edit("group", |tx| apply(tx, &preview, true))
            .unwrap();
        assert_eq!(
            document.entity(3).unwrap().properties.get("targetname"),
            Some("gate")
        );
        assert_eq!(
            document.entity(4).unwrap().properties.get("targetname"),
            Some("gate")
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn escape_imports_and_new_name_wildcard_acquisitions_are_preserved() {
        let (mut document, catalog) = fixture();
        document
            .edit("import", |tx| {
                let list = tx.entity_mut(2)?.connections.as_mut().unwrap();
                list[0].value = "Door\u{1b}SetParent\u{1b}DOOR\u{1b}0.00\u{1b}01".into();
                list[1].value = "gate*\u{1b}Open\u{1b}a,b\u{1b}0.25\u{1b}-1".into();
                Ok(())
            })
            .unwrap();
        let preview = plan(&document, &catalog, 3, "gate", false).unwrap();
        assert!(
            preview
                .references
                .iter()
                .any(|change| change.reference.target == "gate*" && change.review)
        );
        document
            .edit("rename", |tx| apply(tx, &preview, true))
            .unwrap();
        let list = document.entity(2).unwrap().connections.as_ref().unwrap();
        assert_eq!(
            list[0].value,
            "gate\u{1b}SetParent\u{1b}gate\u{1b}0.00\u{1b}01"
        );
        assert_eq!(list[1].value, "gate*\u{1b}Open\u{1b}a,b\u{1b}0.25\u{1b}-1");
    }

    #[test]
    fn case_rename_noop_and_class_ambiguity() {
        let (mut document, catalog) = fixture();
        let noop = plan(&document, &catalog, 3, "Door", false).unwrap();
        document.edit("noop", |tx| apply(tx, &noop, false)).unwrap();
        assert_eq!(document.revision(), 0);
        let case = plan(&document, &catalog, 3, "DOOR", false).unwrap();
        assert!(
            !case
                .references
                .iter()
                .find(|change| change.reference.target == "door*")
                .unwrap()
                .review
        );
        document.edit("case", |tx| apply(tx, &case, true)).unwrap();
        assert_eq!(
            document.entity(2).unwrap().properties.get("target"),
            Some("DOOR")
        );
        assert!(document.edit("stale", |tx| apply(tx, &case, true)).is_err());
        document
            .edit("class reference", |tx| {
                tx.entity_mut(2)?.properties.set("classlink", "logic_relay");
                Ok(())
            })
            .unwrap();
        let acquisition = plan(&document, &catalog, 3, "logic_relay", false).unwrap();
        assert!(
            acquisition
                .references
                .iter()
                .any(|change| change.reference.target == "logic_relay" && change.review)
        );
        document
            .edit("class name", |tx| {
                tx.entity_mut(3)?
                    .properties
                    .set("targetname", "logic_relay");
                tx.entity_mut(2)?.properties.set("classlink", "logic_relay");
                Ok(())
            })
            .unwrap();
        let preview = plan(&document, &catalog, 3, "controller", false).unwrap();
        let link = preview
            .references
            .iter()
            .find(|change| change.reference.target == "logic_relay")
            .unwrap();
        assert!(link.review);
        assert_eq!(link.after, "logic_relay");
    }
}
