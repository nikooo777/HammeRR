use std::sync::Arc;

use compiler::instances::{self, NewReferencePath, SourceContext, SourceLocation};
use compiler::{Control, FileIdentity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source_formats::vmf::{Entity, Solid, Vmf};

use super::worker::{Slot, Ticket};
use super::*;

mod admission;
#[cfg(not(windows))]
mod staging;
#[cfg(windows)]
#[path = "instance_extraction/staging_windows.rs"]
mod staging;
use staging::Stage;

const MAX_OBJECTS: usize = 64;
const MAX_BRUSHES: usize = 256;
const MAX_SIDES: usize = 2048;
const STAGING_NAME_BYTES: usize = 64;
const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Pivot {
    #[default]
    WorldOrigin,
    SelectionCentre,
    Custom(DVec3),
}

impl Pivot {
    fn mode(self) -> &'static str {
        match self {
            Self::WorldOrigin => "world_origin",
            Self::SelectionCentre => "selection_centre",
            Self::Custom(_) => "custom",
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    form: Option<Form>,
    last: Value,
    pub(super) worker: Slot,
    completed: Option<(u64, Actor)>,
    cleanup: staging::Cleanup,
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
    identity: Arc<()>,
    token: u64,
    owner: Actor,
    key: Key,
    control: Control,
    status: &'static str,
    error: Option<String>,
    destination: NewReferencePath,
    pivot: DVec3,
    pivot_mode: &'static str,
    ready: Option<Ready>,
}

struct Plan {
    child: Vmf,
    originals: Vec<(Solid, bool)>,
    pivot: DVec3,
    pivot_mode: &'static str,
    helper: Entity,
    selected: Vec<ObjectId>,
    world_brushes: usize,
    detail_brushes: usize,
    sides: usize,
    allocator_metadata: Vec<source_formats::keyvalues::Node>,
}

struct Ready {
    plan: Plan,
    stage: Stage,
    identity: FileIdentity,
    source: SourceContext,
    bytes: usize,
    sha256: String,
}

enum Work {
    Prepare {
        plan: Box<Plan>,
        location: Box<SourceLocation>,
    },
    Publish(Box<Ready>),
}

pub struct Request {
    identity: Arc<()>,
    token: u64,
    key: Key,
    control: Control,
    destination: NewReferencePath,
    work: Work,
    cleanup: staging::Cleanup,
    worker: Ticket,
}

pub struct Completed {
    identity: Arc<()>,
    token: u64,
    key: Key,
    publishing: bool,
    result: Result<Ready, String>,
    _worker: Ticket,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        self.run_with(run_work)
    }

    fn run_with(
        self,
        run: impl FnOnce(Work, &NewReferencePath, &Control, staging::Cleanup) -> Result<Ready, String>,
    ) -> Completed {
        let publishing = matches!(&self.work, Work::Publish(_));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(self.work, &self.destination, &self.control, self.cleanup)
        }))
        .unwrap_or_else(|_| Err("Instance extraction worker panicked".into()));
        Completed {
            identity: self.identity,
            token: self.token,
            key: self.key,
            publishing,
            result,
            _worker: self.worker,
        }
    }
}

fn run_work(
    work: Work,
    destination: &NewReferencePath,
    control: &Control,
    cleanup: staging::Cleanup,
) -> Result<Ready, String> {
    control.checkpoint()?;
    destination
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    match work {
        Work::Publish(ready) => {
            ready
                .source
                .verify(control)
                .map_err(|error| error.to_string())?;
            ready
                .identity
                .verify(control)
                .map_err(|error| error.to_string())?;
            ready.stage.verify_owned()?;
            control.checkpoint()?;
            Ok(*ready)
        }
        Work::Prepare { plan, location } => {
            prepare(*plan, *location, destination, control, cleanup)
        }
    }
}

