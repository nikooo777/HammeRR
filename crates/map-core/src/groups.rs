use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Group, Properties};

use crate::{Document, EditError, ObjectId, Transaction};

pub const MAX_GROUPS: usize = 8192;
pub const MAX_MEMBERS: usize = 8192;
pub const MAX_DEPTH: usize = 64;
pub const MAX_NAME_CHARS: usize = 128;
const PARENT_KEY: &str = "groupid";
const NAME_KEY: &str = "name";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Member {
    Object(ObjectId),
    Group(u32),
}

#[derive(Debug, Clone)]
pub struct Edit {
    pub group: Option<u32>,
    pub name: String,
    pub parent: Option<u32>,
    pub members: Vec<Member>,
    pub ungroup: bool,
}

#[derive(Debug, Clone)]
pub struct Graph {
    pub parents: BTreeMap<u32, Option<u32>>,
    pub objects: BTreeMap<ObjectId, Option<u32>>,
    counts: BTreeMap<u32, (usize, usize)>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn parent(editor: Option<&Properties>) -> Result<Option<u32>, EditError> {
    let Some(editor) = editor else {
        return Ok(None);
    };
    let mut values = editor.get_all(PARENT_KEY);
    let value = values.next();
    if values.next().is_some() {
        return Err(invalid(
            "Duplicate editor.groupid values; repair the imported membership first",
        ));
    }
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| invalid("Invalid editor.groupid value"))
        })
        .transpose()
}

pub fn name(group: &Group) -> &str {
    group
        .editor
        .as_ref()
        .and_then(|editor| editor.get(NAME_KEY))
        .unwrap_or("")
}

pub fn owned_brush_links(document: &Document, group: u32) -> Vec<u32> {
    document
        .vmf()
        .entities
        .iter()
        .flat_map(|entity| &entity.solids)
        .filter(|solid| {
            solid.editor.as_ref().is_some_and(|editor| {
                editor
                    .get_all(PARENT_KEY)
                    .any(|value| value.parse::<u32>().ok() == Some(group))
            })
        })
        .map(|solid| solid.id)
        .collect()
}

pub fn owned_brush_memberships(document: &Document) -> BTreeMap<u32, Vec<u32>> {
    let mut memberships: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for solid in document
        .vmf()
        .entities
        .iter()
        .flat_map(|entity| &entity.solids)
    {
        if let Some(editor) = &solid.editor {
            for group in editor
                .get_all(PARENT_KEY)
                .filter_map(|value| value.parse::<u32>().ok())
            {
                memberships.entry(group).or_default().push(solid.id);
            }
        }
    }
    memberships
}

impl Graph {
    pub fn read(document: &Document) -> Result<Self, EditError> {
        let vmf = document.vmf();
        if vmf.world.groups.len() > MAX_GROUPS {
            return Err(invalid(format!(
                "Grouping supports at most {MAX_GROUPS} group records"
            )));
        }
        let mut parents = BTreeMap::new();
        for group in &vmf.world.groups {
            if parents
                .insert(group.id, parent(group.editor.as_ref())?)
                .is_some()
            {
                return Err(invalid(format!("Duplicate group ID {}", group.id)));
            }
        }
        let mut objects = BTreeMap::new();
        for solid in &vmf.world.solids {
            objects.insert(ObjectId::Solid(solid.id), parent(solid.editor.as_ref())?);
        }
        for entity in &vmf.entities {
            objects.insert(ObjectId::Entity(entity.id), parent(entity.editor.as_ref())?);
        }
        let mut graph = Self {
            parents,
            objects,
            counts: BTreeMap::new(),
        };
        graph.validate()?;
        graph.counts = graph.parents.keys().map(|&id| (id, (0, 0))).collect();
        for &parent in graph.parents.values().flatten() {
            graph.counts.get_mut(&parent).unwrap().0 += 1;
        }
        for &parent in graph.objects.values().flatten() {
            graph.counts.get_mut(&parent).unwrap().0 += 1;
            let mut cursor = Some(parent);
            while let Some(id) = cursor {
                graph.counts.get_mut(&id).unwrap().1 += 1;
                cursor = graph.parents[&id];
            }
        }
        Ok(graph)
    }

