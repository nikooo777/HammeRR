use std::sync::Arc;

use compiler::Control;
use compiler::instances::{self, DependencyGuard, InstanceStats, SourceLocation};
use map_core::references::{CollapseCatalog, admit_collapse_snapshot, has_incoming_checked};
use map_core::{Document, EditError, ObjectId, Transaction};
use serde_json::{Value, json};
use source_formats::vmf::{Entity, Solid};

use super::worker::{Slot, Ticket};
use super::*;

const MAX_PLAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: usize = 64;
const MAX_PROVENANCE_SOURCE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    form: Option<Form>,
    pub(super) worker: Slot,
}

#[derive(Clone)]
struct Key {
    generation: u64,
    revision: u64,
    definitions: u64,
    saved: u64,
    selection: BTreeSet<ObjectId>,
    path: Option<PathBuf>,
}

impl Key {
    fn capture(session: &Session) -> Self {
        Self {
            generation: session.document_generation,
            revision: session.document.revision(),
            definitions: session.definitions_serial,
            saved: session.history_save_serial,
            selection: session.selection.clone(),
            path: session.path.clone(),
        }
    }

    fn current(&self, session: &Session) -> bool {
        self.generation == session.document_generation
            && self.revision == session.document.revision()
            && self.definitions == session.definitions_serial
            && self.saved == session.history_save_serial
            && self.selection == session.selection
            && self.path == session.path
    }
}

struct Form {
    token: u64,
    owner: Actor,
    identity: Arc<()>,
    key: Key,
    control: Control,
    status: &'static str,
    error: Option<String>,
    ready: Option<Arc<Ready>>,
}

#[derive(Clone)]
struct FaceOrigin {
    side: u32,
    owner: u32,
    file: String,
    original_solid: u32,
    original_side: u32,
    occurrence: Arc<instances::OccurrencePath>,
}

#[derive(Clone)]
struct EntityOrigin {
    entity: u32,
    expanded: u32,
    owner: u32,
    file: String,
    original: u32,
    class: String,
    occurrence: Arc<instances::OccurrencePath>,
}

impl EntityOrigin {
    fn emitted(&self) -> bool {
        matches!(self.class.as_str(), "light" | "light_spot" | "info_target")
    }

    fn static_prop(&self) -> bool {
        self.class == "prop_static"
    }
}

#[derive(Clone)]
struct Plan {
    removed: Vec<ObjectId>,
    world: Vec<Solid>,
    entities: Vec<Entity>,
    entity_origins: Vec<EntityOrigin>,
    faces: Vec<FaceOrigin>,
    added: Vec<ObjectId>,
}

struct Ready {
    document: Document,
    plan: Plan,
    dependencies: DependencyGuard,
    prop_sources: Option<compiler::asset_sources::AssetSourceGuard>,
    stats: InstanceStats,
}

impl Ready {
    fn verify_metadata(&self) -> Result<(), String> {
        self.dependencies
            .verify_metadata()
            .map_err(|error| error.to_string())?;
        if let Some(sources) = &self.prop_sources {
            sources
                .verify_metadata()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

enum Work {
    Prepare {
        document: Box<Document>,
        source: Box<SourceLocation>,
        removed: Vec<ObjectId>,
        catalog: CollapseCatalog,
    },
    Apply(Arc<Ready>),
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    key: Key,
    control: Control,
    game: Arc<source_formats::gamefs::GameFs>,
    work: Work,
    _worker: Ticket,
}

pub struct Completed {
    token: u64,
    identity: Arc<()>,
    key: Key,
    control: Control,
    applying: bool,
    result: Result<Arc<Ready>, String>,
    _worker: Ticket,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        let applying = matches!(self.work, Work::Apply(_));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.control.checkpoint()?;
            let ready = match self.work {
                Work::Prepare {
                    document,
                    source,
                    removed,
                    catalog,
                } => {
                    let targets = removed.iter().map(|id| match id {
                        ObjectId::Entity(id) => *id,
                        _ => unreachable!(),
                    }).collect::<Vec<_>>();
                    if has_incoming_checked(&document, &catalog, &targets, &mut || self.control.checkpoint())? {
                        return Err("A parent reference targets an instance helper; remove the binding before collapse".into());
                    }
                    Arc::new(prepare(*document, *source, removed, &self.game, &self.control)?)
                },
                Work::Apply(ready) => ready,
            };
            ready
                .dependencies
                .verify(&self.control)
                .map_err(|error| error.to_string())?;
            if let Some(sources) = &ready.prop_sources {
                sources
                    .verify_sources(&self.game, &self.control)
                    .map_err(|error| error.to_string())?;
            }
            self.control.checkpoint()?;
            Ok(ready)
        }))
        .unwrap_or_else(|_| Err("Instance collapse worker panicked".into()));
        Completed {
            token: self.token,
            identity: self.identity,
            key: self.key,
            control: self.control,
            applying,
            result,
            _worker: self._worker,
        }
    }
}

