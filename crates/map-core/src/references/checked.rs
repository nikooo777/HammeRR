use std::mem::size_of;

use source_formats::fgd::{Catalog, Class, Io, Property};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Properties, Solid, VisGroup, Vmf};

use super::{ScanWork, incoming_match, properties_work, scan_visit};
use crate::Document;
use crate::metadata::{InputBudget, MAX_DEPTH, MAX_NODES};

const MAX_STORAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SCAN_WORK: usize = 64 * 1024 * 1024;
const MAX_TARGETS: usize = 256;

struct Admission<'a> {
    input: InputBudget,
    bytes: usize,
    items: usize,
    limit: usize,
    checkpoint: &'a mut dyn FnMut() -> Result<(), String>,
}

impl Admission<'_> {
    fn bytes(&mut self, bytes: usize) -> Result<(), String> {
        (self.checkpoint)()?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= self.limit)
            .ok_or("Instance collapse exceeds its snapshot or plan storage limit")?;
        Ok(())
    }

    fn array<T>(&mut self, len: usize) -> Result<(), String> {
        self.items = self
            .items
            .checked_add(len)
            .filter(|items| *items <= MAX_NODES)
            .ok_or("Instance collapse exceeds 131072 structural items")?;
        self.bytes(len.saturating_mul(size_of::<T>()))
    }

    fn text(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.bytes(key.len().saturating_add(value.len()))?;
        self.input
            .text(key, value)
            .map_err(|error| error.to_string())
    }

    fn properties(&mut self, properties: &Properties) -> Result<(), String> {
        self.array::<(String, String)>(properties.0.len())?;
        for (key, value) in &properties.0 {
            self.text(key, value)?;
        }
        Ok(())
    }

    fn optional_properties(&mut self, properties: Option<&Properties>) -> Result<(), String> {
        if let Some(properties) = properties {
            self.properties(properties)?;
        }
        Ok(())
    }

    fn nodes(&mut self, nodes: &[Node], depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH && !nodes.is_empty() {
            return Err("Metadata exceeds 64 levels".into());
        }
        self.array::<Node>(nodes.len())?;
        for node in nodes {
            self.text(&node.key, node.as_str().unwrap_or_default())?;
            if let Some(children) = node.children() {
                self.nodes(children, depth + 1)?;
            }
        }
        Ok(())
    }

    fn groups(&mut self, groups: &[VisGroup], depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH && !groups.is_empty() {
            return Err("Visgroups exceed 64 levels".into());
        }
        self.array::<VisGroup>(groups.len())?;
        for group in groups {
            self.text("", &group.name)?;
            self.properties(&group.extra_properties)?;
            self.nodes(&group.extra, 1)?;
            self.groups(&group.children, depth + 1)?;
        }
        Ok(())
    }

    fn rows<T>(&mut self, rows: Option<&[Vec<T>]>) -> Result<(), String> {
        if let Some(rows) = rows {
            self.array::<Vec<T>>(rows.len())?;
            for row in rows {
                self.bytes(row.len().saturating_mul(size_of::<T>()))?;
            }
        }
        Ok(())
    }

    fn solids(&mut self, solids: &[Solid]) -> Result<(), String> {
        self.array::<Solid>(solids.len())?;
        for solid in solids {
            self.optional_properties(solid.editor.as_ref())?;
            self.properties(&solid.extra_properties)?;
            self.nodes(&solid.extra, 1)?;
            self.array::<source_formats::vmf::Side>(solid.sides.len())?;
            for side in &solid.sides {
                self.text("", &side.material)?;
                self.properties(&side.extra_properties)?;
                self.nodes(&side.extra, 1)?;
                if let Some(displacement) = &side.displacement {
                    self.rows(displacement.normals.as_deref())?;
                    self.rows(displacement.distances.as_deref())?;
                    self.rows(displacement.offsets.as_deref())?;
                    self.rows(displacement.offset_normals.as_deref())?;
                    self.rows(displacement.alphas.as_deref())?;
                    self.rows(displacement.triangle_tags.as_deref())?;
                    if let Some(vertices) = &displacement.allowed_verts {
                        self.bytes(vertices.len().saturating_mul(size_of::<i32>()))?;
                    }
                    self.properties(&displacement.extra_properties)?;
                    self.nodes(&displacement.extra, 1)?;
                }
            }
        }
        Ok(())
    }
}

