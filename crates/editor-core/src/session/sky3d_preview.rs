use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use compiler::sky_region;
use map_core::ObjectId;
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
    fog_defaults: Vec<&'static str>,
    fog_inert_keys: Vec<String>,
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
    vmf: Vmf,
    game: Arc<GameFs>,
    included: HashSet<ObjectId>,
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

fn parse_camera(
    entity: &Entity,
) -> Result<(sky_region::Camera, map_core::fog::ParsedController), String> {
    Ok((
        sky_region::parse_camera(entity)?,
        map_core::fog::parse_sky_camera(entity)?,
    ))
}

fn received_origin(origin: glam::DVec3) -> glam::DVec3 {
    glam::Vec3::from_array(
        origin
            .as_vec3()
            .to_array()
            .map(map_core::fog::received_coordinate),
    )
    .as_dvec3()
}

fn camera_row(entity: Option<&Entity>, id: u32) -> Value {
    let parsed = entity
        .ok_or_else(|| format!("entity:{id}: selected sky_camera is missing"))
        .and_then(parse_camera);
    json!({"id":format!("entity:{id}"),
        "targetname":entity.and_then(|entity|entity.properties.get("targetname")).map(bounded),
        "ready":parsed.is_ok(),"origin":parsed.as_ref().ok().map(|(camera,_)|camera.origin.to_array()),
        "received_origin":parsed.as_ref().ok().map(|(camera,_)|received_origin(camera.origin).to_array()),
        "scale":parsed.as_ref().ok().map(|(camera,_)|camera.scale),
        "fog":parsed.as_ref().ok().map(|(_,fog)|super::fog::profile_report(fog.profile)),
        "fog_defaults":parsed.as_ref().ok().map(|(_,fog)|&fog.defaults),
        "fog_inert_keys":parsed.as_ref().ok().map(|(_,fog)|&fog.inert_keys),
        "issue":parsed.err().map(|error|bounded(&error))})
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
        let entity = self
            .vmf
            .entities
            .iter()
            .find(|entity| entity.id == self.key.camera)
            .ok_or("Selected sky_camera is missing from the frozen source")?;
        let fog = map_core::fog::parse_sky_camera(entity)?;
        let reservation =
            sky_region::admit(&self.vmf, sky_region::Limits::default(), &self.control)?;
        let cpu_reservation = self
            .snapshot_bytes
            .checked_add(reservation.cpu_reservation_bytes)
            .and_then(|bytes| bytes.checked_add(sky3d::MAX_CPU_BYTES))
            .and_then(|bytes| bytes.checked_add(AUXILIARY_BYTES))
            .filter(|bytes| *bytes <= MAX_CPU_BYTES)
            .ok_or("3D sky preview exceeds its aggregate CPU allocation budget")?;
        let region = sky_region::resolve_controlled(
            &self.vmf,
            &self.game,
            self.key.camera,
            |id| self.included.contains(&id),
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
            origin: received_origin(region.origin),
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
        let scene =
            sky3d::Scene::build_controlled(camera, fog.profile, &surfaces, &self.game, || {
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
            fog_defaults: fog.defaults,
            fog_inert_keys: fog.inert_keys,
        })
    }
}

impl Session {
    pub(super) fn sky3d_fog_pass_snapshot(&self) -> String {
        format!(
            "{}:{:?}:{}:{}",
            self.sky3d_preview.serial,
            self.sky3d_preview.camera,
            self.fog_preview_settings().initial_radial_context.name(),
            self.sky3d_preview
                .camera
                .map(|id| Key::capture(self, id).json())
                .unwrap_or(Value::Null)
        )
    }

