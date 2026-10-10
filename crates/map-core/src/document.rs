//! An editable map: object lookup, id allocation, and transactions with
//! undo/redo.
//!
//! Every change goes through [`Document::edit`]. A transaction snapshots each
//! object the first time it's touched; committing records before/after
//! states, so undo and redo restore whole objects at their original position
//! in the file.

use std::fmt;

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
    next_object_id: u32,
    next_side_id: u32,
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
        Self {
            next_object_id: object_max.max(entity_max).max(group_max) + 1,
            next_side_id: side_max + 1,
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

    /// Bumps `mapversion` in both places Hammer stores it, as a save does.
    pub fn bump_map_version(&mut self) {
        let bump = |props: &mut Properties| {
            let next = props
                .get("mapversion")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                + 1;
            props.set("mapversion", next.to_string());
        };
        if let Some(info) = &mut self.vmf.version_info {
            bump(info);
        }
        bump(&mut self.vmf.world.properties);
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
            doc: self,
            touched: Vec::new(),
        };
        match f(&mut tx) {
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
        for change in &step.changes {
            self.put(change.id, change.after.clone());
        }
        self.revision += 1;
        self.undo.push(step);
        self.undo.last()
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
    doc: &'a mut Document,
    touched: Vec<(ObjectId, State)>,
}

impl Transaction<'_> {
    pub fn vmf(&self) -> &Vmf {
        &self.doc.vmf
    }

    pub fn document(&self) -> &Document {
        self.doc
    }

    pub fn new_object_id(&mut self) -> u32 {
        let id = self.doc.next_object_id;
        self.doc.next_object_id += 1;
        id
    }

    pub fn new_side_id(&mut self) -> u32 {
        let id = self.doc.next_side_id;
        self.doc.next_side_id += 1;
        id
    }

    fn touch(&mut self, id: ObjectId) {
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
        if changes.is_empty() {
            return;
        }
        doc.undo.push(Step { label, changes });
        doc.redo.clear();
        doc.revision += 1;
    }

    fn rollback(self) {
        for (id, before) in self.touched.into_iter().rev() {
            self.doc.put(id, before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_entities(n: u32) -> Document {
        let mut doc = Document::empty();
        doc.edit("add", |tx| {
            for _ in 0..n {
                let id = tx.new_object_id();
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
        let id = doc.edit("id", |tx| Ok(tx.new_object_id())).unwrap();
        assert!(id > max);
    }
}
