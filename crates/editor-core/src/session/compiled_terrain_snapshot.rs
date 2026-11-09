use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

use compiler::Control;
use glam::{DVec3, Vec3};
use native_runtime::{CaptureObserver, CleanupDisposition};
use render::{SceneMesh, ViewOptions};
use serde_json::{Value, json};
use source_formats::bsp::native_terrain::{
    NativeTerrainDisplacement, NativeTerrainSample, NativeTerrainSnapshot,
};

use super::native_observation::{Frame, MAX_METADATA_BYTES};
use super::{Actor, Session, baked_build, inspection_framing, native_observation, worker};
use crate::runtime::{SharedSession, lock, native_activity};

mod comparison;
pub use comparison::{ComparisonCompleted, ComparisonOptions, ComparisonRequest, Rounding};

pub const MAX_PAGE_ROWS: usize = 64;
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
pub const MAX_SOURCE_BYTES: usize = 128 * 1024;
pub const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MARKER_HALF_EXTENT: f32 = 4.0;
const VECTOR_LENGTH: f32 = 16.0;
const MARKER_COLOR: [u8; 4] = [255, 220, 90, 255];
const NORMAL_COLOR: [u8; 4] = [80, 220, 255, 255];
const TANGENT_S_COLOR: [u8; 4] = [255, 120, 100, 255];
const TANGENT_T_COLOR: [u8; 4] = [120, 255, 150, 255];
const POLICY: &str = "Complete final terrain position/N/S/T fields observed in one private owned native client after verified mesh publication. Identity is the owned build, native profile, displacement ordinal and native row-major grid index. Contributor labels describe compiled provenance; they do not map samples to authored vertices. Historical rows retain captured bits. Frame and guide require the current build and runtime, no displayed draft and a finite observed position. The guide draws a point cross and normalizes available directions for fixed-length presentation only. Missing or degenerate vectors get no substitute. Display filters apply after complete observation. No native shader, VRAD, pixel or runtime LOD-frame agreement is claimed.";
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: worker::Slot,
    render_serial: u64,
    token: Option<u64>,
    pending: Option<Pending>,
    ready: Option<Arc<Ready>>,
    guide: Option<Guide>,
    visible: bool,
    status: &'static str,
    error: Option<String>,
    presentation_key: Option<(bool, Option<String>)>,
    comparison: comparison::State,
}

struct Pending {
    identity: Arc<()>,
    control: Control,
}

struct Ready {
    frame: Frame,
    build: Arc<baked_build::RetainedBuild>,
    target: native_activity::Target,
    snapshot: NativeTerrainSnapshot,
    profile: String,
    observation: String,
    artifacts: PathBuf,
    elapsed_ms: u64,
    retained_bytes: usize,
}

impl Ready {
    fn observation(&self) -> Value {
        serde_json::from_str(&self.observation)
            .expect("A retained native observation was serialized as JSON")
    }

    fn current(&self, session: &Session) -> Result<(), String> {
        native_observation::verify(
            &self.frame,
            &self.build,
            &self.target,
            session,
            &Control::default(),
        )
    }

    fn displacement(&self, ordinal: u16) -> Result<&NativeTerrainDisplacement, String> {
        self.snapshot
            .displacements()
            .get(usize::from(ordinal))
            .filter(|displacement| displacement.ordinal() == ordinal)
            .ok_or_else(|| "Native displacement ordinal is absent from this snapshot".into())
    }

    fn sample(&self, ordinal: u16, vertex: usize) -> Result<NativeTerrainSample, String> {
        self.displacement(ordinal)?
            .samples()
            .get(vertex)
            .copied()
            .ok_or_else(|| "Native grid index is absent from this displacement".into())
    }
}

#[derive(Clone, Copy)]
struct Guide {
    displacement: u16,
    vertex: usize,
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    control: Control,
    frame: Frame,
    build: Arc<baked_build::RetainedBuild>,
    target: native_activity::Target,
    environment: native_runtime::CaptureEnvironment,
    permit: Option<native_activity::Permit>,
    cleanup: CleanupDisposition,
    _worker: worker::Ticket,
}

pub struct Completed {
    request: Request,
    result: Result<Ready, String>,
}

struct Observer<'a> {
    request: &'a Request,
    session: &'a SharedSession,
}