    pub(super) fn sky3d_fog_pass_source(
        &self,
    ) -> Result<(u32, map_core::fog::Profile, Value), String> {
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Sky fog setup requires the committed document; leave the temporary document preview first".into());
        }
        let id = self
            .sky3d_preview
            .camera
            .ok_or("Choose an existing sky_camera before recording Sky fog")?;
        if self
            .sky3d_preview
            .ready
            .as_ref()
            .is_some_and(|ready| !ready.key.current(self))
        {
            return Err(
                "The selected 3D sky source changed; prepare it again before recording Sky fog"
                    .into(),
            );
        }
        let entity = self
            .document
            .entity(id)
            .ok_or("Selected sky_camera is missing")?;
        let (_, parsed) = parse_camera(entity)?;
        let context = self.fog_preview_settings().initial_radial_context;
        let profile = context.initial_profile(parsed.profile);
        profile.validate()?;
        Ok((
            id,
            profile,
            json!({"kind":"selected_sky_camera","identity":Key::capture(self,id).json(),
            "selection_serial":self.sky3d_preview.serial,"initial_radial_context":context.name(),"initial_radial_context_label":context.label(),
            "profile":super::fog::profile_report(profile)}),
        ))
    }
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
            let entity = self
                .document
                .entity(camera)
                .ok_or("Selected sky_camera is missing")?;
            map_core::fog::parse_sky_camera(entity).map_err(|error| bounded(&error))?;
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
            included: self.working_evaluation(&self.document).objects.clone(),
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

    fn sky3d_fog_report(
        &mut self,
        view: render::ViewOptions,
        camera: render::Camera,
    ) -> Result<Value, String> {
        let context = view.sky_fog_initial_radial_context;
        let prepared = self.sky3d_preview.ready.is_some();
        let (raw, sky_camera, defaults, inert_keys) = if let Some(ready) = &self.sky3d_preview.ready
        {
            (
                ready.scene.fog,
                ready.scene.camera,
                json!(ready.fog_defaults),
                json!(ready.fog_inert_keys),
            )
        } else if let Some(id) = self.sky3d_preview.camera {
            let entity = self
                .document
                .entity(id)
                .ok_or("Selected sky_camera is missing")?;
            let (camera, fog) = parse_camera(entity)?;
            (
                fog.profile,
                sky3d::Camera {
                    origin: received_origin(camera.origin),
                    scale: f64::from(camera.scale),
                },
                json!(fog.defaults),
                json!(fog.inert_keys),
            )
        } else {
            return Ok(
                json!({"ready":false,"applied":false,"initial_radial_context":context.name(),"suppression_reason":"no_camera"}),
            );
        };
        let initial = context.initial_profile(raw);
        let fog = render::fog::sky_view(raw, sky_camera.scale as f32, context)?;
        let miniature = (camera.projection == render::Projection::Perspective)
            .then(|| sky_camera.transform(&camera))
            .transpose()?;
        let requested = view.sky3d_active(&camera) && raw.enabled;
        let colors_active = requested && fog.max_density() > 0.0;
        let compatibility = if prepared && colors_active && view.sky_preview {
            self.skybox_preview_for(view)
                .map(|sky| sky.fog_compatibility())
                .transpose()
        } else {
            Ok(None)
        };
        let issue = compatibility.err();
        Ok(
            json!({"ready":prepared && issue.is_none(),"enabled":raw.enabled,"requested":requested,
            "applied":prepared && colors_active && issue.is_none(),"issue":issue,
            "initial_radial_context":context.name(),"authored":super::fog::profile_report(raw),
            "initial":super::fog::profile_report(initial),"effective":super::fog::profile_report(fog.controller_profile().ok_or("Sky fog invariant failed: controller profile is absent")?),
            "scale":sky_camera.scale,"received_origin":sky_camera.origin.to_array(),
            "coordinate_policy":"Native SPROP_COORD float32 conversion with a 1/32 grid, 14-bit integer wrapping and canonical positive zero; raw source origin and direction remain inspectable.",
            "color_linear":miniature.map(|camera|fog.color_for_camera(&camera)),
            "defaults":defaults,"inert_keys":inert_keys,
            "policy":"Static selected sky_camera fog applies automatically with a ready enabled 3D sky preview in Textured perspective, independently of world Fog preview, controller, water and timeline. Current initial radial context applies before ranges are divided by sky scale once. Standard Sky backgrounds remain unchanged; combined UnlitGeneric backgrounds require explicit nofog. No sky farz, controller inputs or HDR fog display."}),
        )
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
                .and_then(parse_camera)
        });
        let fog = self.sky3d_fog_report(view, camera);
        let fog_issue = fog.as_ref().err().cloned().or_else(|| {
            fog.as_ref()
                .ok()
                .and_then(|fog| fog["issue"].as_str().map(str::to_owned))
        });
        let issue = selected
            .as_ref()
            .and_then(|camera| camera.as_ref().err().map(|error| bounded(error)))
            .or_else(|| self.sky3d_preview.issue.clone())
            .or_else(|| fog_issue.clone());
        let ready = self.sky3d_preview.ready.as_ref();
        let suppression = if !view.sky3d_preview {
            Some("disabled")
        } else if camera.projection != render::Projection::Perspective
            || view.surface != render::SurfaceView::Textured
        {
            Some("unsupported_view")
        } else if ready.is_none() {
            Some("not_prepared")
        } else if fog_issue.is_some() {
            Some("unsupported_sky_fog")
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
            "ready":ready.is_some() && fog_issue.is_none(),"prepared":ready.is_some(),"loading":pending.is_some(),"worker_active":self.sky3d_preview.worker.busy(),
            "active":suppression.is_none(),"eligible":suppression.is_none(),"suppression_reason":suppression,
            "cancellable":pending.is_some_and(|pending|!pending.control.cancelled()),
            "stage":pending.map(|pending|pending.control.progress().stage.name()),
            "source":ready.map(|ready|ready.key.json()),"stats":ready.map(|ready|&ready.stats),
            "snapshot_bytes":ready.map(|ready|ready.snapshot_bytes),"issue":issue,"summary":summary,
            "fog":fog.unwrap_or_else(|error|json!({"ready":false,"applied":false,"issue":bounded(&error)})),
            "diagnostics":issue.as_ref().map(|issue|vec![json!({"level":"error","message":bounded(issue)})]).unwrap_or_default(),
            "limits":{"region":sky_region::Limits::default(),"material_semantics":{"file_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_BYTES,"total_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_TOTAL_BYTES,"reads":compiler::materials::MAX_GEOMETRY_MATERIAL_READS,"allocation_bytes":compiler::materials::MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES},"render":{"vertices":sky3d::MAX_VERTICES,"indices":sky3d::MAX_INDICES,"surfaces":sky3d::MAX_SURFACES,"materials":sky3d::MAX_MATERIALS,"cpu_bytes":sky3d::MAX_CPU_BYTES,"gpu_bytes":sky3d::MAX_GPU_BYTES,"gpu_asset_bytes":sky3d::MAX_GPU_ASSET_BYTES,"gpu_upload_bytes":sky3d::MAX_GPU_UPLOAD_BYTES,"target_bytes":sky3d::MAX_TARGET_BYTES,"target_dimension":sky3d::MAX_TARGET_DIMENSION},"cpu_bytes":MAX_CPU_BYTES,"auxiliary_bytes":AUXILIARY_BYTES,"snapshot_bytes":snapshot::MAX_BYTES,"snapshot_items":snapshot::MAX_ITEMS,"snapshot_depth":snapshot::MAX_DEPTH,"camera_rows":MAX_CAMERAS,"provenance_rows":MAX_PROVENANCE_ROWS,"active_workers":1,"cached_snapshots":1},
            "policy":"Explicit root sky_camera; sealed committed flat world/detail component; editor visibility filters display fragments after topology. Static opaque textured miniature with its independent static camera fog; no models, terrain, portals, water, instances or runtime effects. Main picking, source, selection, history and camera remain ordinary editor state. 2D sky preview is independent, with combined fog material compatibility required. No native framebuffer parity claim."})
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