fn prepare(
    mut document: Document,
    source: SourceLocation,
    removed: Vec<ObjectId>,
    game: &source_formats::gamefs::GameFs,
    control: &Control,
) -> Result<Ready, String> {
    let source = source.hydrate(control).map_err(|error| error.to_string())?;
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve_for_collapse(document.vmf(), Some(&source), control, &mut report)
            .map_err(|error| error.message)?;
    let dependencies = prepared
        .dependencies()
        .cloned()
        .ok_or("No visible instance references to collapse")?;
    let stats = prepared
        .stats()
        .cloned()
        .ok_or("Instance collapse statistics are missing")?;
    let prop_sources = prepared.validate_collapse_static_prop_poses(game, control)?;
    let mut plan_bytes =
        admit_collapse_snapshot(prepared.vmf(), MAX_PLAN_BYTES, &mut || control.checkpoint())?;
    plan_bytes = plan_bytes
        .checked_add(prepared.occurrence_bytes())
        .filter(|&bytes| bytes <= MAX_PLAN_BYTES)
        .ok_or("Collapsed occurrence metadata exceeds the plan limit")?;
    for brush in prepared.generated_brushes() {
        for side in &brush.solid.sides {
            control.checkpoint()?;
            let (_, file, _, _) = prepared
                .face_origin(side.id)
                .ok_or("Collapsed face provenance is missing")?;
            plan_bytes = plan_bytes
                .checked_add(
                    std::mem::size_of::<FaceOrigin>()
                        + std::mem::size_of::<Arc<instances::OccurrencePath>>(),
                )
                .and_then(|bytes| bytes.checked_add(file.len()))
                .filter(|bytes| *bytes <= MAX_PLAN_BYTES)
                .ok_or("Collapsed geometry and provenance exceed the 64 MiB plan limit")?;
        }
    }
    let mut entity_origins = Vec::new();
    for entity in &prepared.vmf().entities {
        control.checkpoint()?;
        if !matches!(
            entity.classname(),
            Some("light" | "light_spot" | "info_target" | "prop_static")
        ) && !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
        {
            continue;
        }
        let Some((owner, file, original)) = prepared.entity_origin(entity.id) else {
            continue;
        };
        let class = entity.classname().unwrap();
        plan_bytes = plan_bytes
            .checked_add(
                std::mem::size_of::<EntityOrigin>()
                    + std::mem::size_of::<Arc<instances::OccurrencePath>>(),
            )
            .and_then(|bytes| bytes.checked_add(file.len()))
            .and_then(|bytes| bytes.checked_add(class.len()))
            .filter(|bytes| *bytes <= MAX_PLAN_BYTES)
            .ok_or("Collapsed entity provenance exceeds the 64 MiB plan limit")?;
        entity_origins.push(EntityOrigin {
            entity: entity.id,
            expanded: entity.id,
            owner,
            file: file.to_owned(),
            original,
            class: class.to_owned(),
            occurrence: Arc::clone(
                prepared
                    .entity_occurrence(entity.id)
                    .ok_or("Collapsed entity occurrence is missing")?,
            ),
        });
    }
    let mut world = Vec::new();
    let mut faces = Vec::new();
    for brush in prepared.generated_brushes() {
        if !brush.detail {
            world.push(brush.solid.clone());
        }
        for side in &brush.solid.sides {
            let (owner, file, original_solid, original_side) = prepared
                .face_origin(side.id)
                .ok_or("Collapsed face provenance is missing")?;
            faces.push(FaceOrigin {
                side: side.id,
                owner,
                file: file.to_owned(),
                original_solid,
                original_side,
                occurrence: Arc::clone(
                    prepared
                        .face_occurrence(side.id)
                        .ok_or("Collapsed face occurrence is missing")?,
                ),
            });
        }
    }
    let entities = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| prepared.root_entity(entity.id).is_some())
        .cloned()
        .collect::<Vec<_>>();
    if world.is_empty() && entities.is_empty() {
        return Err(
            "Visible references contain no editable geometry, static props, lights or cubemaps"
                .into(),
        );
    }
    let mut plan = Plan {
        removed,
        world,
        entities,
        entity_origins,
        faces,
        added: Vec::new(),
    };
    drop(prepared);
    document
        .edit("preview instance collapse", |tx| {
            install(tx, &mut plan, control)
        })
        .map_err(|error| error.to_string())?;
    Ok(Ready {
        document,
        plan,
        dependencies,
        prop_sources,
        stats,
    })
}