impl CaptureObserver for Observer<'_> {
    fn checkpoint(&mut self) -> std::io::Result<()> {
        self.request
            .checkpoint(self.session)
            .map_err(std::io::Error::other)
    }

    fn sample_progress(&mut self, _current: usize, _total: usize) {}
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    fn checkpoint(&self, session: &SharedSession) -> Result<(), String> {
        native_observation::verify(
            &self.frame,
            &self.build,
            &self.target,
            &lock(session),
            &self.control,
        )
    }

    pub fn run(mut self, session: &SharedSession) -> Completed {
        let result = self.execute(session);
        Completed {
            request: self,
            result,
        }
    }

    fn execute(&mut self, session: &SharedSession) -> Result<Ready, String> {
        self.checkpoint(session)?;
        self.build.verify(&self.control, MAX_METADATA_BYTES)?;
        let bytes = baked_build::read_owned_bytes_controlled(&self.build.output, &self.control)?;
        let decoded_bytes = baked_build::decoded_bsp_bound(&bytes)?;
        if decoded_bytes > baked_build::MAX_BSP_BYTES as usize {
            return Err("Decoded terrain carrier exceeds the owned BSP budget".into());
        }
        let bsp = source_formats::bsp::Bsp::parse(&bytes).map_err(|error| error.to_string())?;
        self.build
            .origins
            .validate_channel(&bsp, source_formats::bsp::LightingKind::Ldr)?;
        drop(bsp);
        self.checkpoint(session)?;
        let artifacts_parent = crate::recovery::default_directory()?.join("native-terrain");
        std::fs::create_dir_all(&artifacts_parent).map_err(|error| error.to_string())?;
        let request = native_runtime::TerrainSnapshotRequest {
            environment: self.environment.clone(),
            bsp: bytes,
            map_name: "hammerr_terrain_snapshot".into(),
            channel: native_runtime::CaptureChannel::Ldr,
            artifacts_parent,
        };
        if let Some(permit) = &mut self.permit {
            permit.started();
        }
        self.cleanup = CleanupDisposition::Unconfirmed;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_runtime::observe_terrain_snapshot_with_cleanup(
                request,
                &mut Observer {
                    request: self,
                    session,
                },
            )
        }));
        let receipt = match result {
            Ok(Ok(receipt)) => {
                self.cleanup = CleanupDisposition::Confirmed;
                if let Some(permit) = &mut self.permit {
                    permit.confirmed_clean();
                }
                receipt
            }
            Ok(Err(error)) => {
                self.cleanup = error.cleanup;
                if let Some(permit) = &mut self.permit {
                    match error.cleanup {
                        CleanupDisposition::Confirmed => permit.confirmed_clean(),
                        CleanupDisposition::Unconfirmed => {
                            permit.failed_cleanup(&error.to_string())
                        }
                    }
                }
                return Err(error.to_string());
            }
            Err(_) => {
                if let Some(permit) = &mut self.permit {
                    permit.failed_cleanup(
                        "Native terrain observation panicked; cleanup is unconfirmed",
                    );
                }
                return Err("Native terrain observation panicked; cleanup is unconfirmed".into());
            }
        };
        if !receipt.preservation_passed {
            return Err(
                "Native terrain observation did not confirm preservation of existing files".into(),
            );
        }
        if !receipt.observation.resumed {
            return Err("Native terrain observation did not confirm thread resume".into());
        }
        if Some(receipt.observation.bsp_sha256.as_str()) != self.build.output.sha256() {
            return Err(
                "Native terrain observation belongs to different compiled BSP bytes".into(),
            );
        }
        if Some(receipt.observation.engine_sha256.as_str()) != self.target.engine_image_sha256() {
            return Err("Native terrain observation belongs to a different engine image".into());
        }
        self.checkpoint(session)?;
        self.build.verify(&self.control, MAX_METADATA_BYTES)?;
        let profile = receipt.observation.profile.clone();
        if profile.is_empty() || profile.len() > 512 {
            return Err("Native observation has no bounded profile identity".into());
        }
        let observation =
            serde_json::to_string(&receipt.observation).map_err(|error| error.to_string())?;
        if observation.len() > MAX_OBSERVATION_BYTES {
            return Err("Native observation receipt exceeds its metadata budget".into());
        }
        crate::report_path::checked(&receipt.artifacts)?;
        let retained_bytes = receipt
            .snapshot
            .retained_bytes()
            .checked_add(observation.capacity())
            .and_then(|bytes| bytes.checked_add(profile.capacity()))
            .and_then(|bytes| bytes.checked_add(receipt.artifacts.capacity()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Ready>()))
            .filter(|bytes| *bytes <= MAX_RETAINED_BYTES)
            .ok_or("Native terrain snapshot exceeds its retained-result budget")?;
        self.checkpoint(session)?;
        Ok(Ready {
            frame: self.frame.clone(),
            build: self.build.clone(),
            target: self.target.clone(),
            snapshot: receipt.snapshot,
            profile,
            observation,
            artifacts: receipt.artifacts,
            elapsed_ms: receipt.elapsed_ms,
            retained_bytes,
        })
    }
}

