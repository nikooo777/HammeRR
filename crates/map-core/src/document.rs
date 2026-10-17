//! An editable map: object lookup, id allocation, and transactions with
//! undo/redo.
//!
//! Every change goes through [`Document::edit`]. A transaction snapshots each
//! object the first time it's touched; committing records before/after
//! states, so undo and redo restore whole objects at their original position
//! in the file.

use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;

use crate::context::{self, Metadata, Scope};
use source_formats::keyvalues::Node;

use source_formats::vmf::{Entity, Group, Properties, Solid, VisGroup, Vmf, World};

/// A selectable top-level object. Brushes inside a brush entity belong to
/// their entity and are addressed through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectId {
    Solid(u32),
    Entity(u32),
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Solid(id) => write!(f, "solid {id}"),
            Self::Entity(id) => write!(f, "entity {id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Solid(Solid),
    Entity(Entity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    NotFound(ObjectId),
    Invalid(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "{id} does not exist"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditError {}

/// An object's state and its index in its list (`world.solids` or
/// `entities`); `None` when it doesn't exist.
type State = Option<(usize, Object)>;
type VisGroups = Option<Vec<VisGroup>>;

#[derive(Debug, Clone)]
struct Change {
    id: ObjectId,
    before: State,
    after: State,
}

#[derive(Debug, Clone)]
pub struct Step {
    id: u64,
    pub label: String,
    changes: Vec<Change>,
    world: Option<(Properties, Properties)>,
    extra: Option<(Vec<Node>, Vec<Node>)>,
    cordon: Option<(Option<Node>, Option<Node>)>,
    groups: Option<(Vec<Group>, Vec<Group>)>,
    visgroups: Option<(VisGroups, VisGroups)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

impl Step {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn object_changes(&self) -> impl Iterator<Item = (ObjectId, ChangeKind)> + '_ {
        self.changes.iter().map(|change| {
            let kind = match (&change.before, &change.after) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Removed,
                _ => ChangeKind::Modified,
            };
            (change.id, kind)
        })
    }

    pub fn sections(&self) -> impl Iterator<Item = &'static str> {
        [
            (self.world.is_some(), "world properties"),
            (self.extra.is_some(), "map metadata"),
            (self.cordon.is_some(), "cordon"),
            (self.groups.is_some(), "groups"),
            (self.visgroups.is_some(), "visibility groups"),
        ]
        .into_iter()
        .filter_map(|(changed, name)| changed.then_some(name))
    }

    pub fn objects(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.changes.iter().map(|c| c.id)
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    vmf: Vmf,
    undo: Vec<Step>,
    redo: Vec<Step>,
    revision: u64,
    saved_revision: u64,
    next_object_id: u64,
    next_side_id: u64,
    next_visgroup_id: u64,
    context_cache: OnceLock<Result<Metadata, String>>,
    context_fingerprint: OnceLock<String>,
}

impl Document {
    pub fn try_new(vmf: Vmf) -> Result<Self, EditError> {
        vmf.validate_ids()
            .map_err(|error| EditError::Invalid(error.to_string()))?;
        Ok(Self::new(vmf))
    }

    pub fn new(vmf: Vmf) -> Self {
        let object_max = vmf.object_ids().max().unwrap_or(0);
        let side_max = vmf.side_ids().max().unwrap_or(0);
        let context = Metadata::parse(&vmf);
        Self {
            context_cache: OnceLock::from(context),
            context_fingerprint: OnceLock::new(),
            next_object_id: u64::from(object_max) + 1,
            next_side_id: u64::from(side_max) + 1,
            next_visgroup_id: u64::from(crate::layers::max_id(&vmf)) + 1,
            vmf,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
        }
    }

    /// An empty map, set up the way Hammer creates new CS:S maps.
    pub fn empty() -> Self {
        let props = |pairs: &[(&str, &str)]| {
            Properties(pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect())
        };
        let vmf = Vmf {
            version_info: Some(props(&[
                ("editorversion", "400"),
                ("editorbuild", "8872"),
                ("mapversion", "0"),
                ("formatversion", "100"),
                ("prefab", "0"),
            ])),
            visgroups: Some(Vec::new()),
            visgroups_extra: Vec::new(),
            view_settings: Some(props(&[
                ("bSnapToGrid", "1"),
                ("bShowGrid", "1"),
                ("bShowLogicalGrid", "0"),
                ("nGridSpacing", "16"),
                ("bShow3DGrid", "0"),
            ])),
            world: World {
                id: 1,
                properties: props(&[
                    ("mapversion", "0"),
                    ("classname", "worldspawn"),
                    ("detailmaterial", "detail/detailsprites"),
                    ("detailvbsp", "detail.vbsp"),
                    ("maxpropscreenwidth", "-1"),
                    ("skyname", "sky_day01_01"),
                ]),
                solids: Vec::new(),
                groups: Vec::new(),
                extra: Vec::new(),
            },
            entities: Vec::new(),
            cameras: None,
            cordon: None,
            extra: Vec::new(),
        };
        Self::new(vmf)
    }

    pub fn vmf(&self) -> &Vmf {
        &self.vmf
    }

    pub fn into_vmf(self) -> Vmf {
        self.vmf
    }

    /// An independent draft with the same identity/revision state and no edit history.
    pub fn preview_snapshot(&self) -> Self {
        Self {
            vmf: self.vmf.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: self.revision,
            saved_revision: self.saved_revision,
            next_object_id: self.next_object_id,
            next_side_id: self.next_side_id,
            next_visgroup_id: self.next_visgroup_id,
            context_cache: self.context_cache.clone(),
            context_fingerprint: self.context_fingerprint.clone(),
        }
    }

    pub fn context(&self) -> Result<&Metadata, EditError> {
        self.context_cache
            .get_or_init(|| Metadata::parse(&self.vmf))
            .as_ref()
            .map_err(|message| EditError::Invalid(message.clone()))
    }

    pub fn context_map_fingerprint(&self) -> &str {
        self.context_fingerprint
            .get_or_init(|| context::map_fingerprint(&self.vmf))
    }

    /// Increases with every change, including undo and redo.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    pub fn mark_unsaved(&mut self) {
        self.saved_revision = u64::MAX;
    }

    pub fn snapshot_for_save(&self) -> Vmf {
        let mut vmf = self.vmf.clone();
        Self::bump_vmf_version(&mut vmf);
        vmf
    }

    /// Bumps `mapversion` in both places Hammer stores it, as a save does.
    pub fn bump_map_version(&mut self) {
        Self::bump_vmf_version(&mut self.vmf);
    }

    fn bump_vmf_version(vmf: &mut Vmf) {
        let bump = |props: &mut Properties| {
            let next = props
                .get("mapversion")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_add(1);
            props.set("mapversion", next.to_string());
        };
        if let Some(info) = &mut vmf.version_info {
            bump(info);
        }
        bump(&mut vmf.world.properties);
    }

    pub fn solid(&self, id: u32) -> Option<&Solid> {
        self.vmf.world.solids.iter().find(|s| s.id == id)
    }

    pub fn entity(&self, id: u32) -> Option<&Entity> {
        self.vmf.entities.iter().find(|e| e.id == id)
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.position(id).is_some()
    }

    pub fn objects(&self) -> impl Iterator<Item = ObjectId> + '_ {
        let solids = self.vmf.world.solids.iter().map(|s| ObjectId::Solid(s.id));
        solids.chain(self.vmf.entities.iter().map(|e| ObjectId::Entity(e.id)))
    }

    /// The top-level object owning a brush, given the brush's solid id.
    pub fn owner_of_solid(&self, solid_id: u32) -> Option<ObjectId> {
        if self.solid(solid_id).is_some() {
            return Some(ObjectId::Solid(solid_id));
        }
        self.vmf
            .entities
            .iter()
            .find(|e| e.solids.iter().any(|s| s.id == solid_id))
            .map(|e| ObjectId::Entity(e.id))
    }

    pub fn undo_steps(&self) -> &[Step] {
        &self.undo
    }

    pub fn redo_steps(&self) -> &[Step] {
        &self.redo
    }

    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().rev().map(|s| s.label.as_str())
    }

    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().rev().map(|s| s.label.as_str())
    }

    /// Runs `f` as one undoable step. If it fails, every change it made is
    /// rolled back and nothing is recorded.
    pub fn edit<R>(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut Transaction) -> Result<R, EditError>,
    ) -> Result<R, EditError> {
        self.edit_with_finalizer(label, f, |_| Ok(()))
    }

    /// Runs `finalize` after successful edits and context cleanup, before history
    /// commit. Any error rolls back the document, including allocated IDs.
    /// The finalizer also runs for successful edits that leave no map changes.
    /// Callers needing an Undo step must ensure that edits change the map.
    /// Undo and redo never run the finalizer.
    pub fn edit_with_finalizer<R>(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut Transaction) -> Result<R, EditError>,
        finalize: impl FnOnce(&R) -> Result<(), EditError>,
    ) -> Result<R, EditError> {
        let label = label.into();
        let mut tx = Transaction {
            start_ids: (
                self.next_object_id,
                self.next_side_id,
                self.next_visgroup_id,
            ),
            doc: self,
            touched: Vec::new(),
            start_positions: None,
            reserved_measurement_ids: None,
            world: None,
            extra: None,
            cordon: None,
            groups: None,
            visgroups: None,
        };
        let result = f(&mut tx).and_then(|value| {
            tx.remove_deleted_context()?;
            finalize(&value)?;
            Ok(value)
        });
        match result {
            Ok(result) => {
                tx.commit(label);
                Ok(result)
            }
            Err(error) => {
                tx.rollback();
                Err(error)
            }
        }
    }

    /// Returns the undone step.
    pub fn undo(&mut self) -> Option<&Step> {
        let step = self.undo.pop()?;
        if let Some((before, _)) = &step.cordon {
            self.vmf.cordon = before.clone();
        }
        if let Some((before, _)) = &step.visgroups {
            self.vmf.visgroups = before.clone();
        }
        if let Some((before, _)) = &step.groups {
            self.vmf.world.groups = before.clone();
        }
        if let Some((before, _)) = &step.extra {
            self.vmf.extra = before.clone();
            self.context_cache.take();
        }
        self.context_fingerprint.take();
        if let Some((before, after)) = &step.world {
            self.restore_world_properties(before, after);
        }
        self.restore_objects(
            step.changes
                .iter()
                .map(|change| (change.id, change.before.clone())),
        );
        self.revision += 1;
        self.redo.push(step);
        self.redo.last()
    }

    /// Returns the redone step.
    pub fn redo(&mut self) -> Option<&Step> {
        let step = self.redo.pop()?;
        if let Some((_, after)) = &step.cordon {
            self.vmf.cordon = after.clone();
        }
        if let Some((_, after)) = &step.visgroups {
            self.vmf.visgroups = after.clone();
        }
        if let Some((_, after)) = &step.groups {
            self.vmf.world.groups = after.clone();
        }
        if let Some((_, after)) = &step.extra {
            self.vmf.extra = after.clone();
            self.context_cache.take();
        }
        self.context_fingerprint.take();
        if let Some((before, after)) = &step.world {
            self.restore_world_properties(after, before);
        }
        self.restore_objects(
            step.changes
                .iter()
                .map(|change| (change.id, change.after.clone())),
        );
        self.revision += 1;
        self.undo.push(step);
        self.undo.last()
    }

    fn restore_world_properties(&mut self, target: &Properties, previous: &Properties) {
        let mut properties = target.clone();
        // Saves increment mapversion outside the edit history.
        if target.get("mapversion") == previous.get("mapversion")
            && let Some(version) = self.vmf.world.properties.get("mapversion")
        {
            properties.set("mapversion", version);
        }
        self.vmf.world.properties = properties;
    }

    fn position(&self, id: ObjectId) -> Option<usize> {
        match id {
            ObjectId::Solid(id) => self.vmf.world.solids.iter().position(|s| s.id == id),
            ObjectId::Entity(id) => self.vmf.entities.iter().position(|e| e.id == id),
        }
    }

    fn state(&self, id: ObjectId) -> State {
        let index = self.position(id)?;
        Some(match id {
            ObjectId::Solid(_) => (index, Object::Solid(self.vmf.world.solids[index].clone())),
            ObjectId::Entity(_) => (index, Object::Entity(self.vmf.entities[index].clone())),
        })
    }

    fn take(&mut self, id: ObjectId) -> Option<Object> {
        let index = self.position(id)?;
        Some(match id {
            ObjectId::Solid(_) => Object::Solid(self.vmf.world.solids.remove(index)),
            ObjectId::Entity(_) => Object::Entity(self.vmf.entities.remove(index)),
        })
    }

    fn restore_objects(&mut self, states: impl IntoIterator<Item = (ObjectId, State)>) {
        let mut insertions = Vec::new();
        for (id, state) in states {
            self.take(id);
            if let Some(state) = state {
                insertions.push((id, state));
            }
        }
        insertions.sort_by_key(|(_, (index, _))| *index);
        for (id, state) in insertions {
            self.put(id, Some(state));
        }
    }

    fn put(&mut self, id: ObjectId, state: State) {
        self.take(id);
        match state {
            Some((index, Object::Solid(solid))) => {
                let list = &mut self.vmf.world.solids;
                list.insert(index.min(list.len()), solid);
            }
            Some((index, Object::Entity(entity))) => {
                let list = &mut self.vmf.entities;
                list.insert(index.min(list.len()), entity);
            }
            None => {}
        }
    }
}

