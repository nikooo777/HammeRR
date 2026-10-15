use crate::{Document, EditError, ObjectId, Transaction};
use source_formats::vmf::{Color, Properties, VisGroup, Vmf};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_LAYERS: usize = 2048;
pub const MAX_DEPTH: usize = 64;
pub const MAX_LINKS: usize = 32768;
pub const MAX_EDIT_MEMBERS: usize = 8192;
pub const MAX_NAME_CHARS: usize = 128;

#[derive(Clone, Debug)]
pub struct Record {
    pub id: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub color: [u8; 3],
    pub depth: usize,
}
#[derive(Clone, Debug)]
pub struct Membership {
    pub owner: ObjectId,
    pub hidden: bool,
    pub layers: BTreeSet<u32>,
}
#[derive(Clone, Debug)]
pub struct Graph {
    pub records: BTreeMap<u32, Record>,
    pub memberships: BTreeMap<ObjectId, Membership>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub id: Option<u32>,
    pub name: String,
    pub color: [u8; 3],
    pub parent: Option<u32>,
    pub members: Vec<ObjectId>,
    pub delete: bool,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub(crate) fn max_id(vmf: &Vmf) -> u32 {
    let mut stack = vmf.visgroups.iter().flatten().collect::<Vec<_>>();
    let mut max = 0;
    while let Some(group) = stack.pop() {
        max = max.max(group.id);
        stack.extend(&group.children);
    }
    max
}

pub fn memberships(editor: Option<&Properties>) -> Result<BTreeSet<u32>, EditError> {
    editor
        .into_iter()
        .flat_map(|editor| editor.get_all("visgroupid"))
        .map(|value| {
            value.parse::<u32>().map_err(|_| {
                invalid("Invalid editor.visgroupid; repair the imported layer membership first")
            })
        })
        .collect()
}

impl Graph {
    pub fn read(document: &Document) -> Result<Self, EditError> {
        document
            .vmf()
            .validate_ids()
            .map_err(|error| invalid(error.to_string()))?;
        let mut budget = crate::metadata::InputBudget::default();
        let mut records = BTreeMap::new();
        let mut stack = document
            .vmf()
            .visgroups
            .iter()
            .flatten()
            .map(|group| (group, None, 0))
            .collect::<Vec<_>>();
        while let Some((group, parent, depth)) = stack.pop() {
            if records.len() >= MAX_LAYERS || depth >= MAX_DEPTH {
                return Err(invalid("Layers exceed 2048 records or 64 nesting levels"));
            }
            budget.text("name", &group.name)?;
            budget.properties(&group.extra_properties)?;
            budget.nodes(&group.extra)?;
            if records
                .insert(
                    group.id,
                    Record {
                        id: group.id,
                        parent,
                        name: group.name.clone(),
                        color: group.color.0,
                        depth,
                    },
                )
                .is_some()
            {
                return Err(invalid(format!("Duplicate native layer ID {}", group.id)));
            }
            if stack.len() + group.children.len() > MAX_LAYERS {
                return Err(invalid("Layers exceed 2048 records"));
            }
            stack.extend(
                group
                    .children
                    .iter()
                    .map(|child| (child, Some(group.id), depth + 1)),
            );
        }
        for group in &document.vmf().world.groups {
            if !memberships(group.editor.as_ref())?.is_empty() {
                return Err(invalid(
                    "Direct layer links on native group records are unsupported; assign their member objects instead",
                ));
            }
        }
        let mut graph = Self {
            records,
            memberships: BTreeMap::new(),
        };
        let mut links = 0;
        for solid in &document.vmf().world.solids {
            let id = ObjectId::Solid(solid.id);
            graph.add_membership(id, id, solid.hidden, solid.editor.as_ref(), &mut links)?;
        }
        for entity in &document.vmf().entities {
            let owner = ObjectId::Entity(entity.id);
            graph.add_membership(
                owner,
                owner,
                entity.hidden,
                entity.editor.as_ref(),
                &mut links,
            )?;
            for solid in &entity.solids {
                graph.add_membership(
                    ObjectId::Solid(solid.id),
                    owner,
                    entity.hidden || solid.hidden,
                    solid.editor.as_ref(),
                    &mut links,
                )?;
            }
        }
        Ok(graph)
    }
    fn add_membership(
        &mut self,
        id: ObjectId,
        owner: ObjectId,
        hidden: bool,
        editor: Option<&Properties>,
        links: &mut usize,
    ) -> Result<(), EditError> {
        let layers = memberships(editor)?;
        *links += layers.len();
        if *links > MAX_LINKS {
            return Err(invalid("Layer memberships exceed 32768 links"));
        }
        if let Some(missing) = layers.iter().find(|id| !self.records.contains_key(id)) {
            return Err(invalid(format!("{id} refers to missing layer {missing}")));
        }
        self.memberships.insert(
            id,
            Membership {
                owner,
                hidden,
                layers,
            },
        );
        Ok(())
    }
    pub fn within(&self, mut child: u32, parent: u32) -> bool {
        for _ in 0..MAX_DEPTH {
            if child == parent {
                return true;
            }
            let Some(next) = self.records.get(&child).and_then(|record| record.parent) else {
                return false;
            };
            child = next;
        }
        false
    }
    pub fn members(
        &self,
        layer: u32,
        recursive: bool,
        include_hidden: bool,
    ) -> Result<Vec<ObjectId>, EditError> {
        if !self.records.contains_key(&layer) {
            return Err(invalid("Layer no longer exists"));
        }
        Ok(self
            .memberships
            .values()
            .filter(|membership| include_hidden || !membership.hidden)
            .filter(|membership| {
                membership
                    .layers
                    .iter()
                    .any(|id| *id == layer || (recursive && self.within(*id, layer)))
            })
            .map(|membership| membership.owner)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    pub fn ancestors(&self, ids: &BTreeSet<u32>) -> BTreeSet<u32> {
        let mut result = BTreeSet::new();
        for id in ids {
            let mut current = Some(*id);
            while let Some(id) = current {
                result.insert(id);
                current = self.records[&id].parent;
            }
        }
        result
    }
}

pub(crate) fn set_memberships(
    tx: &mut Transaction,
    id: ObjectId,
    layers: &BTreeSet<u32>,
) -> Result<(), EditError> {
    let owner = match id {
        ObjectId::Entity(_) => id,
        ObjectId::Solid(id) => tx
            .document()
            .owner_of_solid(id)
            .ok_or(EditError::NotFound(ObjectId::Solid(id)))?,
    };
    let editor = match (id, owner) {
        (ObjectId::Entity(id), _) => &mut tx.entity_mut(id)?.editor,
        (ObjectId::Solid(id), ObjectId::Solid(_)) => &mut tx.solid_mut(id)?.editor,
        (ObjectId::Solid(id), ObjectId::Entity(owner)) => {
            &mut tx
                .entity_mut(owner)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == id)
                .ok_or(EditError::NotFound(ObjectId::Solid(id)))?
                .editor
        }
    };
    if editor.is_none() && layers.is_empty() {
        return Ok(());
    }
    let editor = editor.get_or_insert_with(Properties::default);
    editor
        .0
        .retain(|(key, _)| !key.eq_ignore_ascii_case("visgroupid"));
    editor.0.extend(
        layers
            .iter()
            .map(|id| ("visgroupid".into(), id.to_string())),
    );
    Ok(())
}

fn list_mut(
    groups: &mut Vec<VisGroup>,
    parent: Option<u32>,
) -> Result<&mut Vec<VisGroup>, EditError> {
    if let Some(parent) = parent {
        Ok(&mut find_mut(groups, parent)
            .ok_or_else(|| invalid("Parent layer no longer exists"))?
            .children)
    } else {
        Ok(groups)
    }
}
fn find_mut(groups: &mut [VisGroup], id: u32) -> Option<&mut VisGroup> {
    for group in groups {
        if group.id == id {
            return Some(group);
        }
        if let Some(found) = find_mut(&mut group.children, id) {
            return Some(found);
        }
    }
    None
}

pub fn apply(tx: &mut Transaction, edit: &Edit) -> Result<u32, EditError> {
    let graph = Graph::read(tx.document())?;
    if edit.members.len() > MAX_EDIT_MEMBERS
        || edit.members.iter().copied().collect::<BTreeSet<_>>().len() != edit.members.len()
    {
        return Err(invalid("Choose at most 8192 distinct layer members"));
    }
    if edit
        .members
        .iter()
        .any(|id| !graph.memberships.contains_key(id))
    {
        return Err(invalid("A layer member no longer exists"));
    }
    if edit
        .parent
        .is_some_and(|id| !graph.records.contains_key(&id))
    {
        return Err(invalid("Parent layer no longer exists"));
    }
    if let Some(id) = edit.id {
        if !graph.records.contains_key(&id) {
            return Err(invalid("Layer no longer exists"));
        }
        if edit.parent.is_some_and(|parent| graph.within(parent, id)) {
            return Err(invalid(
                "A layer cannot move into itself or its descendants",
            ));
        }
    } else if edit.delete {
        return Err(invalid("Choose an existing layer to delete"));
    }
    if !edit.delete
        && (edit.name.trim() != edit.name
            || edit.name.is_empty()
            || edit.name.chars().count() > MAX_NAME_CHARS
            || edit.name.chars().any(|c| c.is_control() || c == '"'))
    {
        return Err(invalid(
            "Layer names need 1–128 characters without quotes, controls or surrounding whitespace",
        ));
    }
    if edit.id.is_none() && graph.records.len() >= MAX_LAYERS {
        return Err(invalid("Map supports at most 2048 layers"));
    }
    let id = match edit.id {
        Some(id) => id,
        None => tx.new_visgroup_id()?,
    };
    let mut groups = tx.document().vmf().visgroups.clone().unwrap_or_default();
    if let Some(old) = edit.id {
        let parent = graph.records[&old].parent;
        if edit.delete || parent != edit.parent {
            let siblings = list_mut(&mut groups, parent)?;
            let index = siblings.iter().position(|group| group.id == old).unwrap();
            let mut group = siblings.remove(index);
            if edit.delete {
                siblings.splice(index..index, group.children);
            } else {
                group.name = edit.name.clone();
                group.color = Color(edit.color);
                list_mut(&mut groups, edit.parent)?.push(group);
            }
        } else {
            let group = find_mut(&mut groups, old).unwrap();
            group.name = edit.name.clone();
            group.color = Color(edit.color);
        }
    } else {
        list_mut(&mut groups, edit.parent)?.push(VisGroup {
            id,
            name: edit.name.clone(),
            color: Color(edit.color),
            children: Vec::new(),
            extra_properties: Properties::default(),
            extra: Vec::new(),
        });
    }
    tx.set_visgroups(Some(groups));
    let wanted = edit.members.iter().copied().collect::<BTreeSet<_>>();
    for (object, membership) in &graph.memberships {
        let mut layers = membership.layers.clone();
        if edit.delete || !wanted.contains(object) {
            layers.remove(&id);
        } else {
            layers.insert(id);
        }
        if layers != membership.layers {
            set_memberships(tx, *object, &layers)?;
        }
    }
    Graph::read(tx.document())?;
    Ok(id)
}

pub(crate) fn subset(groups: &[VisGroup], included: &BTreeSet<u32>) -> Vec<VisGroup> {
    groups
        .iter()
        .filter(|group| included.contains(&group.id))
        .map(|group| VisGroup {
            id: group.id,
            name: group.name.clone(),
            color: group.color,
            children: subset(&group.children, included),
            extra_properties: group.extra_properties.clone(),
            extra: group.extra.clone(),
        })
        .collect()
}

pub(crate) fn remap(groups: &mut [VisGroup], mapping: &BTreeMap<u32, u32>) {
    for group in groups {
        group.id = mapping[&group.id];
        remap(&mut group.children, mapping);
    }
}

#[cfg(test)]
mod tests;