#[derive(Clone)]
pub struct Query {
    pub token: u64,
    pub displacement: Option<u16>,
    pub offset: usize,
    pub limit: usize,
    pub source_offset: usize,
    pub source_limit: usize,
}

pub struct RenderGuard {
    token: Option<u64>,
    serial: u64,
    ready: Option<Weak<Ready>>,
    displayed: bool,
}

fn vector_values(bits: [u32; 3]) -> [Option<f32>; 3] {
    bits.map(|bits| {
        let value = f32::from_bits(bits);
        value.is_finite().then_some(value)
    })
}

fn vector_direction(bits: [u32; 3]) -> Option<Vec3> {
    let vector = DVec3::from_array(bits.map(|bits| f64::from(f32::from_bits(bits))));
    if !vector.is_finite() || vector == DVec3::ZERO {
        return None;
    }
    Some((vector / vector.length()).as_vec3())
}

fn vector_report(bits: [u32; 3]) -> Value {
    let values = vector_values(bits);
    let finite = values.iter().all(Option::is_some);
    let vector = DVec3::from_array(bits.map(|bits| f64::from(f32::from_bits(bits))));
    json!({"values":values,"bits":bits.map(|bits|format!("0x{bits:08x}")),
        "state":if !finite {"nonfinite"} else if vector == DVec3::ZERO {"zero"} else {"finite_nonzero"},
        "length":finite.then(||vector.length())})
}

fn sample_position_available(sample: NativeTerrainSample) -> bool {
    sample
        .position_bits()
        .iter()
        .all(|bits| f32::from_bits(*bits).is_finite())
}

fn bounded_report(report: Value) -> Result<Value, String> {
    let bytes = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PAGE_BYTES {
        return Err(
            "Native terrain inspection exceeds 256 KiB; request fewer rows or contributors".into(),
        );
    }
    Ok(report)
}

impl Session {
    fn terrain_snapshot_stamp(&self) -> String {
        format!(
            "terrain-observation-v1:{}:{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.content_epoch,
            self.compiled
                .as_ref()
                .map_or(0, |build| build.lightmap_identity.token)
        )
    }

    fn check_terrain_snapshot_token(&self, token: u64) -> Result<(), String> {
        if self.compiled_terrain_snapshot.token != Some(token) {
            return Err("Native terrain snapshot token is stale or foreign".into());
        }
        Ok(())
    }

    fn terrain_snapshot_display_error(&self, ready: &Ready) -> Option<String> {
        ready.current(self).err().or_else(|| {
            if !std::ptr::eq(self.render_document(), self.document()) {
                Some(
                    "Finish the displayed geometry draft before showing a native terrain guide"
                        .into(),
                )
            } else {
                self.inspection_viewport_ready("showing a native terrain guide")
                    .err()
            }
        })
    }

    fn terrain_guide_region_visible(&self, sample: NativeTerrainSample) -> bool {
        let point = DVec3::from_array(
            sample
                .position_bits()
                .map(|bits| f64::from(f32::from_bits(bits))),
        );
        self.working_region_bounds_visible(Some([point, point]))
    }

    fn terrain_guide_error(&self) -> Option<String> {
        let state = &self.compiled_terrain_snapshot;
        let ready = state.ready.as_ref()?;
        let guide = state.guide?;
        self.terrain_snapshot_display_error(ready)
            .or_else(|| ready.sample(guide.displacement, guide.vertex).err())
            .or_else(|| {
                ready
                    .sample(guide.displacement, guide.vertex)
                    .ok()
                    .and_then(|sample| {
                        if !sample_position_available(sample) {
                            Some("The sample has no finite observed position".into())
                        } else if !self.terrain_guide_region_visible(sample) {
                            Some("The native sample is outside the working region".into())
                        } else {
                            None
                        }
                    })
            })
    }

