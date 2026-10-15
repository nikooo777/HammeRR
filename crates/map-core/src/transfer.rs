use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{
    fgd::Catalog,
    keyvalues::{self, Node},
    vmf::{Entity, Properties, Solid, Vmf},
};

use crate::{
    Document, EditError, ObjectId, Transaction,
    context::{Metadata, Scope},
    edit, groups,
    metadata::InputBudget,
    references, repeat,
};

pub const MAX_BYTES: usize = crate::metadata::MAX_BYTES;
pub const MAX_DEPTH: usize = crate::metadata::MAX_DEPTH;
pub const MAX_NODES: usize = crate::metadata::MAX_NODES;
pub const MAX_GROUPS: usize = 256;
const KEY: &str = "hammerr_transfer";

#[derive(Clone, Debug)]
pub struct Payload {
    document: Document,
    ids: Vec<ObjectId>,
    text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub offset: DVec3,
    pub texture_lock: bool,
    pub keep_external: bool,
    pub restore_names: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            offset: DVec3::ZERO,
            texture_lock: true,
            keep_external: false,
            restore_names: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct External {
    pub source: u32,
    pub target: String,
    pub location: String,
    pub matches: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Result {
    pub visgroups: Vec<(u32, u32)>,
    pub objects: Vec<ObjectId>,
    pub mapping: Vec<(ObjectId, ObjectId)>,
    pub sides: Vec<(u32, u32)>,
    pub groups: Vec<(u32, u32)>,
    pub names: Vec<repeat::Name>,
    pub remapped_references: usize,
    pub external: Vec<External>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

impl InputBudget {
    fn solid(&mut self, solid: &Solid) -> std::result::Result<(), EditError> {
        self.editor(solid.editor.as_ref())?;
        if solid.sides.len() > 256 {
            return Err(invalid("Transfer supports at most 256 sides per brush"));
        }
        self.properties(&solid.extra_properties)?;
        self.nodes(&solid.extra)?;
        for side in &solid.sides {
            self.text("material", &side.material)?;
            self.properties(&side.extra_properties)?;
            self.nodes(&side.extra)?;
            if let Some(displacement) = &side.displacement {
                if !(2..=4).contains(&displacement.power) {
                    return Err(invalid("Transferred terrain must use power 2, 3 or 4"));
                }
                self.properties(&displacement.extra_properties)?;
                self.nodes(&displacement.extra)?;
            }
        }
        Ok(())
    }
}

fn add_ancestors(
    graph: &groups::Graph,
    mut group: Option<u32>,
    included: &mut BTreeSet<u32>,
) -> std::result::Result<(), EditError> {
    let mut depth = 0;
    while let Some(id) = group {
        depth += 1;
        if depth > MAX_DEPTH {
            return Err(invalid("Copied groups exceed 64 levels"));
        }
        included.insert(id);
        if included.len() > MAX_GROUPS {
            return Err(invalid("Transfer supports at most 256 groups"));
        }
        group = *graph
            .parents
            .get(&id)
            .ok_or_else(|| invalid(format!("Missing copied group {id}")))?;
    }
    Ok(())
}

impl Payload {
    pub fn capture(
        source: &Document,
        ids: &[ObjectId],
        catalog: &Catalog,
    ) -> std::result::Result<Self, EditError> {
        repeat::validate(
            source,
            ids,
            repeat::Options {
                count: 1,
                translation: DVec3::X,
                rotation: DVec3::ZERO,
                pivot: DVec3::ZERO,
                texture_lock: true,
            },
            catalog,
        )?;
        let graph = groups::Graph::read(source)?;
        let layer_graph = crate::layers::Graph::read(source)?;
        let mut budget = InputBudget::default();
        let mut included = BTreeSet::new();
        let mut objects = BTreeSet::new();
        for &id in ids {
            objects.insert(id);
            let editor = match id {
                ObjectId::Solid(id) => {
                    let solid = source
                        .solid(id)
                        .ok_or(EditError::NotFound(ObjectId::Solid(id)))?;
                    budget.solid(solid)?;
                    solid.editor.as_ref()
                }
                ObjectId::Entity(id) => {
                    let entity = source
                        .entity(id)
                        .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
                    budget.editor(entity.editor.as_ref())?;
                    budget.properties(&entity.properties)?;
                    budget.nodes(&entity.extra)?;
                    for connection in entity.connections.iter().flatten() {
                        budget.text(&connection.output, &connection.value)?;
                    }
                    for solid in &entity.solids {
                        budget.solid(solid)?;
                        objects.insert(ObjectId::Solid(solid.id));
                        add_ancestors(
                            &graph,
                            groups::parent(solid.editor.as_ref())?,
                            &mut included,
                        )?;
                    }
                    entity.editor.as_ref()
                }
            };
            add_ancestors(&graph, groups::parent(editor)?, &mut included)?;
        }
        let partitions =
            crate::partition_metadata::Metadata::parse(source.vmf()).map_err(invalid)?;
        if partitions
            .entries
            .keys()
            .any(|face| objects.contains(&ObjectId::Solid(face.0)))
        {
            return Err(invalid(
                "Resolve or remove saved partition intents on copied brushes before transferring them",
            ));
        }
        let names = repeat::source_names(source, ids)?;
        let entities = ids
            .iter()
            .filter_map(|id| {
                if let ObjectId::Entity(id) = id {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        repeat::validate_opaque_links(source, &entities, &names)?;
        for reference in references::scan(source, catalog)
            .into_iter()
            .filter(|reference| entities.contains(&reference.source))
        {
            if names
                .keys()
                .any(|name| crate::connections::name_matches(name, &reference.target))
                && (!reference.certain
                    || !names.contains_key(&reference.target.to_ascii_lowercase()))
            {
                return Err(invalid(format!(
                    "Entity {} has an uncertain internal reference to {}",
                    reference.source, reference.target
                )));
            }
        }
        let layer_ids = objects
            .iter()
            .flat_map(|id| layer_graph.memberships[id].layers.iter().copied())
            .collect::<BTreeSet<_>>();
        let layer_ids = layer_graph.ancestors(&layer_ids);
        if layer_ids.len() > MAX_GROUPS {
            return Err(invalid("Transfer supports at most 256 native layers"));
        }
        let mut native = source.vmf().visgroups.iter().flatten().collect::<Vec<_>>();
        while let Some(layer) = native.pop() {
            if layer_ids.contains(&layer.id) {
                budget.text("layer name", &layer.name)?;
                budget.properties(&layer.extra_properties)?;
                budget.nodes(&layer.extra)?;
            }
            native.extend(&layer.children);
        }
        let mut vmf = Document::empty().vmf().clone();
        vmf.visgroups = Some(crate::layers::subset(
            source.vmf().visgroups.as_deref().unwrap_or_default(),
            &layer_ids,
        ));
        vmf.world.id = source.vmf().world.id;
        vmf.world.solids = source
            .vmf()
            .world
            .solids
            .iter()
            .filter(|solid| objects.contains(&ObjectId::Solid(solid.id)))
            .cloned()
            .collect();
        vmf.entities = source
            .vmf()
            .entities
            .iter()
            .filter(|entity| entities.contains(&entity.id))
            .cloned()
            .collect();
        for group in source
            .vmf()
            .world
            .groups
            .iter()
            .filter(|group| included.contains(&group.id))
        {
            budget.editor(group.editor.as_ref())?;
            budget.properties(&group.extra_properties)?;
            budget.nodes(&group.extra)?;
            vmf.world.groups.push(group.clone());
        }
        let mut metadata = Metadata::default();
        for (scope, note) in &source.context()?.notes {
            if let Scope::Object(id) = scope
                && objects.contains(id)
            {
                budget.nodes(&note.extra)?;
                metadata.notes.insert(*scope, note.clone());
            }
        }
        if !metadata.notes.is_empty() {
            vmf.extra.push(metadata.to_node());
        }
        vmf.extra
            .push(Node::block(KEY, vec![Node::string("version", "1")]));
        let document = Document::try_new(vmf)?;
        let text = document.vmf().write(&keyvalues::WriteOptions::default());
        if text.len() > MAX_BYTES {
            return Err(invalid("Clipboard payload exceeds 8 MiB"));
        }
        keyvalues::parse_bounded(&text, MAX_DEPTH, MAX_NODES)
            .map_err(|error| invalid(error.to_string()))?;
        let ids = document.objects().collect();
        let payload = Self {
            document,
            ids,
            text,
        };
        let mut probe = Document::empty();
        probe.edit("validate clipboard", |tx| {
            paste(
                tx,
                &payload,
                Options {
                    keep_external: true,
                    ..Default::default()
                },
                catalog,
            )
        })?;
        Ok(payload)
    }

    pub fn decode(text: &str, catalog: &Catalog) -> std::result::Result<Self, EditError> {
        if text.len() > MAX_BYTES {
            return Err(invalid("Clipboard payload exceeds 8 MiB"));
        }
        let nodes = keyvalues::parse_bounded(text, MAX_DEPTH, MAX_NODES)
            .map_err(|error| invalid(error.to_string()))?;
        let markers = nodes
            .iter()
            .filter(|node| node.key == KEY)
            .collect::<Vec<_>>();
        if !markers.is_empty()
            && (markers.len() != 1
                || markers[0].children() != Some(&[Node::string("version", "1")][..]))
        {
            return Err(invalid(
                "Unsupported or malformed clipboard payload version",
            ));
        }
        let vmf = Vmf::from_nodes(&nodes).map_err(|error| invalid(error.to_string()))?;
        let source = Document::try_new(vmf)?;
        let ids = source
            .vmf()
            .world
            .solids
            .iter()
            .map(|solid| ObjectId::Solid(solid.id))
            .chain(
                source
                    .vmf()
                    .entities
                    .iter()
                    .map(|entity| ObjectId::Entity(entity.id)),
            )
            .collect::<Vec<_>>();
        Self::capture(&source, &ids, catalog)
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn document(&self) -> &Document {
        &self.document
    }
    pub fn ids(&self) -> &[ObjectId] {
        &self.ids
    }

    fn references(
        &self,
        catalog: &Catalog,
        destination: Option<&Document>,
    ) -> std::result::Result<(Vec<references::Reference>, Vec<External>), EditError> {
        let names = repeat::source_names(&self.document, &self.ids)?;
        let mut internal = Vec::new();
        let mut external = Vec::new();
        for reference in references::scan(&self.document, catalog) {
            if names
                .keys()
                .any(|name| crate::connections::name_matches(name, &reference.target))
            {
                if !reference.certain || !names.contains_key(&reference.target.to_ascii_lowercase())
                {
                    return Err(invalid(format!(
                        "Entity {} has an uncertain or wildcard internal reference to {}",
                        reference.source, reference.target
                    )));
                }
                internal.push(reference);
            } else if reference.declared && !reference.target.starts_with('!') {
                if !reference.certain {
                    return Err(invalid(format!(
                        "Entity {} has an uncertain declared reference to {}",
                        reference.source, reference.target
                    )));
                }
                let matches = destination
                    .into_iter()
                    .flat_map(|document| &document.vmf().entities)
                    .filter(|entity| {
                        entity.properties.get("targetname").is_some_and(|name| {
                            crate::connections::name_matches(name, &reference.target)
                        })
                    })
                    .map(|entity| entity.id)
                    .collect();
                external.push(External {
                    source: reference.source,
                    target: reference.target,
                    location: format!("{:?}", reference.location),
                    matches,
                });
            }
        }
        Ok((internal, external))
    }

    pub fn external(
        &self,
        catalog: &Catalog,
        destination: &Document,
    ) -> std::result::Result<Vec<External>, EditError> {
        self.references(catalog, Some(destination))
            .map(|(_, external)| external)
    }
}

fn fresh_object(
    tx: &mut Transaction,
    excluded: &BTreeSet<u32>,
) -> std::result::Result<u32, EditError> {
    loop {
        let id = tx.new_object_id()?;
        if !excluded.contains(&id) {
            return Ok(id);
        }
    }
}

fn fresh_side(
    tx: &mut Transaction,
    excluded: &BTreeSet<u32>,
) -> std::result::Result<u32, EditError> {
    loop {
        let id = tx.new_side_id()?;
        if !excluded.contains(&id) {
            return Ok(id);
        }
    }
}

fn remap_group(
    editor: &mut Option<Properties>,
    mapping: &BTreeMap<u32, u32>,
    visgroups: &BTreeMap<u32, u32>,
) -> std::result::Result<(), EditError> {
    if let Some(id) = groups::parent(editor.as_ref())? {
        editor
            .as_mut()
            .unwrap()
            .set("groupid", mapping[&id].to_string());
    }
    let ids = crate::layers::memberships(editor.as_ref())?;
    if !ids.is_empty() {
        let editor = editor.as_mut().unwrap();
        editor
            .0
            .retain(|(key, _)| !key.eq_ignore_ascii_case("visgroupid"));
        for id in ids {
            editor
                .0
                .push(("visgroupid".into(), visgroups[&id].to_string()));
        }
    }
    Ok(())
}

fn clone_solid(
    tx: &mut Transaction,
    solid: &mut Solid,
    result: &mut Result,
    excluded_objects: &BTreeSet<u32>,
    excluded_sides: &BTreeSet<u32>,
) -> std::result::Result<(), EditError> {
    let old = solid.id;
    solid.id = fresh_object(tx, excluded_objects)?;
    result
        .mapping
        .push((ObjectId::Solid(old), ObjectId::Solid(solid.id)));
    for side in &mut solid.sides {
        let old = side.id;
        side.id = fresh_side(tx, excluded_sides)?;
        result.sides.push((old, side.id));
    }
    Ok(())
}

pub fn paste(
    tx: &mut Transaction,
    payload: &Payload,
    options: Options,
    catalog: &Catalog,
) -> std::result::Result<Result, EditError> {
    if !options.offset.is_finite()
        || options.offset.abs().max_element() > crate::primitive::MAX_COORDINATE
    {
        return Err(invalid("Paste offset must be finite and within ±65536"));
    }
    tx.document()
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    groups::Graph::read(tx.document())?;
    crate::layers::Graph::read(tx.document())?;
    let (internal, external) = payload.references(catalog, Some(tx.document()))?;
    for reference in &internal {
        if tx.document().vmf().entities.iter().any(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case(&reference.target))
        }) {
            return Err(invalid(format!(
                "Internal target {} also matches a destination classname; resolve the ambiguity before pasting",
                reference.target
            )));
        }
    }
    if !options.keep_external && !external.is_empty() {
        return Err(invalid(format!(
            "{} external entity links need explicit Keep external links by name",
            external.len()
        )));
    }
    let source = payload.document();
    let names = repeat::source_names(source, payload.ids())?;
    let entity_ids = payload
        .ids
        .iter()
        .filter_map(|id| {
            if let ObjectId::Entity(id) = id {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    repeat::validate_opaque_links(source, &entity_ids, &names)?;
    let mut scan = references::scan(tx.document(), catalog);
    scan.extend(references::scan(source, catalog));
    let renamed = if options.restore_names {
        let mut restored = BTreeMap::new();
        for (key, (_, name)) in &names {
            if tx.document().vmf().entities.iter().any(|entity| {
                entity
                    .properties
                    .get("targetname")
                    .is_some_and(|existing| existing.eq_ignore_ascii_case(name))
            }) {
                return Err(invalid(format!("Cut name {name} is already occupied")));
            }
            restored.insert(key.clone(), name.clone());
        }
        restored
    } else {
        repeat::planned_names(tx.document(), &names, &scan, 1)?
            .pop()
            .unwrap()
    };
    let excluded_objects = source.vmf().object_ids().collect::<BTreeSet<_>>();
    let excluded_sides = source.vmf().side_ids().collect::<BTreeSet<_>>();
    let mut result = Result {
        visgroups: Vec::new(),
        objects: Vec::new(),
        mapping: Vec::new(),
        sides: Vec::new(),
        groups: Vec::new(),
        names: Vec::new(),
        remapped_references: internal.len(),
        external,
    };
    let source_layers = crate::layers::Graph::read(source)?;
    let mut layer_mapping = BTreeMap::new();
    for id in source_layers.records.keys() {
        let mut fresh = tx.new_visgroup_id()?;
        while source_layers.records.contains_key(&fresh) {
            fresh = tx.new_visgroup_id()?;
        }
        layer_mapping.insert(*id, fresh);
        result.visgroups.push((*id, fresh));
    }
    if !layer_mapping.is_empty() {
        let mut imported = source.vmf().visgroups.clone().unwrap_or_default();
        crate::layers::remap(&mut imported, &layer_mapping);
        let mut native = tx.document().vmf().visgroups.clone().unwrap_or_default();
        native.extend(imported);
        tx.set_visgroups(Some(native));
    }
    let mut group_mapping = BTreeMap::new();
    for group in &source.vmf().world.groups {
        group_mapping.insert(group.id, fresh_object(tx, &excluded_objects)?);
    }
    for group in &source.vmf().world.groups {
        let mut copy = group.clone();
        copy.id = group_mapping[&group.id];
        remap_group(&mut copy.editor, &group_mapping, &layer_mapping)?;
        result.groups.push((group.id, copy.id));
        tx.groups_mut().push(copy);
    }
    for &id in payload.ids() {
        let copy = match id {
            ObjectId::Solid(id) => {
                let mut solid = source
                    .solid(id)
                    .ok_or(EditError::NotFound(ObjectId::Solid(id)))?
                    .clone();
                clone_solid(
                    tx,
                    &mut solid,
                    &mut result,
                    &excluded_objects,
                    &excluded_sides,
                )?;
                remap_group(&mut solid.editor, &group_mapping, &layer_mapping)?;
                edit::translate_solid(&mut solid, options.offset, options.texture_lock);
                tx.add_solid(solid)?
            }
            ObjectId::Entity(id) => {
                let mut entity: Entity = source
                    .entity(id)
                    .ok_or(EditError::NotFound(ObjectId::Entity(id)))?
                    .clone();
                entity.id = fresh_object(tx, &excluded_objects)?;
                result
                    .mapping
                    .push((ObjectId::Entity(id), ObjectId::Entity(entity.id)));
                remap_group(&mut entity.editor, &group_mapping, &layer_mapping)?;
                for solid in &mut entity.solids {
                    clone_solid(tx, solid, &mut result, &excluded_objects, &excluded_sides)?;
                    remap_group(&mut solid.editor, &group_mapping, &layer_mapping)?;
                }
                if let Some(before) = entity
                    .properties
                    .get("targetname")
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                {
                    let after = renamed[&before.to_ascii_lowercase()].clone();
                    entity.properties.set("targetname", &after);
                    result.names.push(repeat::Name {
                        source: id,
                        copy: entity.id,
                        before,
                        after,
                    });
                }
                for reference in internal
                    .iter()
                    .rev()
                    .filter(|reference| reference.source == id)
                {
                    let value = repeat::copied_reference(&mut entity, reference)?;
                    if value.get(reference.span.clone()) != Some(reference.target.as_str()) {
                        return Err(invalid("Copied reference span changed"));
                    }
                    value.replace_range(
                        reference.span.clone(),
                        &renamed[&reference.target.to_ascii_lowercase()],
                    );
                }
                edit::translate_entity(&mut entity, options.offset, options.texture_lock);
                tx.add_entity(entity)?
            }
        };
        let (mins, maxs) = crate::query::bounds_of(tx.document(), &[copy])
            .ok_or_else(|| invalid("Pasted object has no finite bounds"))?;
        if !mins.is_finite()
            || !maxs.is_finite()
            || mins.abs().max(maxs.abs()).max_element() > crate::primitive::MAX_COORDINATE
        {
            return Err(invalid("Pasted geometry exceeds ±65536"));
        }
        result.objects.push(copy);
    }
    let mut metadata = tx.document().context()?.clone();
    for (old, new) in &result.mapping {
        if let Some(note) = source.context()?.notes.get(&Scope::Object(*old))
            && metadata
                .notes
                .insert(Scope::Object(*new), note.clone())
                .is_some()
        {
            return Err(invalid("Copied context would replace an existing note"));
        }
    }
    tx.set_context(metadata)?;
    groups::Graph::read(tx.document())?;
    crate::layers::Graph::read(tx.document())?;
    tx.document()
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(result)
}

#[cfg(test)]
mod tests;