fn charge_side_work(work: &mut usize, control: &Control) -> Result<(), EditError> {
    control.checkpoint().map_err(EditError::Invalid)?;
    *work = work
        .checked_add(1)
        .filter(|work| *work <= instances::MAX_GEOMETRY_WORK)
        .ok_or_else(|| {
            EditError::Invalid("Collapse side mapping exceeds the shared work limit".into())
        })?;
    Ok(())
}

fn fresh_solid(
    tx: &mut Transaction,
    solid: &mut Solid,
    sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<(), EditError> {
    solid.id = tx.new_object_id()?;
    for side in &mut solid.sides {
        charge_side_work(work, control)?;
        side.id = sides
            .get(&side.id)
            .copied()
            .filter(|id| *id != 0)
            .ok_or_else(|| EditError::Invalid("Collapsed side ID mapping is incomplete".into()))?;
    }
    Ok(())
}

fn install(tx: &mut Transaction, plan: &mut Plan, control: &Control) -> Result<(), EditError> {
    for &id in &plan.removed {
        tx.remove(id)?;
    }
    let mut sides = BTreeMap::new();
    let mut binding_work = 0;
    for solid in plan
        .world
        .iter()
        .chain(plan.entities.iter().flat_map(|entity| &entity.solids))
    {
        for side in &solid.sides {
            charge_side_work(&mut binding_work, control)?;
            if sides.insert(side.id, 0).is_some() {
                return Err(EditError::Invalid(
                    "Collapsed source side IDs are not unique".into(),
                ));
            }
        }
    }
    for authored in sides.values_mut() {
        charge_side_work(&mut binding_work, control)?;
        *authored = tx.new_side_id()?;
    }
    plan.added.clear();
    for solid in &mut plan.world {
        fresh_solid(tx, solid, &sides, &mut binding_work, control)?;
        plan.added.push(tx.add_solid(solid.clone())?);
    }
    let mut entities = BTreeMap::new();
    for entity in &mut plan.entities {
        let original = entity.id;
        entity.id = tx.new_object_id()?;
        entities.insert(original, entity.id);
        for solid in &mut entity.solids {
            fresh_solid(tx, solid, &sides, &mut binding_work, control)?;
        }
        plan.added.push(tx.add_entity(entity.clone())?);
    }
    for entity in &mut plan.entities {
        if let Some(rewritten) =
            instances::remap_collapsed_cubemap_sides(entity, &sides, &mut binding_work, control)
                .map_err(EditError::Invalid)?
        {
            tx.entity_mut(entity.id)?
                .properties
                .set("sides", rewritten.clone());
            entity.properties.set("sides", rewritten);
        }
    }
    for origin in &mut plan.entity_origins {
        origin.entity = *entities.get(&origin.entity).ok_or_else(|| {
            EditError::Invalid("Collapsed entity ID mapping is incomplete".into())
        })?;
    }
    for face in &mut plan.faces {
        face.side = *sides
            .get(&face.side)
            .ok_or_else(|| EditError::Invalid("Collapsed face ID mapping is incomplete".into()))?;
    }
    tx.vmf()
        .validate_ids()
        .map_err(|error| EditError::Invalid(error.to_string()))
}

impl Session {
    pub fn instance_collapse_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.history_save_serial,
            self.scene_serial,
            self.geometry.instance_collapse.serial
        )
    }

    fn collapse_context(&self) -> Result<(), String> {
        if self.closing || self.compiling() || self.launching() || self.instance_workers_busy() {
            return Err(
                "Wait for active builds, launches or instance workers before collapsing references"
                    .into(),
            );
        }
        if (self.history_navigation_blocked().is_some()
            && self.geometry.instance_collapse.document().is_none())
            || self.property_draft.is_some()
            || self.rename_draft.is_some()
            || self.connection_draft.is_some()
            || self.face_mode
            || self.clip_tool
            || self.spatial.draw_block
            || self.geometry.paint.enabled
            || self.component_picking().is_some()
            || !self.component_selection().is_empty()
        {
            return Err("Apply or cancel other editing tools and use Objects mode before collapsing references".into());
        }
        Ok(())
    }

    pub fn begin_instance_collapse(
        &mut self,
        actor: Actor,
        snapshot: &str,
    ) -> Result<Request, String> {
        if snapshot != self.instance_collapse_stamp() {
            return Err("Map or collapse review changed; inspect again".into());
        }
        self.collapse_context()?;
        if self
            .geometry
            .instance_collapse
            .form
            .as_ref()
            .is_some_and(|form| form.owner != actor)
        {
            return Err("Review the shared collapse before replacing it".into());
        }
        self.path
            .as_deref()
            .ok_or("Save the parent VMF before collapsing instances")?
            .to_str()
            .ok_or("Save the parent VMF through a UTF-8 path before collapsing instances")?;
        admit_collapse_snapshot(self.document.vmf(), MAX_PLAN_BYTES, &mut || Ok(()))?;
        let catalog = CollapseCatalog::capture(&self.definitions)?;
        let removed = self
            .document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            })
            .map(|entity| ObjectId::Entity(entity.id))
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return Err("There are no visible instance references to collapse".into());
        }
        if removed.len() > instances::MAX_INSTANCES
            || self
                .document
                .vmf()
                .object_ids()
                .take(instances::MAX_OBJECTS + 1)
                .count()
                > instances::MAX_OBJECTS
            || self
                .document
                .vmf()
                .side_ids()
                .take(instances::MAX_SIDES + 1)
                .count()
                > instances::MAX_SIDES
        {
            return Err(
                "Instance collapse exceeds the compiler's object, face or reference limits".into(),
            );
        }
        if self.document.vmf().extra.iter().any(|node| {
            ![
                map_core::context::METADATA_KEY,
                map_core::measurements::KEY,
                map_core::partition_metadata::KEY,
                map_core::saved_views::KEY,
                map_core::texture_atlas::regions::KEY,
                "cordons",
            ]
            .contains(&node.key.as_str())
        }) {
            return Err(
                "Unknown parent metadata cannot be proven independent of removed reference IDs"
                    .into(),
            );
        }
        let notes = map_core::context::Metadata::parse(self.document.vmf())?;
        if !notes.extra.is_empty() || notes.notes.values().any(|note| !note.extra.is_empty()) {
            return Err("Custom note metadata cannot be remapped during instance collapse".into());
        }
        if removed.iter().any(|id| {
            notes
                .notes
                .contains_key(&map_core::context::Scope::Object(*id))
        }) {
            return Err("Instance helpers with attached notes cannot be collapsed".into());
        }
        let measurements = map_core::measurements::Measurements::parse(self.document.vmf())?;
        if !measurements.extra.is_empty()
            || measurements.measurements.iter().any(|measurement| {
                measurement.extra.iter().any(|node| {
                    !matches!(
                        node.key.as_str(),
                        "start_entity_origin" | "end_entity_origin"
                    )
                })
            })
        {
            return Err(
                "Custom measurement metadata cannot be remapped during instance collapse".into(),
            );
        }
        for measurement in &measurements.measurements {
            for endpoint in [
                map_core::measurements::Endpoint::Start,
                map_core::measurements::Endpoint::End,
            ] {
                if measurement
                    .origin_reference(endpoint)?
                    .is_some_and(|id| removed.contains(&ObjectId::Entity(id)))
                {
                    return Err("A measurement is attached to an instance helper; detach it before collapse".into());
                }
            }
        }
        let source = SourceLocation::capture(
            self.path
                .as_deref()
                .ok_or("Save the parent VMF before collapsing instances")?,
        )
        .map_err(|error| error.to_string())?;
        self.retire_clean_instance_reference(actor)?;
        let worker = self
            .geometry
            .instance_collapse
            .worker
            .begin()
            .ok_or("Wait for the collapse worker")?;
        let ready = self
            .geometry
            .instance_collapse
            .form
            .take()
            .and_then(|form| form.ready);
        self.geometry.instance_collapse.serial += 1;
        let token = self.geometry.instance_collapse.serial;
        let key = Key::capture(self);
        let identity = Arc::new(());
        let control = Control::default();
        self.geometry.instance_collapse.form = Some(Form {
            token,
            owner: actor,
            identity: identity.clone(),
            key: key.clone(),
            control: control.clone(),
            status: "preparing",
            error: None,
            ready,
        });
        Ok(Request {
            token,
            identity,
            key,
            control,
            game: self.game.clone(),
            work: Work::Prepare {
                document: Box::new(self.document.preview_snapshot_uncached()),
                source: Box::new(source),
                removed,
                catalog,
            },
            _worker: worker,
        })
    }

    pub fn apply_instance_collapse(&mut self, actor: Actor, token: u64) -> Result<Request, String> {
        self.collapse_context()?;
        let form = self.collapse_form(actor, token)?;
        let ready = form
            .ready
            .clone()
            .ok_or("Prepare an instance collapse preview first")?;
        let worker = self
            .geometry
            .instance_collapse
            .worker
            .begin()
            .ok_or("Wait for the collapse worker")?;
        let request = Request {
            token,
            identity: form.identity.clone(),
            key: form.key.clone(),
            control: form.control.clone(),
            game: self.game.clone(),
            work: Work::Apply(ready),
            _worker: worker,
        };
        self.geometry
            .instance_collapse
            .form
            .as_mut()
            .unwrap()
            .status = "checking";
        Ok(request)
    }

    fn collapse_form(&self, actor: Actor, token: u64) -> Result<&Form, String> {
        let form = self
            .geometry
            .instance_collapse
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Instance collapse token expired")?;
        if form.owner != actor {
            return Err("Review the shared collapse before changing it".into());
        }
        if !form.key.current(self) {
            return Err("Map, selection or saved source changed; prepare collapse again".into());
        }
        if form.control.cancelled() {
            return Err("Instance collapse was cancelled".into());
        }
        Ok(form)
    }

    pub fn finish_instance_collapse(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let form = self
            .geometry
            .instance_collapse
            .form
            .as_ref()
            .filter(|form| {
                form.token == completed.token && Arc::ptr_eq(&form.identity, &completed.identity)
            })
            .ok_or("Instance collapse result expired")?;
        if form.owner != actor
            || !completed.key.current(self)
            || self.closing
            || completed.control.cancelled()
        {
            return Err("Instance collapse result is stale or cancelled".into());
        }
        let result = completed.result.and_then(|ready| {
            ready.verify_metadata()?;
            Ok(ready)
        });
        let ready = match result {
            Ok(ready) => ready,
            Err(error) => {
                let form = self.geometry.instance_collapse.form.as_mut().unwrap();
                form.status = "failed";
                form.error = Some(error.clone());
                return Err(error);
            }
        };
        if !completed.applying {
            self.clear_geometry_previews_except(&[previews::Kind::InstanceCollapse]);
            let form = self.geometry.instance_collapse.form.as_mut().unwrap();
            form.ready = Some(ready);
            form.status = "ready";
            form.error = None;
            self.scene_serial += 1;
            self.view_serial += 1;
            return Ok(self.get_instance_collapse());
        }
        let mut plan = ready.plan.clone();
        let captured_preview = self.get_instance_collapse();
        let discarded = self
            .document
            .redo_steps()
            .iter()
            .map(|step| step.id())
            .collect::<Vec<_>>();
        let before = self.document.revision();
        let applied = self.document.edit_with_finalizer(
            "collapse all visible instance references",
            |tx| {
                install(tx, &mut plan, &completed.control)?;
                if plan.added != ready.plan.added || tx.vmf() != ready.document.vmf() {
                    return Err(EditError::Invalid(
                        "Collapse differs from its reviewed preview".into(),
                    ));
                }
                Ok(())
            },
            |_| ready.verify_metadata().map_err(EditError::Invalid),
        );
        if let Err(error) = applied {
            let error = error.to_string();
            let form = self.geometry.instance_collapse.form.as_mut().unwrap();
            form.status = "failed";
            form.error = Some(error.clone());
            return Err(error);
        }
        if self.document.revision() != before {
            for id in discarded {
                self.history_actors.remove(&id);
            }
            self.history_actors.insert(self.document.revision(), actor);
            self.selection = ready.plan.added.iter().copied().collect();
            self.face_selection.clear();
            self.changed();
            self.log(actor, "collapsed all visible instance references");
        }
        Ok(
            json!({"applied":true,"objects":ready.plan.added.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"captured_preview":captured_preview,"state":self.get_instance_collapse()}),
        )
    }

    pub fn review_instance_collapse(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if self.geometry.instance_collapse.worker.busy() {
            return Err("Wait for the collapse worker before reviewing ownership".into());
        }
        let form = self
            .geometry
            .instance_collapse
            .form
            .as_mut()
            .filter(|form| form.token == token)
            .ok_or("Instance collapse token expired")?;
        self.geometry.instance_collapse.serial += 1;
        form.token = self.geometry.instance_collapse.serial;
        form.owner = actor;
        Ok(self.get_instance_collapse())
    }

    pub fn cancel_instance_collapse(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let form = self
            .geometry
            .instance_collapse
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Instance collapse token expired")?;
        if form.owner != actor {
            return Err("Review the shared collapse before cancelling it".into());
        }
        self.clear_instance_collapse();
        Ok(self.get_instance_collapse())
    }

    pub(super) fn clear_instance_collapse(&mut self) {
        if let Some(form) = self.geometry.instance_collapse.form.take() {
            let _ = form.control.cancel();
            self.geometry.instance_collapse.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn get_instance_collapse(&self) -> Value {
        let state = &self.geometry.instance_collapse;
        let Some(form) = &state.form else {
            return json!({"snapshot":self.instance_collapse_stamp(),"token":null,"ready":false,"worker_active":state.worker.busy(),"scope":"all_visible_references","writes_child_files":false});
        };
        let blocked = if !form.key.current(self) {
            Some("Map, selection or saved source changed".to_owned())
        } else {
            form.ready
                .as_ref()
                .and_then(|ready| ready.verify_metadata().err())
        };
        let ready = form.ready.as_ref();
        let provenance_error = ready.and_then(|ready| {
            let bytes = ready
                .plan
                .faces
                .iter()
                .take(MAX_ROWS)
                .map(|face| (face.file.len(), &face.occurrence))
                .chain(
                    ready
                        .plan
                        .entity_origins
                        .iter()
                        .take(MAX_ROWS)
                        .map(|origin| (origin.file.len(), &origin.occurrence)),
                )
                .try_fold(0usize, |bytes, (file, route)| {
                    bytes
                        .checked_add(2 * file)
                        .and_then(|bytes| bytes.checked_add(route.response_copy_bytes()))
                        .filter(|&bytes| bytes <= MAX_PROVENANCE_SOURCE_BYTES)
                });
            bytes
                .is_none()
                .then_some("Captured provenance routes exceed the bounded response-copy budget")
        });
        json!({"snapshot":self.instance_collapse_stamp(),"token":form.token,"owner":form.owner.label(),"status":form.status,"error":blocked.as_ref().or(form.error.as_ref()),"ready":ready.is_some() && blocked.is_none() && form.status=="ready","worker_active":state.worker.busy(),"scope":"all_visible_references","recursive":true,"source":form.key.path,"stats":ready.map(|ready|&ready.stats),"removed":ready.map(|ready|ready.plan.removed.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>()),"objects":ready.map(|ready|ready.plan.added.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>()),"face_provenance":ready.filter(|_|provenance_error.is_none()).map(|ready|ready.plan.faces.iter().take(MAX_ROWS).map(|face|json!({"side":face.side,"owner":crate::identity::id_string(ObjectId::Entity(face.owner)),"file":face.file,"original_solid":face.original_solid,"original_side":face.original_side,"occurrence":face.occurrence.as_ref(),"occurrence_label":face.occurrence.label()})).collect::<Vec<_>>()),"face_provenance_total":ready.map(|ready|ready.plan.faces.len()),"entity_id_map":ready.map(|ready|ready.plan.entity_origins.iter().filter(|origin|origin.emitted()).map(|origin|json!({"expanded":crate::identity::id_string(ObjectId::Entity(origin.expanded)),"authored":crate::identity::id_string(ObjectId::Entity(origin.entity))})).collect::<Vec<_>>()),"helper_id_map":ready.map(|ready|ready.plan.entity_origins.iter().filter(|origin|origin.class.eq_ignore_ascii_case("env_cubemap")).map(|origin|json!({"expanded":crate::identity::id_string(ObjectId::Entity(origin.expanded)),"authored":crate::identity::id_string(ObjectId::Entity(origin.entity))})).collect::<Vec<_>>()),"static_prop_id_map":ready.map(|ready|ready.plan.entity_origins.iter().filter(|origin|origin.static_prop()).map(|origin|json!({"expanded":crate::identity::id_string(ObjectId::Entity(origin.expanded)),"authored":crate::identity::id_string(ObjectId::Entity(origin.entity))})).collect::<Vec<_>>()),"entity_provenance":ready.filter(|_|provenance_error.is_none()).map(|ready|ready.plan.entity_origins.iter().take(MAX_ROWS).map(|origin|json!({"entity":crate::identity::id_string(ObjectId::Entity(origin.entity)),"expanded":crate::identity::id_string(ObjectId::Entity(origin.expanded)),"owner":crate::identity::id_string(ObjectId::Entity(origin.owner)),"file":origin.file,"original_entity":origin.original,"classname":origin.class,"emitted":origin.emitted(),"compiler_handled":!origin.emitted(),"occurrence":origin.occurrence.as_ref(),"occurrence_label":origin.occurrence.label()})).collect::<Vec<_>>()),"entity_provenance_total":ready.map(|ready|ready.plan.entity_origins.len()),"provenance_error":provenance_error,"limits":{"plan_bytes":MAX_PLAN_BYTES,"provenance_source_bytes":MAX_PROVENANCE_SOURCE_BYTES,"occurrence_bytes":instances::MAX_OCCURRENCE_BYTES,"rows":MAX_ROWS,"helper_levels":instances::MAX_EXPANSION_DEPTH},"writes_child_files":false})
    }
}

impl State {
    pub(super) fn document(&self) -> Option<&Document> {
        self.form
            .as_ref()
            .and_then(|form| form.ready.as_ref())
            .map(|ready| &ready.document)
    }
    pub(super) fn selection(&self) -> Option<&[ObjectId]> {
        self.form
            .as_ref()
            .and_then(|form| form.ready.as_ref())
            .map(|ready| ready.plan.added.as_slice())
    }
    pub(super) fn close_draft(&self) -> Option<Value> {
        self.form.as_ref().map(|form| {
            json!([
                self.serial,
                form.token,
                form.owner.label(),
                form.status,
                form.error,
                form.ready.is_some(),
                form.control.cancelled()
            ])
        })
    }
    pub(super) fn close_work(&self) -> Value {
        json!({"sequence":self.serial,"worker_active":self.worker.busy()})
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod light_tests;

#[cfg(test)]
mod cubemap_tests;

#[cfg(test)]
mod static_prop_tests;