    fn terrain_guide_report(&self) -> Value {
        let state = &self.compiled_terrain_snapshot;
        let error = self.terrain_guide_error();
        let requested = state.guide.is_some();
        let displayed = requested && state.visible && error.is_none();
        let sample = state
            .ready
            .as_ref()
            .zip(state.guide)
            .and_then(|(ready, guide)| ready.sample(guide.displacement, guide.vertex).ok());
        let face = state
            .ready
            .as_ref()
            .zip(state.guide)
            .and_then(|(ready, guide)| {
                ready
                    .displacement(guide.displacement)
                    .ok()
                    .map(NativeTerrainDisplacement::face)
            });
        json!({"requested":requested,"visible":state.visible,"displayed":displayed,
            "token":state.token.map(|token|token.to_string()),"build_token":state.ready.as_ref().map(|ready|ready.build.token.to_string()),
            "profile":state.ready.as_ref().map(|ready|&ready.profile),"face":face,
            "displacement":state.guide.map(|guide|guide.displacement),"vertex":state.guide.map(|guide|guide.vertex),
            "suppression_reason":error,"vector_length":VECTOR_LENGTH,"directions_normalized_for_guide":true,
            "position":sample.map(|sample|vector_report(sample.position_bits())),
            "vectors":sample.map(|sample|json!({
                "normal":vector_report(sample.normal().bits()),"tangent_s":vector_report(sample.tangent_s().bits()),
                "tangent_t":vector_report(sample.tangent_t().bits()),"available_segments":([sample.normal(),sample.tangent_s(),sample.tangent_t()]
                    .map(|vector|vector_direction(vector.bits()).is_some()))})),
            "point_marker_half_extent":MARKER_HALF_EXTENT,"guide_cap":1,"maximum_segments":6})
    }

    pub fn compiled_terrain_snapshot(&self) -> Value {
        let state = &self.compiled_terrain_snapshot;
        let build = baked_build::build_state(self);
        let native_activity = self.native_activity_status();
        let current_error = state
            .ready
            .as_ref()
            .and_then(|ready| ready.current(self).err());
        let status = if state.status.is_empty() {
            "not_prepared"
        } else if state.status == "ready" && current_error.is_some() {
            "stale"
        } else {
            state.status
        };
        let ready = state.ready.as_ref();
        json!({"snapshot":self.terrain_snapshot_stamp(),"status":status,
            "token":state.token.map(|token|token.to_string()),"busy":state.worker.busy(),
            "cancel_requested":state.pending.as_ref().is_some_and(|pending|pending.control.cancelled()),
            "prepare_available":cfg!(target_os="linux") && !self.closing && !self.compiling()
                && !state.worker.busy() && !self.document.is_dirty() && build["current"] == true
                && native_activity["available"] == true && self.native_cleanup_result().is_ok()
                && std::ptr::eq(self.render_document(),self.document()),
            "current":ready.is_some() && current_error.is_none(),"current_error":current_error,
            "build_token":ready.map(|ready|ready.build.token.to_string()),"profile":ready.map(|ready|&ready.profile),
            "displacements":ready.map(|ready|ready.snapshot.displacements().len()),
            "samples":ready.map(|ready|ready.snapshot.sample_count()),
            "retained_bytes":ready.map(|ready|ready.retained_bytes),"channel":"ldr",
            "observation":ready.map(|ready|ready.observation()),"artifacts":ready.map(|ready|&ready.artifacts),
            "elapsed_ms":ready.map(|ready|ready.elapsed_ms),"cleanup_error":self.native_cleanup_result().err(),
            "error":state.error,"guide":self.terrain_guide_report(),"native_activity":native_activity,
            "comparison":self.compiled_terrain_basis_comparison(),
            "build":build,"limits":{"page_rows":MAX_PAGE_ROWS,"page_bytes":MAX_PAGE_BYTES,
                "source_bytes":MAX_SOURCE_BYTES,"retained_bytes":MAX_RETAINED_BYTES,"active_workers":1,"snapshots":1},
            "policy":POLICY})
    }