    fn validate(&self) -> Result<(), EditError> {
        for parent in self.parents.values().chain(self.objects.values()).flatten() {
            if !self.parents.contains_key(parent) {
                return Err(invalid(format!("Missing parent group {parent}")));
            }
        }
        for &group in self.parents.keys() {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(group);
            while let Some(id) = cursor {
                if !seen.insert(id) {
                    return Err(invalid(format!(
                        "Group {group} would create a membership cycle"
                    )));
                }
                if seen.len() > MAX_DEPTH {
                    return Err(invalid(format!(
                        "Groups support at most {MAX_DEPTH} nested levels"
                    )));
                }
                cursor = self.parents[&id];
            }
        }
        Ok(())
    }

    pub fn direct_members(&self, group: u32) -> Result<Vec<Member>, EditError> {
        self.require(group)?;
        Ok(self
            .parents
            .iter()
            .filter(|(_, parent)| **parent == Some(group))
            .map(|(&id, _)| Member::Group(id))
            .chain(
                self.objects
                    .iter()
                    .filter(|(_, parent)| **parent == Some(group))
                    .map(|(&id, _)| Member::Object(id)),
            )
            .collect())
    }

    pub fn counts(&self, group: u32) -> (usize, usize) {
        self.counts.get(&group).copied().unwrap_or_default()
    }

    pub fn descendants(&self, group: u32) -> Result<Vec<ObjectId>, EditError> {
        self.require(group)?;
        Ok(self
            .objects
            .iter()
            .filter(|(_, parent)| self.contains_ancestor(**parent, group))
            .map(|(&id, _)| id)
            .collect())
    }

    fn contains_ancestor(&self, mut cursor: Option<u32>, group: u32) -> bool {
        while let Some(id) = cursor {
            if id == group {
                return true;
            }
            cursor = self.parents[&id];
        }
        false
    }

    pub fn root(&self, group: u32) -> Result<u32, EditError> {
        self.require(group)?;
        let mut root = group;
        while let Some(parent) = self.parents[&root] {
            root = parent;
        }
        Ok(root)
    }

    pub fn depth(&self, group: u32) -> Result<usize, EditError> {
        self.require(group)?;
        let mut depth = 0;
        let mut cursor = self.parents[&group];
        while let Some(parent) = cursor {
            depth += 1;
            cursor = self.parents[&parent];
        }
        Ok(depth)
    }

    fn require(&self, group: u32) -> Result<(), EditError> {
        if self.parents.contains_key(&group) {
            Ok(())
        } else {
            Err(invalid(format!("Group {group} does not exist")))
        }
    }

    pub fn selection_members(&self, ids: &[ObjectId]) -> Vec<Member> {
        let selected: BTreeSet<_> = ids.iter().copied().collect();
        let mut selected_counts: BTreeMap<u32, usize> = BTreeMap::new();
        for id in &selected {
            let mut cursor = self.objects.get(id).copied().flatten();
            while let Some(group) = cursor {
                *selected_counts.entry(group).or_default() += 1;
                cursor = self.parents[&group];
            }
        }
        let mut members = Vec::new();
        let mut groups: Vec<_> = self.parents.keys().copied().collect();
        groups.sort_by_key(|&group| (self.depth(group).unwrap_or(0), group));
        let mut included = BTreeSet::new();
        for group in groups {
            if self.parents[&group].is_some_and(|parent| included.contains(&parent)) {
                included.insert(group);
                continue;
            }
            let count = self.counts(group).1;
            if count > 0 && selected_counts.get(&group) == Some(&count) {
                included.insert(group);
                members.push(Member::Group(group));
            }
        }
        members.extend(
            selected
                .into_iter()
                .filter(|id| {
                    let mut cursor = self.objects.get(id).copied().flatten();
                    while let Some(group) = cursor {
                        if included.contains(&group) {
                            return false;
                        }
                        cursor = self.parents[&group];
                    }
                    true
                })
                .map(Member::Object),
        );
        members
    }

