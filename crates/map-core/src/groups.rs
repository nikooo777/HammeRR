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

pub(crate) fn parent(editor: Option<&Properties>) -> Result<Option<u32>, EditError> {
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
    if !edit.ungroup {
        source_formats::keyvalues::validate_string(&edit.name)
            .map_err(|error| invalid(error.to_string()))?;
    }
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