fn prepare(
    mut plan: Plan,
    location: SourceLocation,
    destination: &NewReferencePath,
    control: &Control,
    cleanup: staging::Cleanup,
) -> Result<Ready, String> {
    if plan.pivot != DVec3::ZERO {
        for solid in plan.child.world.solids.iter_mut().chain(
            plan.child
                .entities
                .iter_mut()
                .flat_map(|entity| entity.solids.iter_mut()),
        ) {
            control.checkpoint()?;
            map_core::edit::translate_solid(solid, -plan.pivot, true)
                .map_err(|error| error.to_string())?;
        }
    }
    let input = plan
        .child
        .try_write(&Default::default())
        .map_err(|error| error.to_string())?;
    if input.len() > instances::MAX_FILE_BYTES {
        return Err("Extracted child exceeds 4 MiB".into());
    }
    let nodes = source_formats::keyvalues::parse_bounded(
        &input,
        instances::MAX_DEPTH,
        instances::MAX_FILE_NODES,
    )
    .map_err(|error| error.to_string())?;
    if Vmf::from_nodes(&nodes).map_err(|error| error.to_string())? != plan.child {
        return Err("Child serialization changed typed IDs, metadata or source fields".into());
    }
    let source = location
        .clone()
        .hydrate(control)
        .map_err(|error| error.to_string())?;
    let mut stage = Stage::create(
        destination
            .path()
            .parent()
            .ok_or("Child directory is missing")?,
        cleanup,
    )?;
    stage.write(input.as_bytes())?;
    stage.verify_owned()?;
    let reference = location
        .reference_from_path(stage.path())
        .map_err(|error| error.to_string())?;
    let mut isolated = Document::empty().into_vmf();
    isolated.world.properties = source_formats::vmf::Properties::default();
    isolated.world.properties.set("classname", "worldspawn");
    let mut helper = plan.helper.clone();
    helper.id = isolated
        .world
        .id
        .checked_add(1)
        .ok_or("Helper ID overflow")?;
    helper.properties.set("file", reference.relative());
    isolated.entities.push(helper);
    let mut report = compiler::Report::default();
    let resolved = instances::resolve(&isolated, Some(&source), control, &mut report)
        .map_err(|error| error.message)?;
    let expected = plan
        .originals
        .iter()
        .map(|(solid, detail)| (solid, *detail));
    let generated = resolved.generated_brushes().collect::<Vec<_>>();
    if generated.len() != plan.originals.len() {
        return Err("Serialized child changed the extracted brush count".into());
    }
    for ((original, detail), generated) in expected.zip(generated) {
        control.checkpoint()?;
        if generated.original != original.id || generated.detail != detail {
            return Err("Serialized child changed brush order or ownership".into());
        }
        let mut actual = generated.solid.clone();
        actual.id = original.id;
        actual.editor = original.editor.clone();
        for side in &mut actual.sides {
            let (_, _, solid, original_side) = resolved
                .face_origin(side.id)
                .ok_or("Missing extracted face provenance")?;
            if solid != original.id {
                return Err("Extracted face changed owner".into());
            }
            side.id = original_side;
        }
        for (side, source) in actual.sides.iter_mut().zip(&original.sides) {
            let restored = compiler::map::native_side_projections(side, DVec3::ZERO)?;
            let expected = compiler::map::native_side_projections(source, DVec3::ZERO)?;
            if !restored
                .into_iter()
                .flatten()
                .flatten()
                .map(f32::to_bits)
                .eq(expected.into_iter().flatten().flatten().map(f32::to_bits))
            {
                return Err("Serialized child changed native texture or lightmap projections; extraction rejected".into());
            }
            side.u_axis.offset = source.u_axis.offset;
            side.v_axis.offset = source.v_axis.offset;
        }
        if actual != *original {
            return Err("Serialized child changed planes, UVs, materials or face metadata; extraction rejected".into());
        }
    }
    drop(resolved);
    plan.originals = Vec::new();
    reference
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    let (identity, staged) =
        FileIdentity::read_bounded(stage.path(), instances::MAX_FILE_BYTES as u64, control)
            .map_err(|error| error.to_string())?;
    if staged != input.as_bytes() {
        return Err("Staged child bytes changed".into());
    }
    stage.verify_owned()?;
    source.verify(control).map_err(|error| error.to_string())?;
    destination
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    Ok(Ready {
        plan,
        stage,
        identity,
        source,
        bytes: staged.len(),
        sha256: format!("{:x}", Sha256::digest(&staged)),
    })
}

impl Session {
    pub(super) fn shutdown_instance_extraction(&mut self) {
        if let Some(form) = self.instance_extraction.form.take() {
            let _ = form.control.cancel();
        }
    }

