use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use compiler::sky_region;
use glam::DVec3;
use map_core::{ObjectId, query};
use render::sky3d;
use serde_json::{Value, json};
use source_formats::{
    bsp::surf,
    gamefs::GameFs,
    vmf::{Entity, Vmf},
};

use super::worker::{Slot, Ticket};
use super::{Actor, Session};

mod snapshot;

const MAX_CAMERAS: usize = 64;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;
const MAX_PROVENANCE_ROWS: usize = 64;
const AUXILIARY_BYTES: usize = 8 * 1024 * 1024;
const MAX_CPU_BYTES: usize = 488 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
struct Key {
    scope: String,
    generation: u64,
    revision: u64,
    materials: u64,
    content: u64,
    visibility: u64,
    camera: u32,
}

impl Key {
    fn capture(session: &Session, camera: u32) -> Self {
        Self {
            scope: session.buffer_review_scope.clone(),
            generation: session.document_generation,
            revision: session.document.revision(),
            materials: session.materials.generation(),
            content: session.content_epoch,
            visibility: session.working_region.serial,
            camera,
        }
    }

    fn current(&self, session: &Session) -> bool {
        session.sky3d_preview.camera == Some(self.camera)
            && self.scope == session.buffer_review_scope
            && self.generation == session.document_generation
            && self.revision == session.document.revision()
            && self.materials == session.materials.generation()
            && self.content == session.content_epoch
            && self.visibility == session.working_region.serial
    }

    fn json(&self) -> Value {
        json!({"generation":self.generation,"revision":self.revision,
            "material_generation":self.materials,"content_epoch":self.content,
            "visibility_revision":self.visibility,"camera_entity":format!("entity:{}",self.camera)})
    }
}

struct Pending {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: Slot,
    serial: u64,
    camera: Option<u32>,
    pending: Option<Pending>,
    ready: Option<Ready>,
    status: &'static str,
    issue: Option<String>,
}

struct Ready {
    key: Key,
    scene: Arc<sky3d::Scene>,
    stats: Value,
    snapshot_bytes: usize,
}

#[derive(Clone)]
struct Visibility {
    hidden: Arc<HashSet<ObjectId>>,
    region: Option<[DVec3; 2]>,
}

impl Visibility {
    fn included(
        &self,
        vmf: &Vmf,
        control: &compiler::Control,
    ) -> Result<HashSet<ObjectId>, String> {
        let mut included = HashSet::new();
        for solid in &vmf.world.solids {
            control.checkpoint()?;
            let id = ObjectId::Solid(solid.id);
            if !solid.hidden
                && !self.hidden.contains(&id)
                && self.visible_bounds(|| query::solid_bounds(solid))
            {
                included.insert(id);
            }
        }
        for entity in &vmf.entities {
            control.checkpoint()?;
            let id = ObjectId::Entity(entity.id);
            if entity.hidden || self.hidden.contains(&id) {
                continue;
            }
            let visible = entity.solids.iter().filter(|solid| !solid.hidden);
            if !entity.solids.is_empty() && visible.clone().next().is_none() {
                continue;
            }
            if self.visible_bounds(|| {
                let bounds = visible
                    .map(query::solid_bounds)
                    .collect::<Option<Vec<_>>>()?;
                query::union(bounds)
            }) {
                included.insert(id);
            }
        }
        Ok(included)
    }

    fn visible_bounds(&self, bounds: impl FnOnce() -> Option<(DVec3, DVec3)>) -> bool {
        self.region.is_none_or(|region| {
            bounds().is_none_or(|(mins, maxs)| {
                !mins.is_finite()
                    || !maxs.is_finite()
                    || (mins.cmple(region[1]).all() && maxs.cmpge(region[0]).all())
            })
        })
    }
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
    vmf: Vmf,
    game: Arc<GameFs>,
    visibility: Visibility,
    snapshot_bytes: usize,
    worker: Ticket,
}

pub struct Completed {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
    result: Result<Ready, String>,
    _worker: Ticket,
}

fn bounded(value: &str) -> String {
    map_core::entity_helpers::bounded_preview(value, MAX_DIAGNOSTIC_BYTES)
}