/// Mutable access to a document during [`Document::edit`].
pub struct Transaction<'a> {
    start_ids: (u64, u64, u64),
    doc: &'a mut Document,
    touched: Vec<(ObjectId, State)>,
    start_positions: Option<std::collections::HashMap<ObjectId, usize>>,
    reserved_measurement_ids: Option<HashSet<u32>>,
    world: Option<Properties>,
    extra: Option<Vec<Node>>,
    cordon: Option<Option<Node>>,
    groups: Option<Vec<Group>>,
    visgroups: Option<VisGroups>,
}

impl Transaction<'_> {
    pub(crate) fn set_cordon(&mut self, node: Option<Node>) {
        self.cordon
            .get_or_insert_with(|| self.doc.vmf.cordon.clone());
        self.doc.vmf.cordon = node;
    }

    pub(crate) fn set_visgroups(&mut self, groups: VisGroups) {
        self.doc.context_fingerprint.take();
        self.visgroups
            .get_or_insert_with(|| self.doc.vmf.visgroups.clone());
        self.doc.vmf.visgroups = groups;
        self.doc.next_visgroup_id = self
            .doc
            .next_visgroup_id
            .max(u64::from(crate::layers::max_id(&self.doc.vmf)) + 1);
    }

    pub(crate) fn new_visgroup_id(&mut self) -> Result<u32, EditError> {
        let id = u32::try_from(self.doc.next_visgroup_id)
            .map_err(|_| EditError::Invalid("Native layer IDs are exhausted".into()))?;
        self.doc.next_visgroup_id += 1;
        Ok(id)
    }

    pub(crate) fn groups_mut(&mut self) -> &mut Vec<Group> {
        self.doc.context_fingerprint.take();
        self.groups
            .get_or_insert_with(|| self.doc.vmf.world.groups.clone());
        &mut self.doc.vmf.world.groups
    }

    pub fn world_properties_mut(&mut self) -> &mut Properties {
        self.doc.context_fingerprint.take();
        self.world
            .get_or_insert_with(|| self.doc.vmf.world.properties.clone());
        &mut self.doc.vmf.world.properties
    }

    pub fn vmf(&self) -> &Vmf {
        &self.doc.vmf
    }

    pub fn document(&self) -> &Document {
        self.doc
    }

    pub fn new_object_id(&mut self) -> Result<u32, EditError> {
        if self.reserved_measurement_ids.is_none() {
            self.reserved_measurement_ids =
                Some(crate::measurements::reserved_origin_ids(self.vmf()));
        }
        loop {
            let id = u32::try_from(self.doc.next_object_id)
                .map_err(|_| EditError::Invalid("Map object IDs are exhausted".into()))?;
            self.doc.next_object_id += 1;
            let notes = &self.doc.context()?.notes;
            let reserved = notes.contains_key(&Scope::Object(ObjectId::Solid(id)))
                || notes.contains_key(&Scope::Object(ObjectId::Entity(id)))
                || self
                    .reserved_measurement_ids
                    .as_ref()
                    .unwrap()
                    .contains(&id);
            if !reserved {
                return Ok(id);
            }
        }
    }

    pub fn new_side_id(&mut self) -> Result<u32, EditError> {
        let id = u32::try_from(self.doc.next_side_id)
            .map_err(|_| EditError::Invalid("Map face IDs are exhausted".into()))?;
        self.doc.next_side_id += 1;
        Ok(id)
    }

    fn capture_object_order(&mut self) {
        if self.start_positions.is_none() {
            self.start_positions = Some(
                self.doc
                    .vmf
                    .world
                    .solids
                    .iter()
                    .enumerate()
                    .map(|(index, solid)| (ObjectId::Solid(solid.id), index))
                    .chain(
                        self.doc
                            .vmf
                            .entities
                            .iter()
                            .enumerate()
                            .map(|(index, entity)| (ObjectId::Entity(entity.id), index)),
                    )
                    .collect(),
            );
        }
    }

    fn touch(&mut self, id: ObjectId) {
        self.doc.context_fingerprint.take();
        if !self.touched.iter().any(|(t, _)| *t == id) {
            let mut state = self.doc.state(id);
            if let (Some(positions), Some((index, _))) = (&self.start_positions, &mut state)
                && let Some(original) = positions.get(&id)
            {
                *index = *original;
            }
            self.touched.push((id, state));
        }
    }

    pub fn solid_mut(&mut self, id: u32) -> Result<&mut Solid, EditError> {
        self.touch(ObjectId::Solid(id));
        self.doc
            .vmf
            .world
            .solids
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(EditError::NotFound(ObjectId::Solid(id)))
    }

    pub fn entity_mut(&mut self, id: u32) -> Result<&mut Entity, EditError> {
        self.touch(ObjectId::Entity(id));
        self.doc
            .vmf
            .entities
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(EditError::NotFound(ObjectId::Entity(id)))
    }

    /// Appends a world brush. Its id must be unused.
    pub fn add_solid(&mut self, solid: Solid) -> Result<ObjectId, EditError> {
        let id = ObjectId::Solid(solid.id);
        self.reserve_inserted_ids(
            std::iter::once(solid.id),
            solid.sides.iter().map(|side| side.id),
        )?;
        self.capture_object_order();
        self.touch(id);
        self.doc.vmf.world.solids.push(solid);
        Ok(id)
    }

    /// Appends an entity. Its id must be unused.
    pub fn add_entity(&mut self, entity: Entity) -> Result<ObjectId, EditError> {
        let id = ObjectId::Entity(entity.id);
        self.reserve_inserted_ids(
            std::iter::once(entity.id).chain(entity.solids.iter().map(|solid| solid.id)),
            entity
                .solids
                .iter()
                .flat_map(|solid| solid.sides.iter().map(|side| side.id)),
        )?;
        self.capture_object_order();
        self.touch(id);
        self.doc.vmf.entities.push(entity);
        Ok(id)
    }

    fn reserve_inserted_ids(
        &mut self,
        objects: impl Iterator<Item = u32>,
        sides: impl Iterator<Item = u32>,
    ) -> Result<(), EditError> {
        let mut object_ids: HashSet<_> = self.doc.vmf.object_ids().collect();
        let mut side_ids: HashSet<_> = self.doc.vmf.side_ids().collect();
        let (mut next_object, mut next_side) = (self.doc.next_object_id, self.doc.next_side_id);
        for id in objects {
            if !object_ids.insert(id) {
                return Err(EditError::Invalid(format!("duplicate object ID {id}")));
            }
            next_object = next_object.max(u64::from(id) + 1);
        }
        for id in sides {
            if !side_ids.insert(id) {
                return Err(EditError::Invalid(format!("duplicate side ID {id}")));
            }
            next_side = next_side.max(u64::from(id) + 1);
        }
        self.doc.next_object_id = next_object;
        self.doc.next_side_id = next_side;
        Ok(())
    }

    pub fn remove(&mut self, id: ObjectId) -> Result<Object, EditError> {
        self.capture_object_order();
        self.touch(id);
        self.doc.take(id).ok_or(EditError::NotFound(id))
    }

    pub(crate) fn set_editor_metadata(&mut self, key: &str, node: Option<Node>) {
        if key == crate::measurements::KEY {
            self.reserved_measurement_ids = None;
        }
        self.extra.get_or_insert_with(|| self.doc.vmf.extra.clone());
        let existing = self.doc.vmf.extra.iter().position(|node| node.key == key);
        self.doc.vmf.extra.retain(|node| node.key != key);
        if let Some(node) = node {
            let index = existing
                .unwrap_or(self.doc.vmf.extra.len())
                .min(self.doc.vmf.extra.len());
            self.doc.vmf.extra.insert(index, node);
        }
    }

    pub fn set_context(&mut self, metadata: Metadata) -> Result<(), EditError> {
        metadata.validate().map_err(EditError::Invalid)?;
        self.extra.get_or_insert_with(|| self.doc.vmf.extra.clone());
        let existing = self
            .doc
            .vmf
            .extra
            .iter()
            .position(|node| node.key == context::METADATA_KEY);
        self.doc
            .vmf
            .extra
            .retain(|node| node.key != context::METADATA_KEY);
        if !metadata.notes.is_empty() || !metadata.extra.is_empty() {
            let index = existing
                .unwrap_or(self.doc.vmf.extra.len())
                .min(self.doc.vmf.extra.len());
            self.doc.vmf.extra.insert(index, metadata.to_node());
        }
        self.doc.context_cache = OnceLock::from(Ok(metadata));
        Ok(())
    }

    fn remove_deleted_context(&mut self) -> Result<(), EditError> {
        let mut removed = std::collections::BTreeSet::new();
        for (_, before) in &self.touched {
            match before {
                Some((_, Object::Solid(solid))) => {
                    removed.insert(ObjectId::Solid(solid.id));
                }
                Some((_, Object::Entity(entity))) => {
                    removed.insert(ObjectId::Entity(entity.id));
                    removed.extend(entity.solids.iter().map(|solid| ObjectId::Solid(solid.id)));
                }
                None => {}
            }
        }
        removed.retain(|id| !context::object_exists(self.doc, *id));
        context::remove_notes_for(self, &removed)
    }

    fn commit(self, label: String) {
        let doc = self.doc;
        let changes: Vec<Change> = self
            .touched
            .into_iter()
            .map(|(id, before)| Change {
                id,
                after: doc.state(id),
                before,
            })
            .filter(|c| c.before != c.after)
            .collect();
        let world = self
            .world
            .filter(|before| before != &doc.vmf.world.properties)
            .map(|before| (before, doc.vmf.world.properties.clone()));
        let extra = self
            .extra
            .filter(|before| before != &doc.vmf.extra)
            .map(|before| (before, doc.vmf.extra.clone()));
        let cordon = self
            .cordon
            .filter(|before| before != &doc.vmf.cordon)
            .map(|before| (before, doc.vmf.cordon.clone()));
        let groups = self
            .groups
            .filter(|before| before != &doc.vmf.world.groups)
            .map(|before| (before, doc.vmf.world.groups.clone()));
        let visgroups = self
            .visgroups
            .filter(|before| before != &doc.vmf.visgroups)
            .map(|before| (before, doc.vmf.visgroups.clone()));
        if changes.is_empty()
            && world.is_none()
            && extra.is_none()
            && cordon.is_none()
            && groups.is_none()
            && visgroups.is_none()
        {
            return;
        }
        doc.undo.push(Step {
            id: doc.revision + 1,
            label,
            changes,
            world,
            extra,
            cordon,
            groups,
            visgroups,
        });
        doc.redo.clear();
        doc.revision += 1;
    }

    fn rollback(self) {
        if let Some(before) = self.cordon {
            self.doc.vmf.cordon = before;
        }
        (
            self.doc.next_object_id,
            self.doc.next_side_id,
            self.doc.next_visgroup_id,
        ) = self.start_ids;
        if let Some(before) = self.visgroups {
            self.doc.vmf.visgroups = before;
        }
        self.doc.context_fingerprint.take();
        if let Some(groups) = self.groups {
            self.doc.vmf.world.groups = groups;
        }
        if let Some(extra) = self.extra {
            self.doc.vmf.extra = extra;
            self.doc.context_cache.take();
        }
        if let Some(before) = self.world {
            self.doc.vmf.world.properties = before;
        }
        self.doc.restore_objects(self.touched);
    }
}

