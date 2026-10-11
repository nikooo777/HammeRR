//! An editable map: object lookup, id allocation, and transactions with
//! undo/redo.
//!
//! Every change goes through [`Document::edit`]. A transaction snapshots each
//! object the first time it's touched; committing records before/after
//! states, so undo and redo restore whole objects at their original position
//! in the file.

use std::fmt;
use std::sync::OnceLock;

use crate::context::{self, Metadata, Scope};
use source_formats::keyvalues::Node;

use source_formats::vmf::{Entity, Properties, Solid, Vmf, World};

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

#[derive(Debug, Clone)]
struct Change {
    id: ObjectId,
    before: State,
    after: State,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub label: String,
    changes: Vec<Change>,
    world: Option<(Properties, Properties)>,
    extra: Option<(Vec<Node>, Vec<Node>)>,
}

impl Step {
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
    context_cache: OnceLock<Result<Metadata, String>>,
    context_fingerprint: OnceLock<String>,
}

impl Document {
    pub fn new(vmf: Vmf) -> Self {
        let mut object_max = vmf.world.id;
        let mut side_max = 0;
        let mut visit_solid = |s: &Solid| {
            object_max = object_max.max(s.id);
            side_max = s.sides.iter().map(|side| side.id).fold(side_max, u32::max);
        };
        vmf.world.solids.iter().for_each(&mut visit_solid);
        let mut entity_max = 0;
        for entity in &vmf.entities {
            entity_max = entity_max.max(entity.id);
            entity.solids.iter().for_each(&mut visit_solid);
        }
        let group_max = vmf.world.groups.iter().map(|g| g.id).max().unwrap_or(0);
        let context = Metadata::parse(&vmf);
        Self {
            context_cache: OnceLock::from(context),
            context_fingerprint: OnceLock::new(),
            next_object_id: u64::from(object_max.max(entity_max).max(group_max)) + 1,
            next_side_id: u64::from(side_max) + 1,
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
        let mut tx = Transaction {
            start_ids: (self.next_object_id, self.next_side_id),
            doc: self,
            touched: Vec::new(),
            world: None,
            extra: None,
        };
        let result = f(&mut tx).and_then(|value| tx.remove_deleted_context().map(|()| value));
        match result {
            Ok(result) => {
                tx.commit(label.into());
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
        if let Some((before, _)) = &step.extra {
            self.vmf.extra = before.clone();
            self.context_cache.take();
        }
        self.context_fingerprint.take();
        if let Some((before, after)) = &step.world {
            self.restore_world_properties(before, after);
        }
        for change in step.changes.iter().rev() {
            self.put(change.id, change.before.clone());
        }
        self.revision += 1;
        self.redo.push(step);
        self.redo.last()
    }

    /// Returns the redone step.
    pub fn redo(&mut self) -> Option<&Step> {
        let step = self.redo.pop()?;
        if let Some((_, after)) = &step.extra {
            self.vmf.extra = after.clone();
            self.context_cache.take();
        }
        self.context_fingerprint.take();
        if let Some((before, after)) = &step.world {
            self.restore_world_properties(after, before);
        }
        for change in &step.changes {
            self.put(change.id, change.after.clone());
        }
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
    start_ids: (u64, u64),
    doc: &'a mut Document,
    touched: Vec<(ObjectId, State)>,
    world: Option<Properties>,
    extra: Option<Vec<Node>>,
}

impl Transaction<'_> {
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
        loop {
            let id = u32::try_from(self.doc.next_object_id)
                .map_err(|_| EditError::Invalid("Map object IDs are exhausted".into()))?;
            self.doc.next_object_id += 1;
            let notes = &self.doc.context()?.notes;
            let reserved = notes.contains_key(&Scope::Object(ObjectId::Solid(id)))
                || notes.contains_key(&Scope::Object(ObjectId::Entity(id)));
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

    fn touch(&mut self, id: ObjectId) {
        self.doc.context_fingerprint.take();
        if !self.touched.iter().any(|(t, _)| *t == id) {
            let state = self.doc.state(id);
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
        if self.doc.contains(id) {
            return Err(EditError::Invalid(format!("{id} already exists")));
        }
        self.touch(id);
        self.doc.vmf.world.solids.push(solid);
        Ok(id)
    }

    /// Appends an entity. Its id must be unused.
    pub fn add_entity(&mut self, entity: Entity) -> Result<ObjectId, EditError> {
        let id = ObjectId::Entity(entity.id);
        if self.doc.contains(id) {
            return Err(EditError::Invalid(format!("{id} already exists")));
        }
        self.touch(id);
        self.doc.vmf.entities.push(entity);
        Ok(id)
    }

    pub fn remove(&mut self, id: ObjectId) -> Result<Object, EditError> {
        self.touch(id);
        self.doc.take(id).ok_or(EditError::NotFound(id))
    }

    pub(crate) fn set_editor_metadata(&mut self, key: &str, node: Option<Node>) {
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
        if changes.is_empty() && world.is_none() && extra.is_none() {
            return;
        }
        doc.undo.push(Step {
            label,
            changes,
            world,
            extra,
        });
        doc.redo.clear();
        doc.revision += 1;
    }

    fn rollback(self) {
        (self.doc.next_object_id, self.doc.next_side_id) = self.start_ids;
        self.doc.context_fingerprint.take();
        if let Some(extra) = self.extra {
            self.doc.vmf.extra = extra;
            self.doc.context_cache.take();
        }
        if let Some(before) = self.world {
            self.doc.vmf.world.properties = before;
        }
        for (id, before) in self.touched.into_iter().rev() {
            self.doc.put(id, before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
