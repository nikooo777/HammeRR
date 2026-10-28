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
        let mut snapshot = self.preview_snapshot_uncached();
        snapshot.context_cache = self.context_cache.clone();
        snapshot.context_fingerprint = self.context_fingerprint.clone();
        snapshot
    }

    pub fn preview_snapshot_uncached(&self) -> Self {
        Self {
            vmf: self.vmf.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: self.revision,
            saved_revision: self.saved_revision,
            next_object_id: self.next_object_id,
            next_side_id: self.next_side_id,
            next_visgroup_id: self.next_visgroup_id,
            context_cache: OnceLock::new(),
            context_fingerprint: OnceLock::new(),
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