    fn set_parent(&mut self, member: Member, parent: Option<u32>) -> Result<(), EditError> {
        match member {
            Member::Group(id) => {
                *self
                    .parents
                    .get_mut(&id)
                    .ok_or_else(|| invalid(format!("Group {id} does not exist")))? = parent
            }
            Member::Object(id) => {
                *self.objects.get_mut(&id).ok_or(EditError::NotFound(id))? = parent
            }
        }
        Ok(())
    }
}

fn write_parent(editor: &mut Option<Properties>, parent: Option<u32>) {
    if let Some(parent) = parent {
        editor
            .get_or_insert_with(Properties::default)
            .set(PARENT_KEY, parent.to_string());
    } else if let Some(editor) = editor {
        editor.remove(PARENT_KEY);
    }
}

pub fn apply(tx: &mut Transaction, edit: &Edit) -> Result<u32, EditError> {
    let original = Graph::read(tx.document())?;
    if !edit.ungroup
        && (edit.name.chars().count() > MAX_NAME_CHARS || edit.name.chars().any(char::is_control))
    {
        return Err(invalid(format!(
            "Group names allow at most {MAX_NAME_CHARS} characters and no control characters"
        )));
    }
    if !edit.ungroup && edit.members.len() > MAX_MEMBERS {
        return Err(invalid(format!(
            "Group edits allow at most {MAX_MEMBERS} direct members"
        )));
    }
    if edit.ungroup && edit.group.is_none() {
        return Err(invalid("Choose a group to ungroup"));
    }
    let mut graph = original.clone();
    let id = if let Some(id) = edit.group {
        original.require(id)?;
        id
    } else {
        if graph.parents.len() == MAX_GROUPS {
            return Err(invalid(format!(
                "Grouping supports at most {MAX_GROUPS} group records"
            )));
        }
        if edit.members.is_empty() {
            return Err(invalid("Add at least one member to the new group"));
        }
        let id = tx.new_object_id()?;
        graph.parents.insert(id, None);
        id
    };
    let old_parent = original.parents.get(&id).copied().flatten();
    if edit.ungroup {
        if !owned_brush_links(tx.document(), id).is_empty() {
            return Err(invalid(
                "This group is referenced by brushes inside an entity; child-brush membership editing is not supported",
            ));
        }
        for member in original.direct_members(id)? {
            graph.set_parent(member, old_parent)?;
        }
        graph.parents.remove(&id);
    } else {
        if let Some(parent) = edit.parent {
            graph.require(parent)?;
        }
        let wanted: BTreeSet<_> = edit.members.iter().copied().collect();
        if wanted.len() != edit.members.len() {
            return Err(invalid("Group members must be unique"));
        }
        for member in &wanted {
            let mut parent = match member {
                Member::Object(id) => original.objects.get(id).copied().flatten(),
                Member::Group(id) => original.parents.get(id).copied().flatten(),
            };
            while let Some(group) = parent {
                if wanted.contains(&Member::Group(group)) {
                    return Err(invalid(
                        "Choose a nested group or its members, rather than both",
                    ));
                }
                parent = original.parents[&group];
            }
        }
        if edit.group.is_some() {
            for member in original.direct_members(id)? {
                if !wanted.contains(&member) {
                    graph.set_parent(member, old_parent)?;
                }
            }
        }
        graph.parents.insert(id, edit.parent);
        for &member in &edit.members {
            graph.set_parent(member, Some(id))?;
        }
    }
    graph.validate()?;
    for (&object, &parent) in &graph.objects {
        if original.objects.get(&object) == Some(&parent) {
            continue;
        }
        let editor = match object {
            ObjectId::Solid(id) => &mut tx.solid_mut(id)?.editor,
            ObjectId::Entity(id) => &mut tx.entity_mut(id)?.editor,
        };
        write_parent(editor, parent);
    }
    if edit.ungroup {
        tx.groups_mut().retain(|group| group.id != id);
    } else if edit.group.is_none() {
        tx.groups_mut().push(Group {
            id,
            editor: Some(Properties::default()),
            extra_properties: Properties::default(),
            extra: Vec::new(),
        });
    }
    for group in tx.groups_mut() {
        let parent = graph.parents[&group.id];
        if original.parents.get(&group.id) != Some(&parent) {
            write_parent(&mut group.editor, parent);
        }
        if group.id == id && !edit.ungroup {
            let current = name(group);
            if current != edit.name.trim() {
                group
                    .editor
                    .get_or_insert_with(Properties::default)
                    .set(NAME_KEY, edit.name.trim());
            }
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::{keyvalues::Node, vmf::Entity};

    fn fixture() -> Document {
        let mut document = Document::empty();
        document
            .edit("objects", |tx| {
                for _ in 0..3 {
                    let id = tx.new_object_id()?;
                    tx.add_entity(Entity::new(id, "info_target"))?;
                }
                Ok(())
            })
            .unwrap();
        document
    }

    fn create(document: &mut Document, members: Vec<Member>) -> u32 {
        document
            .edit("group", |tx| {
                apply(
                    tx,
                    &Edit {
                        group: None,
                        name: "Set".into(),
                        parent: None,
                        members,
                        ungroup: false,
                    },
                )
            })
            .unwrap()
    }

    #[test]
    fn nested_groups_ungroup_promotes_children_and_history_preserves_unknown_data() {
        let mut document = fixture();
        let original = document.vmf().clone();
        let inner = create(
            &mut document,
            vec![
                Member::Object(ObjectId::Entity(2)),
                Member::Object(ObjectId::Entity(3)),
            ],
        );
        document
            .edit("metadata", |tx| {
                let group = &mut tx.groups_mut()[0];
                group.extra_properties.set("custom", "keep");
                group.extra.push(Node::block(
                    "unknown",
                    vec![Node::string("nested", "value")],
                ));
                tx.entity_mut(2)?
                    .editor
                    .as_mut()
                    .unwrap()
                    .set("color", "12 34 56");
                Ok(())
            })
            .unwrap();
        let outer = create(
            &mut document,
            vec![Member::Group(inner), Member::Object(ObjectId::Entity(4))],
        );
        let graph = Graph::read(&document).unwrap();
        assert_eq!(graph.root(inner).unwrap(), outer);
        assert_eq!(graph.descendants(outer).unwrap().len(), 3);
        assert_eq!(
            graph.selection_members(&graph.descendants(outer).unwrap()),
            vec![Member::Group(outer)]
        );
        let before = document.vmf().clone();
        document
            .edit("ungroup", |tx| {
                apply(
                    tx,
                    &Edit {
                        group: Some(inner),
                        name: "".into(),
                        parent: None,
                        members: vec![],
                        ungroup: true,
                    },
                )
            })
            .unwrap();
        let graph = Graph::read(&document).unwrap();
        assert_eq!(graph.objects[&ObjectId::Entity(2)], Some(outer));
        assert!(!graph.parents.contains_key(&inner));
        assert_eq!(
            document
                .entity(2)
                .unwrap()
                .editor
                .as_ref()
                .unwrap()
                .get("color"),
            Some("12 34 56")
        );
        let after = document.vmf().clone();
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(document.vmf(), &after);
        for _ in 0..4 {
            document.undo().unwrap();
        }
        assert_eq!(document.vmf(), &original);
    }

    #[test]
    fn cycle_missing_duplicate_and_failed_transaction_are_atomic() {
        let mut document = fixture();
        let inner = create(&mut document, vec![Member::Object(ObjectId::Entity(2))]);
        let outer = create(&mut document, vec![Member::Group(inner)]);
        let before = document.vmf().clone();
        let revision = document.revision();
        for edit in [
            Edit {
                group: Some(outer),
                name: "bad".into(),
                parent: Some(inner),
                members: vec![Member::Group(inner)],
                ungroup: false,
            },
            Edit {
                group: None,
                name: "bad".into(),
                parent: None,
                members: vec![Member::Group(999)],
                ungroup: false,
            },
            Edit {
                group: Some(inner),
                name: "bad".into(),
                parent: None,
                members: vec![Member::Object(ObjectId::Entity(2)); 2],
                ungroup: false,
            },
        ] {
            assert!(document.edit("invalid", |tx| apply(tx, &edit)).is_err());
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.revision(), revision);
        }
        let edit = Edit {
            group: Some(inner),
            name: "rename".into(),
            parent: None,
            members: vec![Member::Object(ObjectId::Entity(3))],
            ungroup: false,
        };
        let result: Result<(), EditError> = document.edit("abort", |tx| {
            apply(tx, &edit)?;
            Err(invalid("abort"))
        });
        assert!(result.is_err());
        assert_eq!(document.vmf(), &before);
        let id = create(&mut document, vec![Member::Object(ObjectId::Entity(4))]);
        assert_eq!(id, outer + 1);
    }

    #[test]
    fn membership_edits_preserve_entity_child_groups_and_noops_keep_history() {
        let mut document = fixture();
        let inner = create(&mut document, vec![Member::Object(ObjectId::Entity(2))]);
        let outer = create(
            &mut document,
            vec![Member::Group(inner), Member::Object(ObjectId::Entity(3))],
        );
        let edit = Edit {
            group: Some(inner),
            name: "Set".into(),
            parent: Some(outer),
            members: vec![Member::Object(ObjectId::Entity(3))],
            ungroup: false,
        };
        document.edit("membership", |tx| apply(tx, &edit)).unwrap();
        let graph = Graph::read(&document).unwrap();
        assert_eq!(graph.objects[&ObjectId::Entity(2)], Some(outer));
        assert_eq!(graph.objects[&ObjectId::Entity(3)], Some(inner));
        let revision = document.revision();
        document.edit("noop", |tx| apply(tx, &edit)).unwrap();
        assert_eq!(document.revision(), revision);
        let text = document.vmf().write(&Default::default());
        let reopened = Document::try_new(source_formats::vmf::Vmf::parse(&text).unwrap()).unwrap();
        assert_eq!(Graph::read(&reopened).unwrap().parents, graph.parents);
        assert_eq!(Graph::read(&reopened).unwrap().objects, graph.objects);
    }

    #[test]
    fn owned_brush_links_are_preserved_and_block_ungrouping() {
        let mut document = fixture();
        let group = create(&mut document, vec![Member::Object(ObjectId::Entity(2))]);
        let solid = document
            .edit("owned brush", |tx| {
                let mut solid = crate::edit::block_solid(
                    tx,
                    glam::DVec3::ZERO,
                    glam::DVec3::splat(16.0),
                    "brick/test",
                )?;
                let id = solid.id;
                let mut editor = Properties::default();
                editor.set(PARENT_KEY, group.to_string());
                editor.set("custom", "preserved");
                solid.editor = Some(editor);
                tx.entity_mut(3)?.solids.push(solid);
                Ok(id)
            })
            .unwrap();
        let before = document.vmf().clone();
        let revision = document.revision();
        let ungroup = Edit {
            group: Some(group),
            name: String::new(),
            parent: None,
            members: Vec::new(),
            ungroup: true,
        };
        assert!(document.edit("ungroup", |tx| apply(tx, &ungroup)).is_err());
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
        assert_eq!(owned_brush_links(&document, group), vec![solid]);
        let rename = Edit {
            group: Some(group),
            name: "Renamed".into(),
            parent: None,
            members: vec![Member::Object(ObjectId::Entity(2))],
            ungroup: false,
        };
        document.edit("rename", |tx| apply(tx, &rename)).unwrap();
        assert_eq!(document.entity(3).unwrap(), &before.entities[1]);
        let text = document.vmf().write(&Default::default());
        let reopened = Document::try_new(source_formats::vmf::Vmf::parse(&text).unwrap()).unwrap();
        assert_eq!(owned_brush_links(&reopened, group), vec![solid]);
        let child = Edit {
            members: vec![Member::Object(ObjectId::Solid(solid))],
            ..rename
        };
        assert!(
            document
                .edit("child membership", |tx| apply(tx, &child))
                .is_err()
        );
    }
}