    fn instance_extraction_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.history_navigation_blocked().is_some()
            || self.property_draft.is_some()
            || self.rename_draft.is_some()
            || self.connection_draft.is_some()
        {
            return Err(
                "Finish the active build, launch or editing draft before extracting an instance"
                    .into(),
            );
        }
        Ok(())
    }

    fn extraction_form(&self, actor: Actor, token: u64) -> Result<&Form, String> {
        let form = self
            .instance_extraction
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Instance extraction request expired")?;
        if form.owner != actor {
            return Err("Review the shared extraction before changing it".into());
        }
        if !form.key.current(self) {
            return Err(
                "Map, selection, source or definitions changed; cancel and prepare again".into(),
            );
        }
        form.destination
            .verify_metadata()
            .map_err(|error| error.to_string())?;
        Ok(form)
    }

    pub fn begin_instance_extraction(
        &mut self,
        actor: Actor,
        file: &str,
    ) -> Result<Request, String> {
        self.begin_instance_extraction_with_pivot(actor, file, Pivot::WorldOrigin)
    }

    pub fn begin_instance_extraction_with_pivot(
        &mut self,
        actor: Actor,
        file: &str,
        pivot: Pivot,
    ) -> Result<Request, String> {
        self.instance_extraction_context()?;
        self.require_root_objects(&self.selected_ids())?;
        if self.instance_extraction.worker.busy() {
            return Err("Wait for the cancelled extraction worker to finish".into());
        }
        if self.instance_extraction.form.is_some() {
            return Err("Cancel the current extraction before preparing another".into());
        }
        let location = SourceLocation::capture(
            self.path
                .as_deref()
                .ok_or("Save the parent VMF before extracting an instance")?,
        )
        .map_err(|error| error.to_string())?;
        let destination = location
            .new_reference(file)
            .map_err(|error| error.to_string())?;
        let prefix = destination
            .relative()
            .rsplit_once('/')
            .map_or(0, |(directory, _)| directory.len() + 1);
        if prefix + STAGING_NAME_BYTES > instances::MAX_FIELD_BYTES {
            return Err("Child directory leaves insufficient space for the bounded private staging filename".into());
        }
        let plan = admission::plan(self, destination.relative(), pivot)?;
        let key = Key {
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            saved: self.history_save_serial,
            selection: self.selection.clone(),
            path: self.path.clone(),
        };
        self.instance_extraction.serial = self.instance_extraction.serial.wrapping_add(1);
        let token = self.instance_extraction.serial;
        let control = Control::default();
        let identity = Arc::new(());
        self.instance_extraction.form = Some(Form {
            identity: identity.clone(),
            token,
            owner: actor,
            key: key.clone(),
            control: control.clone(),
            status: "preparing",
            error: None,
            destination: destination.clone(),
            pivot: plan.pivot,
            pivot_mode: plan.pivot_mode,
            ready: None,
        });
        let worker = self
            .instance_extraction
            .worker
            .begin()
            .ok_or("Wait for the cancelled extraction worker to finish")?;
        Ok(Request {
            identity,
            worker,
            cleanup: self.instance_extraction.cleanup.clone(),
            token,
            key,
            control,
            destination,
            work: Work::Prepare {
                plan: Box::new(plan),
                location: Box::new(location),
            },
        })
    }

    pub fn get_instance_extraction(&self) -> Value {
        let cleanup = self
            .instance_extraction
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(form) = &self.instance_extraction.form else {
            return json!({"token":null,"status":"idle","worker_active":self.instance_extraction.worker.busy(),"last":self.instance_extraction.last,"cleanup_error":cleanup});
        };
        let error = self
            .instance_extraction_context()
            .err()
            .or_else(|| self.extraction_form(form.owner, form.token).err())
            .or_else(|| form.error.clone())
            .or_else(|| {
                form.ready.as_ref().and_then(|ready| {
                    ready
                        .identity
                        .verify_metadata()
                        .err()
                        .map(|error| error.to_string())
                })
            });
        let ready = form.ready.as_ref();
        json!({"cleanup_error":cleanup,"worker_active":self.instance_extraction.worker.busy(),"token":form.token,"owner":form.owner.label(),"status":if error.is_some(){"blocked"}else{form.status},"error":error,
            "selected":form.key.selection.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"file":form.destination.relative(),"source":form.key.path.as_ref().map(|path|path.display().to_string()),"pivot":form.pivot.to_array(),"pivot_mode":form.pivot_mode,
            "ready":form.status=="ready"&&error.is_none(),"world_brushes":ready.map(|ready|ready.plan.world_brushes),"detail_brushes":ready.map(|ready|ready.plan.detail_brushes),
            "objects":ready.map(|ready|ready.plan.selected.len()),"sides":ready.map(|ready|ready.plan.sides),"bytes":ready.map(|ready|ready.bytes),"sha256":ready.map(|ready|ready.sha256.as_str()),
            "contents_validation":if ready.is_some(){"passed"}else if form.status=="preparing"{"pending"}else{"unavailable"},
            "validation":"Preparation checks serialized child parsing and compiler expansion with exact original world-space geometry/axes/scales/metadata and bit-identical native texture/lightmap projections. Unrelated parent content is not certified.",
            "undo_policy":"Undo restores parent objects; the exported child remains on disk. Redo changes only the parent.",
            "limits":{"objects":MAX_OBJECTS,"brushes":MAX_BRUSHES,"sides":MAX_SIDES,"file_bytes":instances::MAX_FILE_BYTES,"retained_plan_bytes":MAX_PLAN_BYTES,"platform":"linux_dirfd","pivot":"world_origin_selection_centre_or_custom","pivot_changes":"cancel_and_prepare_again","roundtrip":"exact_geometry_and_native_uv_or_rejected"}})
    }

    pub fn review_instance_extraction(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let form = self
            .instance_extraction
            .form
            .as_mut()
            .filter(|form| form.token == token)
            .ok_or("Instance extraction request expired")?;
        if form.status != "ready" && form.status != "failed" {
            return Err("Wait for the extraction worker before reviewing ownership".into());
        }
        self.instance_extraction.serial = self.instance_extraction.serial.wrapping_add(1);
        form.token = self.instance_extraction.serial;
        form.owner = actor;
        Ok(self.get_instance_extraction())
    }

    pub fn cancel_instance_extraction(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        if self.instance_extraction.completed == Some((token, actor)) {
            return Ok(
                json!({"token":null,"status":"completed","last":self.instance_extraction.last}),
            );
        }
        let form = self
            .instance_extraction
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Instance extraction request expired")?;
        if form.owner != actor {
            return Err("Review the shared extraction before cancelling it".into());
        }
        form.control.cancel()?;
        self.instance_extraction.form = None;
        Ok(self.get_instance_extraction())
    }

    pub fn publish_instance_extraction(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Request, String> {
        let owned = self
            .instance_extraction
            .form
            .as_ref()
            .is_some_and(|form| form.token == token && form.owner == actor);
        if !owned {
            return Err("Instance extraction request expired or belongs to another actor".into());
        }
        if let Err(error) = self
            .instance_extraction_context()
            .and_then(|()| self.extraction_form(actor, token).map(|_| ()))
        {
            let form = self.instance_extraction.form.as_mut().unwrap();
            form.ready = None;
            form.status = "failed";
            form.error = Some(error.clone());
            return Err(error);
        }
        let worker = self
            .instance_extraction
            .worker
            .begin()
            .ok_or("Wait for the cancelled extraction worker to finish")?;
        let form = self.instance_extraction.form.as_mut().unwrap();
        let ready = form
            .ready
            .take()
            .ok_or("Prepare and validate the extraction first")?;
        form.status = "publishing";
        Ok(Request {
            worker,
            cleanup: self.instance_extraction.cleanup.clone(),
            identity: form.identity.clone(),
            token,
            key: form.key.clone(),
            control: form.control.clone(),
            destination: form.destination.clone(),
            work: Work::Publish(Box::new(ready)),
        })
    }

    pub fn finish_instance_extraction(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Some(form) = self.instance_extraction.form.as_ref().filter(|form| {
            form.token == completed.token
                && form.owner == actor
                && Arc::ptr_eq(&form.identity, &completed.identity)
        }) else {
            return Err("Instance extraction result expired".into());
        };
        let admission = self
            .instance_extraction_context()
            .and_then(|()| self.extraction_form(actor, completed.token).map(|_| ()))
            .and_then(|()| {
                if completed.key.current(self) && !form.control.cancelled() {
                    Ok(())
                } else {
                    Err("Instance extraction result is stale or cancelled".into())
                }
            });
        let result = admission.and(completed.result);
        let ready = match result {
            Ok(ready) => ready,
            Err(error) => {
                let form = self.instance_extraction.form.as_mut().unwrap();
                form.status = "failed";
                form.error = Some(error.clone());
                return Err(error);
            }
        };
        if !completed.publishing {
            let form = self.instance_extraction.form.as_mut().unwrap();
            form.status = "ready";
            form.ready = Some(ready);
            return Ok(self.get_instance_extraction());
        }
        let destination = form.destination.clone();
        let Ready {
            plan,
            stage,
            identity,
            source,
            bytes,
            sha256,
        } = ready;
        let path = destination.relative().to_owned();
        let removed = plan.selected.clone();
        let before = self.document.revision();
        let discarded = self
            .document
            .redo_steps()
            .iter()
            .map(|step| step.id())
            .collect::<Vec<_>>();
        let mut stage = Some(stage);
        let result = self.document.edit_with_finalizer(
            "extract selection to instance",
            |tx| {
                let mut helper = plan.helper;
                helper.id = tx.new_object_id()?;
                let mut allocator = Document::empty().into_vmf();
                allocator.world.id = helper.id;
                allocator.extra = plan.allocator_metadata;
                let mut allocator = Document::new(allocator);
                allocator.edit("check instance generated ID headroom", |probe| {
                    for _ in 0..plan.world_brushes + 2 * plan.detail_brushes {
                        probe.new_object_id()?;
                    }
                    Ok(())
                })?;
                for id in &plan.selected {
                    tx.remove(*id)?;
                }
                let retained_side_max = u64::from(tx.vmf().side_ids().max().unwrap_or(0));
                if retained_side_max + plan.sides as u64 > u64::from(u32::MAX) {
                    return Err(EditError::Invalid(
                        "Instance generated side IDs are exhausted".into(),
                    ));
                }
                let id = tx.add_entity(helper)?;
                if !matches!(id, ObjectId::Entity(_))
                    || !tx
                        .vmf()
                        .entities
                        .iter()
                        .any(|entity| ObjectId::Entity(entity.id) == id)
                {
                    return Err(EditError::Invalid(
                        "Extracted helper selection is unavailable".into(),
                    ));
                }
                Ok((id, BTreeSet::from([id])))
            },
            |_| {
                destination
                    .verify_metadata()
                    .map_err(|error| EditError::Invalid(error.to_string()))?;
                source
                    .verify_metadata()
                    .map_err(|error| EditError::Invalid(error.to_string()))?;
                identity
                    .verify_metadata()
                    .map_err(|error| EditError::Invalid(error.to_string()))?;
                stage.take().unwrap().publish(destination.path())?;
                Ok(())
            },
        );
        let (id, selection) = match result {
            Ok(result) => result,
            Err(error) => {
                let error = error.to_string();
                let form = self.instance_extraction.form.as_mut().unwrap();
                form.status = "failed";
                form.error = Some(error.clone());
                return Err(error);
            }
        };
        if self.document.revision() != before {
            for id in discarded {
                self.history_actors.remove(&id);
            }
            self.history_actors.insert(self.document.revision(), actor);
            self.selection = selection;
            self.face_selection.clear();
            self.changed();
            self.log(actor, "extract selection to instance");
        }
        self.instance_extraction.form = None;
        self.instance_extraction.completed = Some((completed.token, actor));
        self.instance_extraction.last = json!({"id":crate::identity::id_string(id),"file":path,"bytes":bytes,"sha256":sha256,"removed":removed.into_iter().map(crate::identity::id_string).collect::<Vec<_>>(),"world_brushes":plan.world_brushes,"detail_brushes":plan.detail_brushes,"sides":plan.sides,"pivot":plan.pivot.to_array(),"pivot_mode":plan.pivot_mode,"published":true,"undo_retains_child":true});
        Ok(self.get_instance_extraction())
    }
}

impl State {
    pub(super) fn close_work(&self) -> serde_json::Value {
        let cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        serde_json::json!({"sequence": self.serial, "worker_active": self.worker.busy(), "cleanup_error": cleanup})
    }

    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.form.as_ref().map(|form| {
            serde_json::json!([
                self.serial,
                form.token,
                form.owner.label(),
                form.status,
                form.control.cancelled(),
                form.error,
                form.ready.is_some()
            ])
        })
    }
}