fn camera_row(entity: Option<&Entity>, id: u32) -> Value {
    let parsed = entity
        .ok_or_else(|| format!("entity:{id}: selected sky_camera is missing"))
        .and_then(sky_region::parse_camera);
    json!({"id":format!("entity:{id}"),
        "targetname":entity.and_then(|entity|entity.properties.get("targetname")).map(bounded),
        "ready":parsed.is_ok(),"origin":parsed.as_ref().ok().map(|camera|camera.origin.to_array()),
        "scale":parsed.as_ref().ok().map(|camera|camera.scale),"issue":parsed.err().map(|error|bounded(&error))})
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        self.run_with(Self::prepare)
    }

    fn run_with(self, prepare: impl FnOnce(&Self) -> Result<Ready, String>) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(&self)))
            .unwrap_or_else(|_| Err("3D sky preview worker panicked".into()));
        Completed {
            token: self.token,
            identity: self.identity,
            control: self.control,
            key: self.key,
            result,
            _worker: self.worker,
        }
    }

    fn prepare(&self) -> Result<Ready, String> {
        self.control.checkpoint()?;
        let reservation =
            sky_region::admit(&self.vmf, sky_region::Limits::default(), &self.control)?;
        let cpu_reservation = self
            .snapshot_bytes
            .checked_add(reservation.cpu_reservation_bytes)
            .and_then(|bytes| bytes.checked_add(sky3d::MAX_CPU_BYTES))
            .and_then(|bytes| bytes.checked_add(AUXILIARY_BYTES))
            .filter(|bytes| *bytes <= MAX_CPU_BYTES)
            .ok_or("3D sky preview exceeds its aggregate CPU allocation budget")?;
        let included = self.visibility.included(&self.vmf, &self.control)?;
        let region = sky_region::resolve_controlled(
            &self.vmf,
            &self.game,
            self.key.camera,
            |id| included.contains(&id),
            &self.control,
        )?;
        self.control.checkpoint()?;
        let mut vertices = 0usize;
        let mut indices = 0usize;
        let mut surfaces = 0usize;
        for surface in region
            .surfaces
            .iter()
            .filter(|surface| surface.flags & (surf::SKY | surf::SKY2D | surf::NODRAW) == 0)
        {
            self.control.checkpoint()?;
            vertices = vertices
                .checked_add(surface.vertices.len())
                .filter(|vertices| *vertices <= sky3d::MAX_VERTICES)
                .ok_or("3D sky preview exceeds its displayed vertex budget")?;
            let triangles = surface
                .vertices
                .len()
                .checked_sub(2)
                .ok_or("3D sky preview has an invalid clipped surface")?;
            indices = triangles
                .checked_mul(3)
                .and_then(|count| indices.checked_add(count))
                .filter(|indices| *indices <= sky3d::MAX_INDICES)
                .ok_or("3D sky preview exceeds its displayed index budget")?;
            surfaces = surfaces
                .checked_add(1)
                .filter(|surfaces| *surfaces <= sky3d::MAX_SURFACES)
                .ok_or("3D sky preview exceeds its displayed surface budget")?;
        }
        let stats = serde_json::to_value(&region.stats).map_err(|error| error.to_string())?;
        let sources = region
            .surfaces
            .iter()
            .filter(|surface| surface.flags & (surf::SKY | surf::SKY2D | surf::NODRAW) == 0)
            .flat_map(|surface| &surface.source_sides)
            .map(|source| (source.owner, source.solid, source.side))
            .collect::<BTreeSet<_>>();
        let owners = sources
            .iter()
            .map(|(owner, _, _)| *owner)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(crate::identity::id_string)
            .collect::<Vec<_>>();
        let provenance = sources
            .iter()
            .take(MAX_PROVENANCE_ROWS)
            .map(|(owner, solid, side)| {
                json!({"owner":crate::identity::id_string(*owner),"solid":solid,"side":side})
            })
            .collect::<Vec<_>>();
        let camera = sky3d::Camera {
            origin: region.origin,
            scale: f64::from(region.scale),
        };
        let surfaces = region
            .surfaces
            .into_iter()
            .filter(|surface| surface.flags & (surf::SKY | surf::SKY2D | surf::NODRAW) == 0)
            .map(|surface| sky3d::Surface {
                points: surface.vertices,
                normal: surface.normal,
                material: surface.material,
                texture_u: surface.texture_vectors[0].map(f64::from),
                texture_v: surface.texture_vectors[1].map(f64::from),
            })
            .collect::<Vec<_>>();
        let scene = sky3d::Scene::build_controlled(camera, &surfaces, &self.game, || {
            self.control.checkpoint()
        })?;
        self.control.checkpoint()?;
        let stats = json!({"region":stats,"geometry":scene.stats,"cpu_reservation_bytes":cpu_reservation,
            "source_owners":owners,"source_sides":provenance,"source_sides_total":sources.len(),
            "source_sides_truncated":sources.len()>MAX_PROVENANCE_ROWS});
        Ok(Ready {
            key: self.key.clone(),
            scene: Arc::new(scene),
            stats,
            snapshot_bytes: self.snapshot_bytes,
        })
    }
}