    pub fn begin_compiled_terrain_snapshot(
        &mut self,
        actor: Actor,
        snapshot: &str,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        self.native_cleanup_result()?;
        if !cfg!(target_os = "linux") {
            return Err(
                "Native terrain observation requires the supported Linux CS:S client".into(),
            );
        }
        if snapshot != self.terrain_snapshot_stamp() {
            return Err("Native terrain observation source changed; refresh its snapshot".into());
        }
        if self.compiling() || self.compiled_terrain_snapshot.worker.busy() {
            return Err("Wait for the active build or terrain observation to finish".into());
        }
        let native_observation::Capture {
            frame,
            build,
            target,
            environment,
        } = native_observation::capture(self)?;
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                token.checked_add(1)
            })
            .map_err(|_| "Native terrain observation identity exhausted")?;
        let permit = self
            .native_activity
            .as_ref()
            .map(|client| {
                client.reserve(native_activity::Phase::TerrainSnapshot, Some(&target), None)
            })
            .transpose()?;
        let ticket = self
            .compiled_terrain_snapshot
            .worker
            .begin()
            .ok_or("A native terrain observation is already running")?;
        let identity = Arc::new(());
        let control = Control::default();
        let state = &mut self.compiled_terrain_snapshot;
        state.token = Some(token);
        state.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
        });
        state.ready = None;
        state.comparison.retire();
        state.guide = None;
        state.visible = false;
        state.status = "preparing";
        state.error = None;
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(actor, "Started final native terrain observation");
        Ok(Request {
            token,
            identity,
            control,
            frame,
            build,
            target,
            environment,
            permit,
            cleanup: CleanupDisposition::Confirmed,
            _worker: ticket,
        })
    }

    pub fn finish_compiled_terrain_snapshot(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed { request, result } = completed;
        if request.cleanup == CleanupDisposition::Unconfirmed {
            self.native_cleanup_error = Some("Native terrain observation cleanup is unconfirmed; inspect its retained artifacts before further native activity".into());
        }
        let owned = self
            .compiled_terrain_snapshot
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(&pending.identity, &request.identity))
            && self.compiled_terrain_snapshot.token == Some(request.token);
        if !owned {
            drop(request);
            return Ok(self.compiled_terrain_snapshot());
        }
        let context = native_observation::verify(
            &request.frame,
            &request.build,
            &request.target,
            self,
            &request.control,
        );
        let result = match (result, context) {
            (Ok(ready), Ok(())) => Ok(ready),
            (Err(error), Err(context)) => Err(format!("{error}; {context}")),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        };
        let state = &mut self.compiled_terrain_snapshot;
        state.pending = None;
        match result {
            Ok(ready) => {
                state.ready = Some(Arc::new(ready));
                state.status = "ready";
                state.error = None;
            }
            Err(error) => {
                state.ready = None;
                state.status = if request.control.cancelled() {
                    "cancelled"
                } else {
                    "failed"
                };
                state.error = Some(baked_build::bounded_message(&error));
            }
        }
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(
            actor,
            format!(
                "Native terrain observation {}",
                self.compiled_terrain_snapshot.status
            ),
        );
        drop(request);
        Ok(self.compiled_terrain_snapshot())
    }

    pub fn fail_compiled_terrain_snapshot_start(
        &mut self,
        actor: Actor,
        token: u64,
        error: String,
        cleanup: CleanupDisposition,
    ) -> Result<Value, String> {
        if cleanup == CleanupDisposition::Unconfirmed {
            self.native_cleanup_error = Some(baked_build::bounded_message(&error));
        }
        if self.compiled_terrain_snapshot.token == Some(token)
            && self.compiled_terrain_snapshot.pending.is_some()
        {
            self.compiled_terrain_snapshot.pending = None;
            self.compiled_terrain_snapshot.ready = None;
            self.compiled_terrain_snapshot.status = "failed";
            self.compiled_terrain_snapshot.error = Some(baked_build::bounded_message(&error));
            self.invalidate_compiled_terrain_snapshot_render();
            self.log(actor, "Native terrain observation failed to start");
        }
        Err(error)
    }

    pub fn cancel_compiled_terrain_snapshot(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        if self.compiled_terrain_snapshot.comparison.running() {
            return self.cancel_compiled_terrain_basis_comparison(actor, token);
        }
        let pending = self
            .compiled_terrain_snapshot
            .pending
            .as_ref()
            .ok_or("No native terrain observation is running")?;
        pending.control.cancel()?;
        self.log(actor, "Requested native terrain observation cancellation");
        Ok(self.compiled_terrain_snapshot())
    }

    pub fn clear_compiled_terrain_snapshot(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        self.retire_compiled_terrain_snapshot();
        self.compiled_terrain_snapshot.token = Some(token);
        self.compiled_terrain_snapshot.status = "cleared";
        self.log(actor, "Cleared native terrain snapshot");
        Ok(self.compiled_terrain_snapshot())
    }

    pub fn retire_compiled_terrain_snapshot(&mut self) {
        let state = &mut self.compiled_terrain_snapshot;
        if let Some(pending) = state.pending.take() {
            let _ = pending.control.cancel();
        }
        state.token = None;
        state.ready = None;
        state.comparison.retire();
        state.guide = None;
        state.visible = false;
        state.status = "not_prepared";
        state.error = None;
        self.invalidate_compiled_terrain_snapshot_render();
    }

    fn invalidate_compiled_terrain_snapshot_render(&mut self) {
        self.compiled_terrain_snapshot.render_serial =
            self.compiled_terrain_snapshot.render_serial.wrapping_add(1);
        self.scene_serial = self.scene_serial.wrapping_add(1);
    }

    pub fn compiled_terrain_snapshot_serial(&self) -> u64 {
        self.compiled_terrain_snapshot.render_serial
    }

    pub fn compiled_terrain_snapshot_render_report(&self) -> Value {
        json!({"guide":self.terrain_guide_report(),"policy":POLICY})
    }

    pub fn preflight_compiled_terrain_snapshot(&mut self) {
        let presentation_key = (
            self.compiled_terrain_snapshot.visible
                && self.compiled_terrain_snapshot.guide.is_some(),
            self.terrain_guide_error(),
        );
        if self.compiled_terrain_snapshot.presentation_key.as_ref() != Some(&presentation_key) {
            self.compiled_terrain_snapshot.presentation_key = Some(presentation_key);
            self.invalidate_compiled_terrain_snapshot_render();
        }
    }

    pub fn inspect_compiled_terrain_snapshot(&self, query: Query) -> Result<Value, String> {
        self.check_terrain_snapshot_token(query.token)?;
        if !(1..=MAX_PAGE_ROWS).contains(&query.limit)
            || query.offset.checked_add(query.limit).is_none()
            || !(1..=MAX_PAGE_ROWS).contains(&query.source_limit)
            || query
                .source_offset
                .checked_add(query.source_limit)
                .is_none()
        {
            return Err(
                "Native terrain pages require 1–64 rows and contributors without overflow".into(),
            );
        }
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a complete native terrain snapshot first")?;
        let mut current_error = ready.current(self).err();
        let display_error = self.terrain_snapshot_display_error(ready);
        let mut rows = Vec::new();
        let mut provenance = Value::Null;
        let total;
        if let Some(ordinal) = query.displacement {
            let displacement = ready.displacement(ordinal)?;
            total = displacement.samples().len();
            for (vertex, sample) in displacement
                .samples()
                .iter()
                .copied()
                .enumerate()
                .skip(query.offset)
                .take(query.limit)
            {
                rows.push(json!({"build_token":ready.build.token.to_string(),"profile":ready.profile,
                    "displacement":ordinal,"face":displacement.face(),"power":displacement.power(),
                    "vertex":vertex,"row":vertex/displacement.grid_size(),"column":vertex%displacement.grid_size(),
                    "position":vector_report(sample.position_bits()),"normal":vector_report(sample.normal().bits()),
                    "tangent_s":vector_report(sample.tangent_s().bits()),"tangent_t":vector_report(sample.tangent_t().bits()),
                    "comparison":self.terrain_basis_sample_comparison(ordinal,vertex),
                    "frame_available":current_error.is_none() && sample_position_available(sample) && display_error.is_none(),
                    "guide_available":current_error.is_none() && sample_position_available(sample) && display_error.is_none() && self.terrain_guide_region_visible(sample)}));
            }
            provenance = self.terrain_snapshot_provenance(
                ready,
                displacement.face(),
                query.source_offset,
                query.source_limit,
            )?;
        } else {
            if query.source_offset != 0 {
                return Err("Contributor pages require one native displacement ordinal".into());
            }
            total = ready.snapshot.displacements().len();
            for displacement in ready
                .snapshot
                .displacements()
                .iter()
                .skip(query.offset)
                .take(query.limit)
            {
                let contributors = ready
                    .build
                    .origins
                    .contributors(usize::from(displacement.face()));
                let count = contributors.as_ref().map_or(0, ExactSizeIterator::len);
                let label = contributors
                    .and_then(|mut origins| origins.next().map(|origin| origin.label(true)));
                rows.push(json!({"build_token":ready.build.token.to_string(),"profile":ready.profile,
                    "displacement":displacement.ordinal(),"face":displacement.face(),"power":displacement.power(),
                    "grid_size":displacement.grid_size(),"samples":displacement.samples().len(),
                    "source_label":label,"source_contributors":count,
                    "comparison":self.terrain_basis_displacement_comparison(displacement.ordinal())}));
            }
        }
        current_error = ready.current(self).err().or(current_error);
        if current_error.is_some() && query.displacement.is_some() {
            for row in &mut rows {
                row["frame_available"] = json!(false);
                row["guide_available"] = json!(false);
            }
        }
        bounded_report(
            json!({"token":query.token.to_string(),"status":if current_error.is_none(){"ready"}else{"historical"},
            "current":current_error.is_none(),"current_error":current_error,"build_token":ready.build.token.to_string(),
            "profile":ready.profile,"channel":"ldr","displacement":query.displacement,
            "page":{"kind":if query.displacement.is_some(){"samples"}else{"displacements"},"offset":query.offset,"limit":query.limit,
                "total":total,"returned":rows.len(),"next_offset":query.offset.checked_add(query.limit).filter(|offset|*offset<total),"rows":rows},
            "provenance":provenance,"guide":self.terrain_guide_report(),
            "comparison":self.compiled_terrain_basis_comparison(),"policy":POLICY}),
        )
    }

    fn terrain_snapshot_provenance(
        &self,
        ready: &Ready,
        face: u16,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        let contributors = ready
            .build
            .origins
            .contributors(usize::from(face))
            .ok_or("Compiled FACE provenance is unavailable")?;
        let total = contributors.len();
        let mut bytes = 0usize;
        let mut rows = Vec::new();
        for origin in contributors.skip(offset).take(limit) {
            bytes = origin
                .source_bytes()
                .checked_mul(6)
                .and_then(|additional| additional.checked_add(std::mem::size_of_val(origin)))
                .and_then(|additional| bytes.checked_add(additional))
                .filter(|bytes| *bytes <= MAX_SOURCE_BYTES)
                .ok_or("Compiled source routes exceed 128 KiB; request fewer contributors")?;
            rows.push(json!(origin));
        }
        Ok(
            json!({"face":face,"model":ready.build.origins.model(usize::from(face)),"contributors":rows,
            "page":{"offset":offset,"limit":limit,"total":total,"next_offset":offset.checked_add(limit).filter(|offset|*offset<total)},
            "relation":"Compiled source contributors only. No authored vertex or triangle correspondence is established."}),
        )
    }

    pub fn frame_compiled_terrain_sample(
        &mut self,
        actor: Actor,
        token: u64,
        displacement: u16,
        vertex: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_terrain_snapshot_token(token)?;
        inspection_framing::validate_aspect(aspect)?;
        self.inspection_viewport_ready("framing a native terrain sample")?;
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a native terrain snapshot first")?;
        ready.current(self)?;
        if let Some(error) = self.terrain_snapshot_display_error(ready) {
            return Err(error);
        }
        let sample = ready.sample(displacement, vertex)?;
        if !sample_position_available(sample) {
            return Err("Native terrain sample has no finite observed position".into());
        }
        let point = Vec3::from_array(sample.position_bits().map(f32::from_bits));
        let half = Vec3::splat(inspection_framing::ORIGIN_HALF_EXTENT);
        let camera = self.inspection_frame_camera(
            point - half,
            point + half,
            aspect,
            "Native terrain sample",
        )?;
        ready.current(self)?;
        let build_token = ready.build.token.to_string();
        let profile = ready.profile.clone();
        let face = ready.displacement(displacement)?.face();
        self.try_set_camera(camera)?;
        self.log(
            actor,
            format!("Frame native displacement {displacement} sample {vertex}"),
        );
        Ok(
            json!({"token":token.to_string(),"build_token":build_token,"profile":profile,
            "face":face,"displacement":displacement,"vertex":vertex,
            "position":vector_report(sample.position_bits()),"camera":{"position":camera.position.to_array(),
                "yaw_degrees":camera.yaw_degrees,"pitch_degrees":camera.pitch_degrees}}),
        )
    }

    pub fn set_compiled_terrain_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
        displacement: u16,
        vertex: usize,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_terrain_snapshot_token(token)?;
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a native terrain snapshot first")?;
        ready.current(self)?;
        if let Some(error) = self.terrain_snapshot_display_error(ready) {
            return Err(error);
        }
        let sample = ready.sample(displacement, vertex)?;
        if !sample_position_available(sample) {
            return Err("Native terrain sample has no finite observed position".into());
        }
        if !self.terrain_guide_region_visible(sample) {
            return Err("The native sample is outside the working region".into());
        }
        self.compiled_terrain_snapshot.guide = Some(Guide {
            displacement,
            vertex,
        });
        self.compiled_terrain_snapshot.visible = true;
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(
            actor,
            format!("Show native displacement {displacement} sample {vertex} vectors"),
        );
        Ok(self.terrain_guide_report())
    }

    pub fn clear_compiled_terrain_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        self.compiled_terrain_snapshot.guide = None;
        self.compiled_terrain_snapshot.visible = false;
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(actor, "Cleared native terrain vector guide");
        Ok(self.terrain_guide_report())
    }

    pub fn set_compiled_terrain_snapshot_visible(
        &mut self,
        actor: Actor,
        token: u64,
        visible: bool,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        if visible {
            self.ensure_not_closing()?;
            if self.compiled_terrain_snapshot.guide.is_none() {
                return Err("Choose one native terrain sample guide first".into());
            }
            if let Some(error) = self.terrain_guide_error() {
                return Err(error);
            }
        }
        self.compiled_terrain_snapshot.visible = visible;
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(
            actor,
            if visible {
                "Show native terrain vector guide"
            } else {
                "Hide native terrain vector guide"
            },
        );
        Ok(self.terrain_guide_report())
    }

    pub fn compiled_terrain_snapshot_render_guard(
        &self,
        _view: ViewOptions,
    ) -> Result<Option<RenderGuard>, String> {
        let state = &self.compiled_terrain_snapshot;
        let displayed =
            state.visible && state.guide.is_some() && self.terrain_guide_error().is_none();
        if displayed {
            state
                .ready
                .as_ref()
                .ok_or("Native terrain snapshot was retired")?
                .current(self)?;
        }
        Ok(Some(RenderGuard {
            token: state.token,
            serial: state.render_serial,
            ready: state.ready.as_ref().map(Arc::downgrade),
            displayed,
        }))
    }

    pub fn verify_compiled_terrain_snapshot_render(
        &mut self,
        guard: Option<&RenderGuard>,
        _view: ViewOptions,
    ) -> Result<(), String> {
        let Some(guard) = guard else {
            return Err("Native terrain render custody is unavailable; refresh the scene".into());
        };
        let result = (|| {
            let state = &self.compiled_terrain_snapshot;
            let same_ready = match (&state.ready, &guard.ready) {
                (Some(current), Some(captured)) => Weak::ptr_eq(&Arc::downgrade(current), captured),
                (None, None) => true,
                _ => false,
            };
            let displayed =
                state.visible && state.guide.is_some() && self.terrain_guide_error().is_none();
            if state.token != guard.token
                || state.render_serial != guard.serial
                || !same_ready
                || displayed != guard.displayed
            {
                return Err(
                    "Native terrain guide context changed during rendering; render again".into(),
                );
            }
            if guard.displayed {
                state
                    .ready
                    .as_ref()
                    .ok_or("Native terrain snapshot was retired")?
                    .current(self)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.preflight_compiled_terrain_snapshot();
        }
        result
    }

    pub fn append_compiled_terrain_snapshot_guide(&self, mesh: &mut SceneMesh, _view: ViewOptions) {
        let state = &self.compiled_terrain_snapshot;
        if !state.visible || self.terrain_guide_error().is_some() {
            return;
        }
        let Some((ready, guide)) = state.ready.as_ref().zip(state.guide) else {
            return;
        };
        let Ok(sample) = ready.sample(guide.displacement, guide.vertex) else {
            return;
        };
        let point = Vec3::from_array(sample.position_bits().map(f32::from_bits));
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            for position in [
                point - axis * MARKER_HALF_EXTENT,
                point + axis * MARKER_HALF_EXTENT,
            ] {
                mesh.lines.push(render::scene::LineVertex {
                    position: position.to_array(),
                    color: MARKER_COLOR,
                });
            }
        }
        for (vector, color) in [
            (sample.normal(), NORMAL_COLOR),
            (sample.tangent_s(), TANGENT_S_COLOR),
            (sample.tangent_t(), TANGENT_T_COLOR),
        ] {
            if let Some(direction) = vector_direction(vector.bits()) {
                for position in [point, point + direction * VECTOR_LENGTH] {
                    mesh.lines.push(render::scene::LineVertex {
                        position: position.to_array(),
                        color,
                    });
                }
            }
        }
    }
}
