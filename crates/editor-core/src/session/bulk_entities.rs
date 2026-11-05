use serde_json::{Value, json};
use source_formats::vmf::{Entity, Properties, Solid};

use super::*;

pub const MAX_BULK_ENTITIES: usize = 64;
const MAX_BRUSHES: usize = 256;
const MAX_SIDES: usize = 8192;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
type PropertyChanges = Vec<(String, Option<String>)>;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

#[derive(Clone)]
struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    definitions: u64,
    entities: Vec<u32>,
    rows: Vec<Value>,
    plan: Plan,
    has_incoming: bool,
}

#[derive(Clone)]
enum Plan {
    Class {
        classname: String,
        changes: Vec<(u32, PropertyChanges)>,
    },
    World {
        entities: Vec<u32>,
        solids: Vec<Solid>,
        ordering: Option<map_core::instance_order::Ordering>,
    },
}

impl Plan {
    fn operation(&self) -> &'static str {
        match self {
            Self::Class { .. } => "class_change",
            Self::World { .. } => "move_to_world",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Class { .. } => "change entity classes",
            Self::World { .. } => "move entities to world",
        }
    }

    fn apply(&self, tx: &mut Transaction) -> Result<Vec<ObjectId>, EditError> {
        match self {
            Self::Class { changes, .. } => {
                for (entity, changes) in changes {
                    map_core::edit::set_properties(tx, *entity, changes)?;
                }
                Ok(changes
                    .iter()
                    .map(|(id, _)| ObjectId::Entity(*id))
                    .collect())
            }
            Self::World {
                entities,
                solids,
                ordering,
            } => {
                for id in entities {
                    tx.remove(ObjectId::Entity(*id))?;
                }
                let ids = solids
                    .iter()
                    .map(|solid| tx.add_solid(solid.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(ordering) = ordering {
                    tx.set_instance_order(ordering)?;
                }
                map_core::groups::Graph::read(tx.document())?;
                map_core::layers::Graph::read(tx.document())?;
                Ok(ids)
            }
        }
    }
}

impl Draft {
    fn current(&self, session: &Session) -> bool {
        self.generation == session.document_generation
            && self.revision == session.document.revision()
            && self.definitions == session.definitions_serial
            && std::ptr::eq(session.render_document(), &session.document)
    }
}

impl State {
    pub(super) fn is_some(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.draft = None;
    }

    pub(super) fn close_draft(&self) -> Option<Value> {
        self.draft.as_ref().map(|draft| {
            json!({"token":draft.token,
            "generation":draft.generation,"revision":draft.revision,
            "definitions":draft.definitions,"entities":draft.entities,
            "operation":draft.plan.operation()})
        })
    }
}

impl Session {
    pub fn whole_entity_targets(&self, ids: &[ObjectId]) -> Result<Vec<u32>, String> {
        if ids.is_empty() || ids.len() > MAX_BULK_ENTITIES {
            return Err(format!(
                "Choose between 1 and {MAX_BULK_ENTITIES} whole entities"
            ));
        }
        self.require_root_objects(ids)?;
        let mut seen = BTreeSet::new();
        let mut entities = Vec::with_capacity(ids.len());
        let mut errors = Vec::new();
        for &id in ids {
            match id {
                ObjectId::Entity(entity) if self.document.entity(entity).is_some() => {
                    if seen.insert(entity) {
                        entities.push(entity);
                    } else {
                        errors.push(format!("entity:{entity} is listed more than once"));
                    }
                }
                _ => errors.push(format!("{id} is not a whole entity")),
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        entities.sort_unstable();
        Ok(entities)
    }

    pub(super) fn checked_entity_property_changes(
        &self,
        entity: u32,
        changes: &[(String, Option<String>)],
    ) -> Result<Vec<(String, Option<String>)>, String> {
        let current = self
            .document
            .entity(entity)
            .ok_or_else(|| format!("entity:{entity} does not exist"))?;
        let classname = changes
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case("classname"))
            .and_then(|(_, value)| value.as_deref())
            .or_else(|| current.classname())
            .unwrap_or("");
        if classname.trim().is_empty() {
            return Err("classname must not be empty".into());
        }
        if let Some(class) = self.definitions.class(classname) {
            let kind = if current.solids.is_empty() {
                ClassKind::Point
            } else {
                ClassKind::Solid
            };
            if class.kind != kind {
                return Err(format!(
                    "{} is a {} entity class",
                    class.name,
                    class.kind.label()
                ));
            }
        }
        for (key, value) in changes {
            self.validate_property(classname, key, value.as_deref())?;
        }
        let canonical = self.definitions.class(classname).map(|class| &class.name);
        let mut changes = changes.to_vec();
        if let Some(canonical) = canonical {
            for (key, value) in &mut changes {
                if key.eq_ignore_ascii_case("classname") && value.is_some() {
                    *value = Some(canonical.clone());
                }
            }
        }
        Ok(changes)
    }

    fn admit_bulk_entities(&self, entities: &[u32]) -> Result<(), String> {
        self.ensure_not_closing()?;
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        map_core::references::admit_snapshot_storage(
            self.document.vmf(),
            MAX_SNAPSHOT_BYTES,
            &mut || Ok(()),
        )?;
        let mut brushes = 0usize;
        let mut sides = 0usize;
        for id in entities {
            let entity = self.document.entity(*id).ok_or("Entity no longer exists")?;
            brushes += entity.solids.len();
            for solid in &entity.solids {
                sides += solid.sides.len();
            }
            if brushes > MAX_BRUSHES || sides > MAX_SIDES {
                return Err(format!(
                    "entity:{id}: batch exceeds {MAX_BRUSHES} brushes or {MAX_SIDES} sides"
                ));
            }
        }
        Ok(())
    }

    fn checked_future_class(&self, entity: &Entity, classname: &str) -> Result<(), String> {
        let mut keys = BTreeSet::new();
        for (key, value) in entity.properties.iter() {
            if !keys.insert(key.to_ascii_lowercase()) {
                return Err(format!(
                    "Duplicate {key} fields must be repaired before changing class"
                ));
            }
            if key.eq_ignore_ascii_case("classname") {
                continue;
            }
            if map_core::editor_metadata::is_storage_key(key) {
                source_formats::keyvalues::validate_string(value)
                    .map_err(|error| format!("{key}: {error}"))?;
                continue;
            }
            let property = self
                .definitions
                .class(classname)
                .and_then(|class| class.property(key));
            if let Some(property) = property.filter(|property| property.read_only) {
                source_formats::keyvalues::validate_string(key)
                    .map_err(|error| error.to_string())?;
                source_formats::keyvalues::validate_string(value)
                    .map_err(|error| format!("{key}: {error}"))?;
                property
                    .validate(value)
                    .map_err(|error| format!("{key}: {error}"))?;
                if (key.eq_ignore_ascii_case("origin") || key.eq_ignore_ascii_case("angles"))
                    && fgd::vector(value, 3).is_none()
                {
                    return Err(format!("{key} expects three finite coordinates"));
                }
            } else {
                self.validate_property(classname, key, Some(value))?;
            }
        }
        Ok(())
    }

    fn class_plan(&self, entities: &[u32], classname: &str) -> Result<(Plan, Vec<Value>), String> {
        let class = self.definitions.class(classname).ok_or_else(|| {
            format!("{classname} is not a loaded entity class; load its definitions first")
        })?;
        let classname = class.name.clone();
        let requested = [("classname".to_owned(), Some(classname.clone()))];
        let ordering = self
            .document
            .instance_order()
            .map_err(|error| error.to_string())?;
        let mut changes = Vec::with_capacity(entities.len());
        let mut rows = Vec::with_capacity(entities.len());
        let mut errors = Vec::new();
        let mut changed = false;
        for &id in entities {
            let entity = self.document.entity(id).ok_or("Entity no longer exists")?;
            let changes_class = !entity
                .classname()
                .is_some_and(|source| source.eq_ignore_ascii_case(&classname));
            let checked = if changes_class
                && ordering.is_some_and(|ordering| {
                    ordering
                        .origin_custody
                        .iter()
                        .any(|origin| origin.entity == id)
                }) {
                Err(
                    "has retained instance origin custody that cannot survive a class change"
                        .into(),
                )
            } else {
                self.checked_entity_property_changes(id, &requested)
                    .and_then(|changes| {
                        self.checked_future_class(entity, &classname)
                            .map(|()| changes)
                    })
            };
            match checked {
                Ok(checked) => {
                    changed |= entity.classname() != Some(classname.as_str());
                    changes.push((id, checked));
                    rows.push(json!({"entity":format!("entity:{id}"),
                        "name":entity.properties.get("targetname"),
                        "before":entity.classname(), "after":classname,
                        "brushes":entity.solids.len(),
                        "retained_properties":entity.properties.0.len().saturating_sub(1),
                        "retained_connections":entity.connections.as_ref().map_or(0, Vec::len)}));
                }
                Err(error) => errors.push(format!("entity:{id}: {error}")),
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if !changed {
            return Err("Every captured entity already has this class".into());
        }
        Ok((Plan::Class { classname, changes }, rows))
    }

    fn world_plan(&self, entities: &[u32]) -> Result<(Plan, Vec<Value>, bool), String> {
        let vmf = self.document.vmf();
        map_core::groups::Graph::read(&self.document).map_err(|error| error.to_string())?;
        map_core::layers::Graph::read(&self.document).map_err(|error| error.to_string())?;
        if vmf.extra.iter().any(|node| {
            ![
                map_core::context::METADATA_KEY,
                map_core::measurements::KEY,
                map_core::partition_metadata::KEY,
                map_core::free_partition_metadata::KEY,
                map_core::saved_views::KEY,
                map_core::texture_atlas::regions::KEY,
                "cordons",
            ]
            .contains(&node.key.as_str())
        }) {
            return Err(
                "Unknown map metadata cannot be proven independent of removed entity IDs".into(),
            );
        }
        let notes = map_core::context::Metadata::parse(vmf)?;
        if !notes.extra.is_empty() || notes.notes.values().any(|note| !note.extra.is_empty()) {
            return Err("Unknown note metadata cannot be preserved during entity removal".into());
        }
        let removed = entities.iter().copied().collect::<BTreeSet<_>>();
        map_core::measurements::Measurements::parse(vmf)?
            .validate_retained_references(&removed, &BTreeSet::new())?;
        let partitions = map_core::partition_metadata::Metadata::parse(vmf)?;
        partitions.validate()?;
        if !partitions.extra.is_empty()
            || partitions
                .entries
                .values()
                .any(|intent| !intent.extra.is_empty())
        {
            return Err(
                "Unknown partition metadata cannot be proven independent of removed entity IDs"
                    .into(),
            );
        }
        let free_partitions = map_core::free_partition_metadata::Metadata::parse(vmf)?;
        free_partitions.validate()?;
        if !free_partitions.extra.is_empty()
            || free_partitions
                .entries
                .values()
                .any(|intent| !intent.extra.is_empty())
        {
            return Err(
                "Unknown free-cut metadata cannot be proven independent of removed entity IDs"
                    .into(),
            );
        }
        let views = map_core::saved_views::Views::parse(vmf)?;
        if !views.extra.is_empty() || views.views.iter().any(|view| !view.extra.is_empty()) {
            return Err(
                "Unknown saved-view metadata cannot be proven independent of removed entity IDs"
                    .into(),
            );
        }
        let regions = map_core::texture_atlas::regions::Regions::parse(vmf)?;
        if !regions.extra.is_empty()
            || regions
                .regions
                .iter()
                .any(|region| !region.extra.is_empty())
        {
            return Err(
                "Unknown atlas-region metadata cannot be proven independent of removed entity IDs"
                    .into(),
            );
        }
        let mut ordering = map_core::instance_order::read(vmf)?;
        let mut solids = Vec::new();
        let mut rows = Vec::with_capacity(entities.len());
        let mut errors = Vec::new();
        for &id in entities {
            let entity = self.document.entity(id).ok_or("Entity no longer exists")?;
            let result = (|| {
                if entity.solids.is_empty() {
                    return Err("has no brushes to move to world".into());
                }
                if !entity.extra.is_empty() {
                    return Err(
                        "contains unknown entity blocks that cannot survive entity removal".into(),
                    );
                }
                if notes
                    .notes
                    .contains_key(&map_core::context::Scope::Object(ObjectId::Entity(id)))
                {
                    return Err(
                        "has an attached entity note; move or remove that note first".into(),
                    );
                }
                if ordering.as_ref().is_some_and(|ordering| {
                    ordering
                        .origin_custody
                        .iter()
                        .any(|origin| origin.entity == id)
                }) {
                    return Err(
                        "has retained instance origin custody that cannot become world geometry"
                            .into(),
                    );
                }
                let mut promoted = Vec::with_capacity(entity.solids.len());
                for solid in &entity.solids {
                    map_core::transform::validated_geometry(solid)
                        .map_err(|error| format!("solid:{}: {error}", solid.id))?;
                    promoted.push(promote_solid(entity, solid)?);
                }
                Ok::<_, String>(promoted)
            })();
            match result {
                Ok(promoted) => {
                    rows.push(json!({"entity":format!("entity:{id}"),
                        "name":entity.properties.get("targetname"), "before":entity.classname(),
                        "after":"world brushes", "brushes":entity.solids.len(),
                        "removed_properties":entity.properties.0,
                        "removed_connections":entity.connections.as_deref().unwrap_or_default().iter()
                            .map(|connection| json!({"output":connection.output,"value":connection.value})).collect::<Vec<_>>(),
                        "source_editor_properties":entity.editor.as_ref().map(|editor| &editor.0)}));
                    solids.extend(promoted);
                }
                Err(error) => errors.push(format!("entity:{id}: {error}")),
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if let Some(ordering) = &mut ordering {
            for slot in &mut ordering.slots {
                if let map_core::instance_order::Content::Materialized {
                    world,
                    entities: held,
                } = &mut slot.content
                {
                    for id in held.iter().filter(|id| removed.contains(id)) {
                        let entity = self.document.entity(*id).ok_or("Entity no longer exists")?;
                        world.extend(entity.solids.iter().map(|solid| solid.id));
                    }
                    held.retain(|id| !removed.contains(id));
                }
            }
            ordering.nested_entities.retain(|id| !removed.contains(id));
        }
        let catalog = map_core::references::CollapseCatalog::capture(&self.definitions)?;
        let incoming = map_core::references::has_incoming_checked(
            &self.document,
            &catalog,
            entities,
            &mut || Ok(()),
        )?;
        Ok((
            Plan::World {
                entities: entities.to_vec(),
                solids,
                ordering,
            },
            rows,
            incoming,
        ))
    }

    fn install_bulk_entities(
        &mut self,
        entities: Vec<u32>,
        plan: Plan,
        rows: Vec<Value>,
        has_incoming: bool,
    ) -> Result<Value, String> {
        let mut candidate = Document::new(self.document.vmf().clone());
        candidate
            .edit(plan.label(), |tx| plan.apply(tx))
            .map_err(|error| error.to_string())?;
        let token = self
            .bulk_entities
            .serial
            .checked_add(1)
            .ok_or("Entity action draft tokens exhausted")?;
        self.bulk_entities.serial = token;
        self.bulk_entities.draft = Some(Draft {
            token,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            entities,
            rows,
            plan,
            has_incoming,
        });
        self.view_serial += 1;
        Ok(self.bulk_entities_report())
    }

    pub fn preview_entity_class_change(
        &mut self,
        ids: &[ObjectId],
        classname: &str,
    ) -> Result<Value, String> {
        let entities = self.whole_entity_targets(ids)?;
        self.admit_bulk_entities(&entities)?;
        let (plan, rows) = self.class_plan(&entities, classname)?;
        self.install_bulk_entities(entities, plan, rows, false)
    }

    pub fn preview_move_entities_to_world(&mut self, ids: &[ObjectId]) -> Result<Value, String> {
        let entities = self.whole_entity_targets(ids)?;
        self.admit_bulk_entities(&entities)?;
        let (plan, rows, incoming) = self.world_plan(&entities)?;
        self.install_bulk_entities(entities, plan, rows, incoming)
    }

    pub fn bulk_entities_report(&self) -> Value {
        let Some(draft) = &self.bulk_entities.draft else {
            return Value::Null;
        };
        let current = draft.current(self);
        json!({"token":draft.token,"handle":Self::edit_handle("bulk_entities",current.then_some(draft.token)),
            "operation":draft.plan.operation(),"generation":draft.generation,"revision":draft.revision,
            "definitions":draft.definitions,"entities":draft.entities.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>(),
            "classname":match &draft.plan {Plan::Class{classname,..}=>Some(classname.as_str()),_=>None},
            "targets":draft.rows,"has_incoming_references":draft.has_incoming,
            "warnings":if matches!(draft.plan,Plan::World{..}) {
                let mut warnings=vec!["Entity behavior and connections are removed. Authored source brush coordinates and UVs are retained; runtime poses are not baked."];
                if draft.has_incoming {warnings.push("Incoming references are not rewritten and may become unresolved");} warnings
            } else {vec!["Stored properties and connections are retained. No defaults are inserted or obsolete properties removed."]},
            "can_apply":current,"error":(!current).then_some("Map, entity definitions or geometry preview changed; preview again")})
    }

    pub fn bulk_entities_handle(&self) -> Value {
        Self::edit_handle(
            "bulk_entities",
            self.bulk_entities
                .draft
                .as_ref()
                .filter(|draft| draft.current(self))
                .map(|draft| draft.token),
        )
    }

    pub fn ensure_bulk_entities_resolved(&self) -> Result<(), String> {
        if self.bulk_entities.is_some() {
            return Err("Apply or cancel the entity action preview first".into());
        }
        Ok(())
    }

    fn checked_bulk_entities(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .bulk_entities
            .draft
            .as_ref()
            .ok_or("No entity action preview")?;
        if draft.token != token {
            return Err("Entity action preview was replaced; inspect it again".into());
        }
        if !draft.current(self) {
            return Err(
                "Map, entity definitions or geometry preview changed; preview again".into(),
            );
        }
        Ok(draft)
    }

    pub fn apply_bulk_entities(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let draft = self.checked_bulk_entities(token)?.clone();
        let ids = self.edit(actor, draft.plan.label(), |tx| draft.plan.apply(tx))?;
        self.bulk_entities.clear();
        self.select(&ids, SelectMode::Replace)?;
        Ok(
            json!({"operation":draft.plan.operation(),"entities":draft.entities.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>(),
            "objects":ids.iter().map(|id|id.to_string()).collect::<Vec<_>>(),"revision":self.document.revision()}),
        )
    }

    pub fn cancel_bulk_entities(&mut self, token: u64) -> Result<(), String> {
        let draft = self
            .bulk_entities
            .draft
            .as_ref()
            .ok_or("No entity action preview")?;
        if draft.token != token {
            return Err("Entity action preview was replaced; inspect it again".into());
        }
        self.bulk_entities.clear();
        self.view_serial += 1;
        Ok(())
    }

    pub fn move_entities_to_world(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
    ) -> Result<Vec<ObjectId>, String> {
        let entities = self.whole_entity_targets(ids)?;
        self.admit_bulk_entities(&entities)?;
        let (plan, _, _) = self.world_plan(&entities)?;
        let ids = self.edit(actor, plan.label(), |tx| plan.apply(tx))?;
        self.select(&ids, SelectMode::Replace)?;
        Ok(ids)
    }
}

fn promote_solid(entity: &Entity, source: &Solid) -> Result<Solid, String> {
    let mut solid = source.clone();
    solid.hidden |= entity.hidden;
    let Some(parent) = &entity.editor else {
        return Ok(solid);
    };
    let editor = solid.editor.get_or_insert_with(Properties::default);
    let mut keys = BTreeSet::new();
    for (key, value) in parent.iter() {
        if !key.eq_ignore_ascii_case("visgroupid") {
            if !keys.insert(key.to_ascii_lowercase()) {
                return Err(format!("duplicate owner editor metadata {key}"));
            }
            if editor.get_all(key).take(2).count() > 1 {
                return Err(format!(
                    "solid:{}: duplicate child editor metadata {key}",
                    source.id
                ));
            }
        }
        if key.eq_ignore_ascii_case("logicalpos") {
            continue;
        }
        if key.eq_ignore_ascii_case("visgroupid") {
            if !editor.get_all(key).any(|existing| existing == value) {
                editor.0.push((key.into(), value.into()));
            }
        } else if key.eq_ignore_ascii_case("groupid") {
            if editor.get(key).is_some_and(|existing| existing != value) {
                return Err(format!(
                    "solid:{}: child and owner group memberships conflict",
                    source.id
                ));
            }
            editor.set(key, value);
        } else if ["visgroupshown", "visgroupautoshown"]
            .iter()
            .any(|known| key.eq_ignore_ascii_case(known))
        {
            if !matches!(value, "0" | "1")
                || editor.get_all(key).any(|value| !matches!(value, "0" | "1"))
            {
                return Err(format!("solid:{}: malformed {key} visibility", source.id));
            }
            if value == "0" || editor.get(key).is_none() {
                editor.set(key, value);
            }
        } else if key.eq_ignore_ascii_case("color") {
            if editor.get(key).is_none() {
                editor.set(key, value);
            }
        } else {
            return Err(format!(
                "unsupported owner editor metadata {key} cannot survive entity removal"
            ));
        }
    }
    Ok(solid)
}