impl Session {
    pub fn set_sky3d_preview(
        &mut self,
        enabled: bool,
        camera: Option<u32>,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if enabled && camera.is_none() {
            return Err("Choose an existing sky_camera; no camera is selected".into());
        }
        if let Some(camera) = camera {
            sky_region::camera(self.document.vmf(), camera).map_err(|error| bounded(&error))?;
        }
        if self.sky3d_preview.camera != camera {
            self.retire_sky3d_preview("not_prepared", None);
            self.sky3d_preview.camera = camera;
        }
        if self.view.sky3d_preview != enabled {
            self.view.sky3d_preview = enabled;
            self.view_serial += 1;
        }
        Ok(self.sky3d_preview_state())
    }

    pub fn begin_sky3d_preview(&mut self, actor: Actor) -> Result<Request, String> {
        self.ensure_not_closing()?;
        self.preflight_sky3d_preview();
        if self.sky3d_preview.worker.busy() {
            return Err("Wait for the active 3D sky preview worker to finish".into());
        }
        let camera = self
            .sky3d_preview
            .camera
            .ok_or("Choose an existing sky_camera; no camera is selected")?;
        sky_region::camera(self.document.vmf(), camera).map_err(|error| bounded(&error))?;
        if let Some(error) = &self.game_error {
            let error = format!("Game content is unavailable: {}", bounded(error));
            self.retire_sky3d_preview("failed", Some(error.clone()));
            return Err(error);
        }
        let (vmf, snapshot_bytes) = match snapshot::capture(self.document.vmf()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.retire_sky3d_preview("failed", Some(error.clone()));
                return Err(error);
            }
        };
        let worker = self
            .sky3d_preview
            .worker
            .begin()
            .ok_or("Wait for the active 3D sky preview worker to finish")?;
        self.retire_sky3d_preview("loading", None);
        let token = self.sky3d_preview.serial;
        let identity = Arc::new(());
        let control = compiler::Control::default();
        let key = Key::capture(self, camera);
        self.sky3d_preview.pending = Some(Pending {
            token,
            identity: identity.clone(),
            control: control.clone(),
            key: key.clone(),
        });
        self.log(actor, "preparing 3D sky preview");
        Ok(Request {
            token,
            identity,
            control,
            key,
            vmf,
            game: self.game.clone(),
            visibility: Visibility {
                hidden: self.working_region.editor_hidden.clone(),
                region: self
                    .working_region
                    .enabled
                    .then_some(self.working_region.bounds)
                    .flatten(),
            },
            snapshot_bytes,
            worker,
        })
    }

    pub fn finish_sky3d_preview(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let matches = self.sky3d_preview.pending.as_ref().is_some_and(|pending| {
            pending.token == completed.token && Arc::ptr_eq(&pending.identity, &completed.identity)
        });
        if !matches {
            return Err("3D sky preview result belongs to an expired request".into());
        }
        self.sky3d_preview.pending = None;
        if self.closing || !completed.key.current(self) {
            self.retire_sky3d_preview(
                "stale",
                Some("Map, camera, materials or editor visibility changed; prepare again".into()),
            );
            return Err("3D sky preview result is stale; prepare again".into());
        }
        match completed.result {
            Ok(ready) if !completed.control.cancelled() => {
                self.sky3d_preview.ready = Some(ready);
                self.sky3d_preview.status = "ready";
                self.sky3d_preview.issue = None;
            }
            result => {
                self.sky3d_preview.ready = None;
                self.sky3d_preview.status = if completed.control.cancelled() {
                    "cancelled"
                } else {
                    "failed"
                };
                self.sky3d_preview.issue = Some(if completed.control.cancelled() {
                    "3D sky preview preparation cancelled".into()
                } else {
                    bounded(&result.err().unwrap_or_default())
                });
            }
        }
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(
            actor,
            format!("3D sky preview {}", self.sky3d_preview.status),
        );
        drop(completed._worker);
        Ok(self.sky3d_preview_state())
    }

    pub fn fail_sky3d_preview(&mut self, token: u64, error: String) -> Result<Value, String> {
        let pending = self
            .sky3d_preview
            .pending
            .as_ref()
            .filter(|pending| pending.token == token)
            .ok_or("3D sky preview request token is expired")?;
        let current = pending.key.current(self);
        self.retire_sky3d_preview(
            if current { "failed" } else { "stale" },
            Some(bounded(&error)),
        );
        Ok(self.sky3d_preview_state())
    }

    pub fn cancel_sky3d_preview(&mut self) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if let Some(pending) = &self.sky3d_preview.pending {
            pending.control.cancel()?;
        }
        self.sky3d_preview.ready = None;
        self.sky3d_preview.status = "cancelled";
        self.sky3d_preview.issue = Some("3D sky preview preparation cancelled".into());
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.sky3d_preview_state())
    }

    pub fn preflight_sky3d_preview(&mut self) -> bool {
        let stale = self
            .sky3d_preview
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.key.current(self))
            || self
                .sky3d_preview
                .ready
                .as_ref()
                .is_some_and(|ready| !ready.key.current(self));
        if stale {
            self.retire_sky3d_preview(
                "stale",
                Some("Map, camera, materials or editor visibility changed; prepare again".into()),
            );
            return true;
        }
        if self.sky3d_preview.pending.is_some() && !self.sky3d_preview.worker.busy() {
            self.retire_sky3d_preview(
                "cancelled",
                Some("3D sky preview request was dropped before completion".into()),
            );
            return true;
        }
        false
    }

    pub(super) fn sky3d_preview_scene(&mut self) -> Option<Arc<sky3d::Scene>> {
        self.preflight_sky3d_preview();
        self.sky3d_preview
            .ready
            .as_ref()
            .map(|ready| ready.scene.clone())
    }

    pub fn sky3d_preview_state(&mut self) -> Value {
        self.sky3d_preview_state_for(self.view(), self.camera())
    }

    pub fn sky3d_preview_state_for(
        &mut self,
        view: render::ViewOptions,
        camera: render::Camera,
    ) -> Value {
        self.preflight_sky3d_preview();
        let selected = self.sky3d_preview.camera.map(|id| {
            self.document
                .entity(id)
                .ok_or_else(|| format!("entity:{id}: selected sky_camera is missing"))
                .and_then(sky_region::parse_camera)
        });
        let issue = selected
            .as_ref()
            .and_then(|camera| camera.as_ref().err().map(|error| bounded(error)))
            .or_else(|| self.sky3d_preview.issue.clone());
        let ready = self.sky3d_preview.ready.as_ref();
        let suppression = if !view.sky3d_preview {
            Some("disabled")
        } else if camera.projection != render::Projection::Perspective
            || view.surface != render::SurfaceView::Textured
        {
            Some("unsupported_view")
        } else if ready.is_none() {
            Some("not_prepared")
        } else {
            None
        };
        let mut cameras = self.document.vmf().entities.iter().filter(|entity| {
            entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("sky_camera"))
        });
        let rows = cameras
            .by_ref()
            .take(MAX_CAMERAS)
            .map(|entity| camera_row(Some(entity), entity.id))
            .collect::<Vec<_>>();
        let cameras_truncated = cameras.next().is_some();
        let status = if self.sky3d_preview.status.is_empty() {
            "not_prepared"
        } else {
            self.sky3d_preview.status
        };
        let summary = issue.clone().unwrap_or_else(|| {
            match status {
                "ready" if !view.sky3d_preview => "3D sky preview prepared; display is off",
                "ready" => "3D sky preview ready · perspective Textured view",
                "loading" => "Preparing 3D sky preview…",
                "cancelled" => "3D sky preview preparation cancelled",
                "stale" => "3D sky preview changed; prepare again",
                "failed" => "3D sky preview preparation failed",
                _ if self.sky3d_preview.camera.is_none() => "Choose an existing sky_camera",
                _ => "Prepare the selected sky_camera to display its miniature",
            }
            .into()
        });
        let pending = self.sky3d_preview.pending.as_ref();
        json!({"snapshot":format!("{}:{}:{}:{}:{}:{}",self.document_generation,self.document.revision(),self.materials.generation(),self.content_epoch,self.working_region.serial,self.sky3d_preview.serial),
            "token":self.sky3d_preview.serial,"enabled":self.view.sky3d_preview,
            "requested":view.sky3d_preview,
            "camera_entity":self.sky3d_preview.camera.map(|id|format!("entity:{id}")),
            "selected_camera":self.sky3d_preview.camera.map(|id|camera_row(self.document.entity(id),id)),
            "cameras":rows,"cameras_truncated":cameras_truncated,"status":status,
            "ready":ready.is_some(),"loading":pending.is_some(),"worker_active":self.sky3d_preview.worker.busy(),
            "active":suppression.is_none(),"eligible":suppression.is_none(),"suppression_reason":suppression,
            "cancellable":pending.is_some_and(|pending|!pending.control.cancelled()),
            "stage":pending.map(|pending|pending.control.progress().stage.name()),
            "source":ready.map(|ready|ready.key.json()),"stats":ready.map(|ready|&ready.stats),
            "snapshot_bytes":ready.map(|ready|ready.snapshot_bytes),"issue":issue,"summary":summary,
            "diagnostics":issue.as_ref().map(|issue|vec![json!({"level":"error","message":bounded(issue)})]).unwrap_or_default(),
            "limits":{"region":sky_region::Limits::default(),"material_semantics":{"file_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_BYTES,"total_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_TOTAL_BYTES,"reads":compiler::materials::MAX_GEOMETRY_MATERIAL_READS,"allocation_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES},"render":{"vertices":sky3d::MAX_VERTICES,"indices":sky3d::MAX_INDICES,"surfaces":sky3d::MAX_SURFACES,"materials":sky3d::MAX_MATERIALS,"cpu_bytes":sky3d::MAX_CPU_BYTES,"gpu_bytes":sky3d::MAX_GPU_BYTES,"gpu_asset_bytes":sky3d::MAX_GPU_ASSET_BYTES,"gpu_upload_bytes":sky3d::MAX_GPU_UPLOAD_BYTES,"target_bytes":sky3d::MAX_TARGET_BYTES,"target_dimension":sky3d::MAX_TARGET_DIMENSION},"cpu_bytes":MAX_CPU_BYTES,"auxiliary_bytes":AUXILIARY_BYTES,"snapshot_bytes":snapshot::MAX_BYTES,"snapshot_items":snapshot::MAX_ITEMS,"snapshot_depth":snapshot::MAX_DEPTH,"camera_rows":MAX_CAMERAS,"provenance_rows":MAX_PROVENANCE_ROWS,"active_workers":1,"cached_snapshots":1},
            "policy":"Explicit root sky_camera; sealed committed flat world/detail component; editor visibility filters display fragments after topology. Static opaque textured miniature only; no sky fog, models, terrain, portals, water, instances or runtime effects. Main picking, source, selection, history and camera remain ordinary editor state. 2D sky preview is independent. No native parity claim."})
    }

    fn retire_sky3d_preview(&mut self, status: &'static str, issue: Option<String>) {
        self.retire_sky3d_preview_work(status, issue);
        self.scene_serial += 1;
        self.view_serial += 1;
    }

    fn retire_sky3d_preview_work(&mut self, status: &'static str, issue: Option<String>) {
        if let Some(pending) = self.sky3d_preview.pending.take() {
            let _ = pending.control.cancel();
        }
        self.sky3d_preview.serial = self.sky3d_preview.serial.wrapping_add(1);
        self.sky3d_preview.ready = None;
        self.sky3d_preview.status = status;
        self.sky3d_preview.issue = issue;
    }

    pub(super) fn reset_sky3d_preview(&mut self) {
        self.retire_sky3d_preview("not_prepared", None);
        self.sky3d_preview.camera = None;
        self.view.sky3d_preview = false;
    }

    pub(super) fn shutdown_sky3d_preview(&mut self) {
        if self.sky3d_preview.pending.is_some() || self.sky3d_preview.ready.is_some() {
            self.retire_sky3d_preview_work("cancelled", Some("The editor is closing".into()));
        }
    }
}

impl State {
    pub(super) fn close_work(&self) -> Value {
        json!({"sequence":self.serial,"status":self.status,"worker_active":self.worker.busy(),
            "request":self.pending.as_ref().map(|pending|json!({"token":pending.token,"cancelled":pending.control.cancelled()}))})
    }
}

#[cfg(test)]
mod tests;