pub fn admit_collapse_snapshot(
    vmf: &Vmf,
    limit: usize,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<usize, String> {
    let mut budget = Admission {
        input: InputBudget::default(),
        bytes: size_of::<Vmf>(),
        items: 0,
        limit: limit.min(MAX_STORAGE_BYTES),
        checkpoint,
    };
    budget.bytes(0)?;
    budget.optional_properties(vmf.version_info.as_ref())?;
    budget.optional_properties(vmf.view_settings.as_ref())?;
    budget.groups(vmf.visgroups.as_deref().unwrap_or_default(), 1)?;
    budget.nodes(&vmf.visgroups_extra, 1)?;
    budget.nodes(&vmf.extra, 1)?;
    for node in vmf.cameras.iter().chain(vmf.cordon.iter()) {
        budget.nodes(std::slice::from_ref(node), 1)?;
    }
    budget.properties(&vmf.world.properties)?;
    budget.solids(&vmf.world.solids)?;
    budget.array::<source_formats::vmf::Group>(vmf.world.groups.len())?;
    for group in &vmf.world.groups {
        budget.optional_properties(group.editor.as_ref())?;
        budget.properties(&group.extra_properties)?;
        budget.nodes(&group.extra, 1)?;
    }
    budget.nodes(&vmf.world.extra, 1)?;
    budget.array::<Entity>(vmf.entities.len())?;
    for entity in &vmf.entities {
        budget.bytes(entity.layout_storage_bytes())?;
        budget.properties(&entity.properties)?;
        budget.optional_properties(entity.editor.as_ref())?;
        budget.nodes(&entity.extra, 1)?;
        let connections = entity.connections.as_deref().unwrap_or_default();
        budget.array::<source_formats::vmf::Connection>(connections.len())?;
        for connection in connections {
            budget.text(&connection.output, &connection.value)?;
        }
        budget.solids(&entity.solids)?;
    }
    Ok(budget.bytes)
}

pub struct CollapseCatalog {
    classes: Vec<Class>,
    name_bytes: usize,
}

impl CollapseCatalog {
    pub fn capture(catalog: &Catalog) -> Result<Self, String> {
        let mut checkpoint = || Ok(());
        let mut budget = Admission {
            input: InputBudget::default(),
            bytes: 0,
            items: 0,
            limit: MAX_STORAGE_BYTES,
            checkpoint: &mut checkpoint,
        };
        let mut classes = Vec::new();
        let mut name_bytes = 0usize;
        for class in catalog.classes() {
            budget.array::<Class>(1)?;
            budget.text("", &class.name)?;
            budget.array::<Property>(class.properties.len())?;
            budget.array::<Io>(class.inputs.len())?;
            for property in &class.properties {
                budget.text(&property.name, &property.kind)?;
            }
            for input in &class.inputs {
                budget.text(&input.name, &input.kind)?;
            }
            name_bytes += class.name.len() + 1;
            classes.push(Class {
                name: class.name.clone(),
                kind: class.kind,
                description: String::new(),
                bases: Vec::new(),
                helpers: Vec::new(),
                outputs: Vec::new(),
                properties: class
                    .properties
                    .iter()
                    .map(|property| Property {
                        name: property.name.clone(),
                        kind: property.kind.clone(),
                        label: String::new(),
                        description: String::new(),
                        default: None,
                        choices: Vec::new(),
                        read_only: property.read_only,
                    })
                    .collect(),
                inputs: class
                    .inputs
                    .iter()
                    .map(|input| Io {
                        name: input.name.clone(),
                        kind: input.kind.clone(),
                        description: String::new(),
                    })
                    .collect(),
            });
        }
        Ok(Self {
            classes,
            name_bytes,
        })
    }
}

pub fn has_incoming_checked(
    document: &Document,
    catalog: &CollapseCatalog,
    targets: &[u32],
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<bool, String> {
    has_incoming_with_work(document, catalog, targets, MAX_SCAN_WORK, checkpoint)
}

fn has_incoming_with_work(
    document: &Document,
    catalog: &CollapseCatalog,
    targets: &[u32],
    limit: usize,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<bool, String> {
    admit_collapse_snapshot(document.vmf(), MAX_STORAGE_BYTES, checkpoint)?;
    if targets.len() > MAX_TARGETS {
        return Err("Reference inspection exceeds 256 targets".into());
    }
    let mut work = ScanWork {
        remaining: limit,
        checkpoint: Some(checkpoint),
    };
    let mut names = Vec::with_capacity(targets.len());
    for &id in targets {
        work.charge(document.vmf().entities.len())?;
        let entity = document.entity(id).ok_or("Entity no longer exists")?;
        work.charge(properties_work(entity, "targetname"))?;
        names.push((id, entity.properties.get("targetname").unwrap_or_default()));
    }
    scan_visit(
        document,
        &|name| {
            catalog
                .classes
                .iter()
                .find(|class| class.name.eq_ignore_ascii_case(name))
        },
        (catalog.name_bytes, catalog.classes.len()),
        &mut work,
        &mut |reference, work| {
            for &(id, name) in &names {
                work.charge(
                    name.len()
                        .saturating_add(reference.target.len())
                        .saturating_add(1),
                )?;
                if incoming_match(
                    reference.source,
                    reference.target,
                    reference.declared,
                    id,
                    name,
                ) {
                    return Ok(true);
                }
            }
            Ok(false)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::references;

    fn fixture() -> (Document, Catalog) {
        let catalog = Catalog::parse(
            r#"
@BaseClass = Links [ target(target_destination) : "Target" input Link(target_destination) : "Link" ]
@PointClass base(Links) = logic_relay []
@SolidClass = func_instance []
"#,
        )
        .unwrap();
        let vmf = Vmf::parse(
            r#"
world { "id" "1" "classname" "worldspawn" }
entity { "id" "2" "classname" "logic_relay" "targetname" "sink"
 "notes" "Helper" "target" "Helper"
 connections { "OnTrigger" "sink,Link,Helper,0,-1" "OnTrigger" "!self,SetParent,Helper,0,-1" }
}
entity { "id" "3" "classname" "func_instance" "targetname" "Helper" }
entity { "id" "4" "classname" "func_instance" "targetname" "unused" }
hidden { entity { "id" "5" "classname" "logic_relay" "parentname" "Helper,hinge" } }
"#,
        )
        .unwrap();
        (Document::new(vmf), catalog)
    }

    #[test]
    fn checked_incoming_preserves_shared_inherited_hidden_and_uncertain_semantics() {
        let (document, catalog) = fixture();
        let captured = CollapseCatalog::capture(&catalog).unwrap();
        let references = references::incoming(&document, &catalog, 3).unwrap();
        assert!(references.iter().any(|reference| reference.hidden));
        assert!(references.iter().any(|reference| !reference.declared));
        assert!(references.iter().any(|reference| reference.certain
            && matches!(
                reference.location,
                references::Location::Connection {
                    index: 0,
                    field: 2,
                    ..
                }
            )));
        for targets in [&[3][..], &[4][..], &[4, 3][..], &[2][..]] {
            let expected = targets.iter().any(|target| {
                !references::incoming(&document, &catalog, *target)
                    .unwrap()
                    .is_empty()
            });
            assert_eq!(
                has_incoming_checked(&document, &captured, targets, &mut || Ok(())).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn checked_incoming_cancels_and_refuses_cumulative_work_without_mutation() {
        let (document, catalog) = fixture();
        let source = document.vmf().clone();
        let captured = CollapseCatalog::capture(&catalog).unwrap();
        let mut admitted_calls = 0;
        admit_collapse_snapshot(document.vmf(), MAX_STORAGE_BYTES, &mut || {
            admitted_calls += 1;
            Ok(())
        })
        .unwrap();
        let mut calls = 0;
        let error = has_incoming_checked(&document, &captured, &[4], &mut || {
            calls += 1;
            if calls > admitted_calls + 8 {
                Err("cancelled reference scan".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error, "cancelled reference scan");
        assert!(
            has_incoming_with_work(&document, &captured, &[4], 200, &mut || Ok(()))
                .unwrap_err()
                .contains("work limit")
        );
        assert!(!has_incoming_checked(&document, &captured, &[4], &mut || Ok(())).unwrap());
        assert_eq!(document.vmf(), &source);
        assert_eq!(document.undo_labels().count(), 0);
    }

    #[test]
    fn collapse_storage_admission_counts_layout_and_numeric_rows_before_copying() {
        let (document, _) = fixture();
        assert!(document.entity(2).unwrap().layout_storage_bytes() > 0);
        let admitted =
            admit_collapse_snapshot(document.vmf(), MAX_STORAGE_BYTES, &mut || Ok(())).unwrap();
        assert!(admit_collapse_snapshot(document.vmf(), admitted - 1, &mut || Ok(())).is_err());
        let mut checkpoint = || Ok(());
        let mut budget = Admission {
            input: InputBudget::default(),
            bytes: 0,
            items: 0,
            limit: 128,
            checkpoint: &mut checkpoint,
        };
        let rows = vec![vec![0f64; 16]];
        assert!(
            budget
                .rows(Some(&rows))
                .unwrap_err()
                .contains("storage limit")
        );
    }
}