#[cfg(test)]
#[path = "document_finalizer_tests.rs"]
mod finalizer_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_snapshots_preserve_state_and_reserved_ids_without_history() {
        let mut document = Document::empty();
        document
            .edit("context", |tx| {
                context::set_note(
                    tx,
                    Scope::Map,
                    "Room".into(),
                    "Keep the entrance clear".into(),
                    "system",
                    None,
                )
            })
            .unwrap();
        for value in ["sky_one", "sky_two"] {
            document
                .edit("sky", |tx| {
                    tx.world_properties_mut().set("skyname", value);
                    Ok(())
                })
                .unwrap();
        }
        document.undo().unwrap();
        document
            .edit("reserve IDs", |tx| {
                for _ in 0..10 {
                    tx.new_object_id()?;
                    tx.new_side_id()?;
                }
                Ok(())
            })
            .unwrap();
        assert!(!document.undo.is_empty() && !document.redo.is_empty());
        for saved in [false, true] {
            if saved {
                document.mark_saved();
            }
            let fingerprint = document.context_map_fingerprint().to_owned();
            let mut draft = document.preview_snapshot();
            assert_eq!(draft.vmf(), document.vmf());
            assert_eq!(draft.revision(), document.revision());
            assert_eq!(draft.is_dirty(), document.is_dirty());
            assert_eq!(draft.context(), document.context());
            assert_eq!(draft.context_map_fingerprint(), fingerprint);
            assert!(draft.undo().is_none() && draft.redo().is_none());
            let allocated = draft
                .edit("draft IDs", |tx| {
                    Ok((tx.new_object_id()?, tx.new_side_id()?))
                })
                .unwrap();
            assert_eq!(
                allocated,
                (document.next_object_id as u32, document.next_side_id as u32)
            );
            draft
                .edit("draft edit", |tx| {
                    tx.world_properties_mut().set("skyname", "draft");
                    Ok(())
                })
                .unwrap();
            assert_eq!(draft.undo_labels().count(), 1);
            assert_eq!(
                document.vmf().world.properties.get("skyname"),
                Some("sky_one")
            );
            draft.undo().unwrap();
            assert_eq!(draft.vmf(), document.vmf());
        }
    }

    #[test]
    fn world_undo_after_save_keeps_the_save_version() {
        let mut doc = Document::empty();
        doc.edit("sky", |tx| {
            tx.world_properties_mut().set("skyname", "sky_new");
            Ok(())
        })
        .unwrap();
        doc.bump_map_version();
        doc.mark_saved();
        doc.undo().unwrap();
        assert_eq!(
            doc.vmf().world.properties.get("skyname"),
            Some("sky_day01_01")
        );
        assert_eq!(doc.vmf().world.properties.get("mapversion"), Some("1"));
        doc.redo().unwrap();
        assert_eq!(doc.vmf().world.properties.get("skyname"), Some("sky_new"));
        assert_eq!(doc.vmf().world.properties.get("mapversion"), Some("1"));
    }

    #[test]
    fn world_and_object_edits_rollback_and_undo_together() {
        let mut doc = doc_with_entities(1);
        let before = doc.vmf().clone();
        let revision = doc.revision();
        let result: Result<(), EditError> = doc.edit("invalid", |tx| {
            tx.world_properties_mut().set("skyname", "changed");
            tx.entity_mut(2)?.properties.set("custom", "value");
            Err(EditError::Invalid("abort".into()))
        });
        assert!(result.is_err());
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), revision);
        doc.edit("world and entity", |tx| {
            tx.world_properties_mut().set("skyname", "changed");
            tx.entity_mut(2)?.properties.set("custom", "value");
            Ok(())
        })
        .unwrap();
        let after = doc.vmf().clone();
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
        doc.redo().unwrap();
        assert_eq!(doc.vmf(), &after);
        let revision = doc.revision();
        doc.edit("noop", |tx| {
            tx.world_properties_mut();
            Ok(())
        })
        .unwrap();
        assert_eq!(doc.revision(), revision);
    }

    fn doc_with_entities(n: u32) -> Document {
        let mut doc = Document::empty();
        doc.edit("add", |tx| {
            for _ in 0..n {
                let id = tx.new_object_id()?;
                tx.add_entity(Entity::new(id, "info_target"))?;
            }
            Ok(())
        })
        .unwrap();
        doc
    }

    #[test]
    fn undo_and_redo_restore_order_and_content() {
        let mut doc = doc_with_entities(3);
        let ids: Vec<u32> = doc.vmf().entities.iter().map(|e| e.id).collect();
        let original = doc.vmf().clone();

        doc.edit("delete middle", |tx| {
            tx.remove(ObjectId::Entity(ids[1])).map(drop)
        })
        .unwrap();
        doc.edit("rename first", |tx| {
            tx.entity_mut(ids[0])?.properties.set("targetname", "a");
            Ok(())
        })
        .unwrap();
        assert_eq!(doc.vmf().entities.len(), 2);

        assert_eq!(doc.undo().unwrap().label, "rename first");
        assert_eq!(doc.undo().unwrap().label, "delete middle");
        assert_eq!(doc.vmf(), &original);

        doc.redo();
        doc.redo();
        assert_eq!(doc.vmf().entities.len(), 2);
        assert_eq!(
            doc.entity(ids[0]).unwrap().properties.get("targetname"),
            Some("a")
        );
    }

    fn ordered_edit_fixture() -> (Document, Vec<u32>, Vec<u32>) {
        let mut document = doc_with_entities(5);
        document
            .edit("world brushes", |tx| {
                for _ in 0..3 {
                    crate::edit::create_block(
                        tx,
                        glam::DVec3::ZERO,
                        glam::DVec3::splat(16.0),
                        "test",
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let entities = document
            .vmf()
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect();
        let solids = document
            .vmf()
            .world
            .solids
            .iter()
            .map(|solid| solid.id)
            .collect();
        (document, entities, solids)
    }

    fn mixed_order_edit(
        tx: &mut Transaction<'_>,
        entities: &[u32],
        solids: &[u32],
    ) -> Result<(), EditError> {
        tx.entity_mut(entities[1])?
            .properties
            .set("targetname", "modified then deleted");
        tx.solid_mut(solids[1])?.hidden = true;
        let added = tx.new_object_id()?;
        tx.add_entity(Entity::new(added, "info_target"))?;
        crate::edit::create_block(tx, glam::DVec3::ZERO, glam::DVec3::splat(16.0), "test")?;
        tx.remove(ObjectId::Entity(entities[0]))?;
        tx.remove(ObjectId::Solid(solids[0]))?;
        tx.entity_mut(entities[4])?
            .properties
            .set("targetname", "modified after removal");
        tx.solid_mut(solids[2])?.hidden = true;
        let added = tx.new_object_id()?;
        tx.add_entity(Entity::new(added, "info_target"))?;
        crate::edit::create_block(tx, glam::DVec3::ZERO, glam::DVec3::splat(16.0), "test")?;
        tx.remove(ObjectId::Entity(entities[1]))?;
        tx.remove(ObjectId::Solid(solids[1]))?;
        Ok(())
    }

    #[test]
    fn mixed_add_remove_and_modify_transactions_replay_exact_object_order() {
        let (mut document, entities, solids) = ordered_edit_fixture();
        let original = document.vmf().clone();
        document
            .edit("mixed ordering", |tx| {
                mixed_order_edit(tx, &entities, &solids)
            })
            .unwrap();
        let committed = document.vmf().clone();
        assert_eq!(
            committed.entities[..3]
                .iter()
                .map(|entity| entity.id)
                .collect::<Vec<_>>(),
            entities[2..]
        );
        assert_eq!(committed.world.solids[0].id, solids[2]);
        for _ in 0..3 {
            document.undo().unwrap();
            assert_eq!(document.vmf(), &original);
            document.redo().unwrap();
            assert_eq!(document.vmf(), &committed);
        }
    }

    #[test]
    fn failed_mixed_structural_edits_restore_original_order_and_history() {
        let (mut document, entities, solids) = ordered_edit_fixture();
        let original = document.vmf().clone();
        let revision = document.revision();
        let history = document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let next_ids = (document.next_object_id, document.next_side_id);
        let result = document.edit("failed ordering", |tx| {
            mixed_order_edit(tx, &entities, &solids)?;
            Err::<(), _>(EditError::Invalid("reject after mixed edits".into()))
        });
        assert!(result.is_err());
        assert_eq!(document.vmf(), &original);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.undo_labels().collect::<Vec<_>>(), history);
        assert_eq!((document.next_object_id, document.next_side_id), next_ids);
    }

    #[test]
    fn failed_edits_roll_back_and_record_nothing() {
        let mut doc = doc_with_entities(1);
        let before = doc.vmf().clone();
        let revision = doc.revision();
        let id = before.entities[0].id;
        let result = doc.edit("broken", |tx| {
            tx.entity_mut(id)?.properties.set("targetname", "x");
            tx.remove(ObjectId::Entity(id))?;
            tx.remove(ObjectId::Solid(999))
        });
        assert_eq!(result, Err(EditError::NotFound(ObjectId::Solid(999))));
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), revision);
        assert_eq!(doc.undo_labels().count(), 1);
    }

    #[test]
    fn no_op_edits_are_not_recorded() {
        let mut doc = doc_with_entities(1);
        let id = doc.vmf().entities[0].id;
        doc.edit("nothing", |tx| tx.entity_mut(id).map(drop))
            .unwrap();
        assert_eq!(doc.undo_labels().collect::<Vec<_>>(), ["add"]);
    }

    #[test]
    fn new_ids_never_collide_with_existing_ones() {
        let mut doc = doc_with_entities(2);
        let max = doc.vmf().entities.iter().map(|e| e.id).max().unwrap();
        let id = doc.edit("id", |tx| tx.new_object_id()).unwrap();
        assert!(id > max);
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    use crate::edit;
    use glam::DVec3;

    fn solid(id: u32, first_side: u32) -> Solid {
        let mut fixture = Document::empty();
        fixture
            .edit("fixture", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "test")
            })
            .unwrap();
        let mut solid = fixture.vmf().world.solids[0].clone();
        solid.id = id;
        for (index, side) in solid.sides.iter_mut().enumerate() {
            side.id = first_side + index as u32;
        }
        solid
    }

    #[test]
    fn explicit_insertion_reserves_object_and_side_ids_including_child_brushes() {
        let mut document = Document::empty();
        document
            .edit("explicit brush", |tx| {
                tx.add_solid(solid(3, 100))?;
                assert_eq!(tx.new_object_id()?, 4);
                assert_eq!(tx.new_side_id()?, 106);
                Ok(())
            })
            .unwrap();
        let mut entity = Entity::new(20, "func_detail");
        entity.solids.push(solid(200, 1_000));
        document
            .edit("explicit child", |tx| {
                tx.add_entity(entity)?;
                assert_eq!(tx.new_object_id()?, 201);
                assert_eq!(tx.new_side_id()?, 1_006);
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        document.undo().unwrap();
        document.redo().unwrap();
        assert_eq!(document.vmf(), &source);
        document
            .edit("next", |tx| {
                assert_eq!(tx.new_object_id()?, 202);
                assert_eq!(tx.new_side_id()?, 1_007);
                Ok(())
            })
            .unwrap();
        document.vmf().validate_ids().unwrap();
    }

    #[test]
    fn insertion_checks_both_namespaces_globally_and_rolls_back_atomically() {
        let mut vmf = Document::empty().vmf().clone();
        vmf.world.groups.push(source_formats::vmf::Group {
            id: 40,
            editor: None,
            extra_properties: Properties::default(),
            extra: Vec::new(),
        });
        vmf.world.solids.push(solid(10, 100));
        let mut entity = Entity::new(20, "func_detail");
        entity.solids.push(solid(30, 200));
        vmf.entities.push(entity);
        let mut document = Document::try_new(vmf).unwrap();
        document
            .edit("history", |tx| {
                tx.world_properties_mut().set("skyname", "test");
                Ok(())
            })
            .unwrap();
        document.undo().unwrap();
        let original = document.vmf().clone();
        let counters = (document.next_object_id, document.next_side_id);
        let revision = document.revision();
        for collision in [1, 10, 20, 30, 40] {
            let error = document
                .edit("invalid object", |tx| {
                    tx.world_properties_mut().set("skyname", "discard me");
                    tx.add_solid(solid(collision, 1_000))
                })
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("duplicate object ID {collision}"))
            );
            assert_eq!(document.vmf(), &original);
            assert_eq!((document.next_object_id, document.next_side_id), counters);
            assert_eq!(document.revision(), revision);
            assert_eq!(document.redo_labels().count(), 1);
        }
        for first_side in [100, 200] {
            let error = document
                .edit("invalid side", |tx| tx.add_solid(solid(500, first_side)))
                .unwrap_err();
            assert!(error.to_string().contains("duplicate side ID"));
            assert_eq!((document.next_object_id, document.next_side_id), counters);
        }
        let mut duplicate_face = solid(500, 1_000);
        duplicate_face.sides[1].id = duplicate_face.sides[0].id;
        assert!(
            document
                .edit("duplicate face", |tx| tx.add_solid(duplicate_face))
                .is_err()
        );
        for child_id in [20, 30, 500] {
            let mut entity = Entity::new(500, "func_detail");
            entity.solids.push(solid(child_id, 1_000));
            assert!(
                document
                    .edit("invalid child", |tx| tx.add_entity(entity))
                    .is_err()
            );
        }
        assert_eq!(document.vmf(), &original);
        assert_eq!(document.undo_labels().count(), 0);
        assert_eq!(document.redo_labels().count(), 1);
        assert_eq!((document.next_object_id, document.next_side_id), counters);
        document
            .edit("independent namespaces", |tx| tx.add_solid(solid(100, 500)))
            .unwrap();
        document.vmf().validate_ids().unwrap();
    }

    #[test]
    fn duplicate_imports_are_rejected_without_renumbering() {
        let valid = Document::empty().vmf().clone();
        let mut duplicate_world = valid.clone();
        duplicate_world.entities.push(Entity::new(1, "info_target"));
        assert!(
            Document::try_new(duplicate_world)
                .unwrap_err()
                .to_string()
                .contains("duplicate object ID 1")
        );
        let mut duplicate_child = valid.clone();
        duplicate_child.world.solids.push(solid(2, 100));
        let mut entity = Entity::new(3, "func_detail");
        entity.solids.push(solid(2, 200));
        duplicate_child.entities.push(entity);
        assert!(
            Document::try_new(duplicate_child)
                .unwrap_err()
                .to_string()
                .contains("duplicate object ID 2")
        );
        let mut duplicate_side = valid;
        duplicate_side.world.solids.push(solid(2, 100));
        duplicate_side.world.solids.push(solid(3, 100));
        assert!(
            Document::try_new(duplicate_side)
                .unwrap_err()
                .to_string()
                .contains("duplicate side ID 100")
        );
    }

    #[test]
    fn maximum_imported_ids_remain_readable_and_fail_creation_cleanly() {
        let mut vmf = Document::empty().vmf().clone();
        vmf.world.id = u32::MAX;
        let mut document = Document::new(vmf.clone());
        let error = document
            .edit("create", |tx| {
                edit::create_entity(tx, "info_target", None, &[], vec![])
            })
            .unwrap_err();
        assert!(error.to_string().contains("IDs are exhausted"));
        assert_eq!(document.vmf(), &vmf);
        assert_eq!(document.revision(), 0);
    }

    #[test]
    fn partial_allocation_failure_restores_map_and_counters() {
        let mut document = Document::empty();
        document.next_side_id = u64::from(u32::MAX) - 2;
        let before = document.vmf().clone();
        let ids = (document.next_object_id, document.next_side_id);
        let error = document
            .edit("box", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "test")
            })
            .unwrap_err();
        assert!(error.to_string().contains("face IDs are exhausted"));
        assert_eq!((document.next_object_id, document.next_side_id), ids);
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), 0);
        assert_eq!(document.undo_labels().count(), 0);
        let last = document
            .edit("allocate", |tx| {
                tx.new_side_id()?;
                tx.new_side_id()?;
                tx.new_side_id()
            })
            .unwrap();
        assert_eq!(last, u32::MAX);
        assert!(document.edit("exhausted", |tx| tx.new_side_id()).is_err());
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_step_identity_and_summaries_survive_undo_redo_and_branch_replacement() {
        let mut document = Document::empty();
        document
            .edit("add marker and world", |tx| {
                let id = tx.new_object_id()?;
                tx.add_entity(Entity::new(id, "info_target"))?;
                tx.world_properties_mut().set("skyname", "first");
                Ok(())
            })
            .unwrap();
        let first = &document.undo_steps()[0];
        let first_id = first.id();
        assert_eq!(
            first.object_changes().collect::<Vec<_>>(),
            [(ObjectId::Entity(2), ChangeKind::Added)]
        );
        assert_eq!(first.sections().collect::<Vec<_>>(), ["world properties"]);
        document
            .edit("remove marker", |tx| {
                tx.remove(ObjectId::Entity(2)).map(drop)
            })
            .unwrap();
        let removed_id = document.undo_steps()[1].id();
        assert_eq!(
            document.undo_steps()[1]
                .object_changes()
                .collect::<Vec<_>>(),
            [(ObjectId::Entity(2), ChangeKind::Removed)]
        );
        document.undo().unwrap();
        assert_eq!(document.undo_steps()[0].id(), first_id);
        assert_eq!(document.redo_steps()[0].id(), removed_id);
        document.redo().unwrap();
        assert_eq!(document.undo_steps()[1].id(), removed_id);
        document.undo().unwrap();
        document
            .edit("modify marker", |tx| {
                tx.entity_mut(2)?.properties.set("targetname", "changed");
                Ok(())
            })
            .unwrap();
        assert!(document.redo_steps().is_empty());
        assert!(document.undo_steps()[1].id() > removed_id);
        assert_eq!(
            document.undo_steps()[1]
                .object_changes()
                .collect::<Vec<_>>(),
            [(ObjectId::Entity(2), ChangeKind::Modified)]
        );
    }
}
