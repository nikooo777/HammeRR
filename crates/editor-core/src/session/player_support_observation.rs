use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use compiler::Control;
use glam::Vec3;
use native_runtime::{CaptureObserver, CleanupDisposition, PlayerSupportSample};
use serde_json::{Value, json};

pub use native_runtime::{
    MAX_PLAYER_SUPPORT_DURATION_MS, MAX_PLAYER_SUPPORT_INTERVAL_MS, MAX_PLAYER_SUPPORT_SAMPLES,
    MIN_PLAYER_SUPPORT_DURATION_MS, MIN_PLAYER_SUPPORT_INTERVAL_MS, MIN_PLAYER_SUPPORT_SAMPLES,
    PlayerSupportSettings as Options,
};

use super::native_observation::{Frame, MAX_METADATA_BYTES};
use super::{Actor, Session, baked_build, inspection_framing, native_observation, worker};
use crate::runtime::{SharedSession, lock, native_activity};

pub const MAX_PAGE_ROWS: usize = 64;
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
pub const MAX_RETAINED_BYTES: usize = 2 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: usize = 128 * 1024;
const POLICY: &str = "Passive snapshots of one freshly spawned idle player in the exact retained compiled BSP, observed in a private owned Linux CS:S client with -insecure and sv_lan 1. Samples retain actual native player identity, origin, velocities, collision bounds, movement hull, simulation time, ground owner and checked supporting surface identity. Airborne surface fields are cached previous state and do not establish present support. Only the native observer's completed publication contract can produce rows. Natural standing, falling and sliding describe the observed route only; no input, swept trace, contact plane/triangle identity, authored face correspondence, seam crossing or VPhysics friction-contact qualification is claimed. Historical samples remain inspectable; framing requires the current build/runtime and no displayed geometry draft.";
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: worker::Slot,
    token: Option<u64>,
    pending: Option<Pending>,
    ready: Option<Ready>,
    status: &'static str,
    error: Option<String>,
}

struct Pending {
    identity: Arc<()>,
    control: Control,
    options: Options,
}

struct Ready {
    frame: Frame,
    build: Arc<baked_build::RetainedBuild>,
    target: native_activity::Target,
    samples: Vec<PlayerSupportSample>,
    observation: String,
    profile: String,
    options: Options,
    artifacts: PathBuf,
    elapsed_ms: u64,
    retained_bytes: usize,
}

impl Ready {
    fn current(&self, session: &Session) -> Result<(), String> {
        native_observation::verify(
            &self.frame,
            &self.build,
            &self.target,
            session,
            &Control::default(),
        )
    }

    fn observation(&self) -> Value {
        serde_json::from_str(&self.observation)
            .expect("A retained player observation was serialized as JSON")
    }

    fn sample(&self, index: usize) -> Result<&PlayerSupportSample, String> {
        self.samples
            .get(index)
            .filter(|sample| sample.ordinal == index)
            .ok_or_else(|| "Player sample index is absent from this observation".into())
    }
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    control: Control,
    frame: Frame,
    build: Arc<baked_build::RetainedBuild>,
    target: native_activity::Target,
    environment: native_runtime::CaptureEnvironment,
    options: Options,
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
        let bsp = baked_build::read_owned_bytes_controlled(&self.build.output, &self.control)?;
        let decoded_bytes = baked_build::decoded_bsp_bound(&bsp)?;
        if decoded_bytes > baked_build::MAX_BSP_BYTES as usize {
            return Err("Decoded player-observation BSP exceeds its owned byte budget".into());
        }
        self.checkpoint(session)?;
        let artifacts_parent = crate::recovery::default_directory()?.join("player-support");
        std::fs::create_dir_all(&artifacts_parent).map_err(|error| error.to_string())?;
        let request = native_runtime::PlayerSupportRequest {
            environment: self.environment.clone(),
            bsp,
            map_name: "hammerr_player_support".into(),
            artifacts_parent,
            settings: self.options,
        };
        if let Some(permit) = &mut self.permit {
            permit.started();
        }
        self.cleanup = CleanupDisposition::Unconfirmed;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_runtime::observe_player_support_with_cleanup(
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
                    permit
                        .failed_cleanup("Player support observer panicked; cleanup is unconfirmed");
                }
                return Err("Player support observer panicked; cleanup is unconfirmed".into());
            }
        };
        if !receipt.preservation_passed || !receipt.observation.resumed {
            return Err(
                "Player support observation did not confirm preservation and resume".into(),
            );
        }
        if Some(receipt.observation.bsp_sha256.as_str()) != self.build.output.sha256()
            || Some(receipt.observation.engine_sha256.as_str()) != self.target.engine_image_sha256()
            || Some(receipt.observation.server_sha256.as_str()) != self.target.server_image_sha256()
            || Some(receipt.observation.vphysics_sha256.as_str())
                != self.target.physics_image_sha256()
        {
            return Err(
                "Player support observation belongs to different BSP or runtime bytes".into(),
            );
        }
        self.checkpoint(session)?;
        self.build.verify(&self.control, MAX_METADATA_BYTES)?;
        let profile = receipt.observation.profile.clone();
        if profile.is_empty() || profile.len() > 512 {
            return Err("Player support observation has no bounded native profile".into());
        }
        if receipt.samples.is_empty()
            || receipt.samples.len() > self.options.max_samples
            || receipt
                .samples
                .iter()
                .enumerate()
                .any(|(index, sample)| sample.ordinal != index)
        {
            return Err("Player support observation has an invalid bounded sample sequence".into());
        }
        let observation =
            serde_json::to_string(&receipt.observation).map_err(|error| error.to_string())?;
        if observation.len() > MAX_OBSERVATION_BYTES {
            return Err("Player support observation receipt exceeds its metadata budget".into());
        }
        crate::report_path::checked(&receipt.artifacts)?;
        let sample_storage = receipt
            .samples
            .capacity()
            .checked_mul(std::mem::size_of::<PlayerSupportSample>())
            .and_then(|bytes| {
                receipt.samples.iter().try_fold(bytes, |bytes, sample| {
                    bytes
                        .checked_add(sample.phase.capacity())
                        .and_then(|bytes| bytes.checked_add(sample.ground_resolution.capacity()))
                        .and_then(|bytes| bytes.checked_add(sample.player.classname.capacity()))
                        .and_then(|bytes| {
                            bytes.checked_add(
                                sample
                                    .ground
                                    .as_ref()
                                    .map_or(0, |entity| entity.classname.capacity()),
                            )
                        })
                        .and_then(|bytes| {
                            bytes.checked_add(
                                sample
                                    .surface
                                    .as_ref()
                                    .map_or(0, |surface| surface.name.capacity()),
                            )
                        })
                })
            })
            .ok_or("Player support sample storage exceeds its byte budget")?;
        let retained_bytes = sample_storage
            .checked_add(observation.capacity())
            .and_then(|bytes| bytes.checked_add(profile.capacity()))
            .and_then(|bytes| bytes.checked_add(receipt.artifacts.capacity()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Ready>()))
            .filter(|bytes| *bytes <= MAX_RETAINED_BYTES)
            .ok_or("Player support observation exceeds its retained-result budget")?;
        self.checkpoint(session)?;
        Ok(Ready {
            frame: self.frame.clone(),
            build: self.build.clone(),
            target: self.target.clone(),
            samples: receipt.samples,
            observation,
            profile,
            options: self.options,
            artifacts: receipt.artifacts,
            elapsed_ms: receipt.elapsed_ms,
            retained_bytes,
        })
    }
}

#[derive(Clone, Copy)]
pub struct Query {
    pub token: u64,
    pub offset: usize,
    pub limit: usize,
}

impl Session {
    fn player_support_stamp(&self) -> String {
        format!(
            "player-support-v1:{}:{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.content_epoch,
            self.compiled
                .as_ref()
                .map_or(0, |build| build.lightmap_identity.token)
        )
    }

    fn check_player_support_token(&self, token: u64) -> Result<(), String> {
        if self.player_support_observation.token != Some(token) {
            return Err("Player support observation token is stale or foreign".into());
        }
        Ok(())
    }

    fn player_support_frame_error(&self, ready: &Ready) -> Option<String> {
        ready.current(self).err().or_else(|| {
            if !std::ptr::eq(self.render_document(), self.document()) {
                Some("Finish the displayed geometry draft before framing a player sample".into())
            } else {
                self.inspection_viewport_ready("framing a player sample")
                    .err()
            }
        })
    }

    pub fn player_support_observation(&self) -> Value {
        let state = &self.player_support_observation;
        let build = baked_build::build_state(self);
        let native_activity = self.native_activity_status();
        let ready = state.ready.as_ref();
        let current_error = ready.and_then(|ready| ready.current(self).err());
        let status = if state.status.is_empty() {
            "idle"
        } else if state.status == "ready" && current_error.is_some() {
            "stale"
        } else {
            state.status
        };
        json!({"snapshot":self.player_support_stamp(),"status":status,
            "token":state.token.map(|token|token.to_string()),"busy":state.worker.busy(),
            "cancel_requested":state.pending.as_ref().is_some_and(|pending|pending.control.cancelled()),
            "start_available":cfg!(target_os="linux") && !self.closing && !self.compiling()
                && !state.worker.busy() && !self.document.is_dirty() && build["current"] == true
                && native_activity["available"] == true && self.native_cleanup_result().is_ok()
                && std::ptr::eq(self.render_document(),self.document()),
            "current":ready.is_some() && current_error.is_none(),"current_error":current_error,
            "frame_error":ready.and_then(|ready|self.player_support_frame_error(ready)),
            "build_token":ready.map(|ready|ready.build.token.to_string()),"profile":ready.map(|ready|&ready.profile),
            "samples":ready.map(|ready|ready.samples.len()),
            "settings":ready.map(|ready|ready.options).or_else(||state.pending.as_ref().map(|pending|pending.options)),
            "observation":ready.map(Ready::observation),"artifacts":ready.map(|ready|&ready.artifacts),
            "elapsed_ms":ready.map(|ready|ready.elapsed_ms),"retained_bytes":ready.map(|ready|ready.retained_bytes),
            "error":state.error,"cleanup_error":self.native_cleanup_result().err(),"native_activity":native_activity,"build":build,
            "limits":{"page_rows":MAX_PAGE_ROWS,"page_bytes":MAX_PAGE_BYTES,"retained_bytes":MAX_RETAINED_BYTES,
                "duration_ms":[MIN_PLAYER_SUPPORT_DURATION_MS,MAX_PLAYER_SUPPORT_DURATION_MS],
                "interval_ms":[MIN_PLAYER_SUPPORT_INTERVAL_MS,MAX_PLAYER_SUPPORT_INTERVAL_MS],
                "max_samples":[MIN_PLAYER_SUPPORT_SAMPLES,MAX_PLAYER_SUPPORT_SAMPLES],"active_workers":1,"observations":1},
            "policy":POLICY})
    }

    pub fn begin_player_support_observation(
        &mut self,
        actor: Actor,
        snapshot: &str,
        options: Options,
    ) -> Result<Request, String> {
        options.validate().map_err(|error| error.to_string())?;
        if snapshot != self.player_support_stamp() {
            return Err("Player support source changed; refresh its snapshot".into());
        }
        if self.player_support_observation.worker.busy() {
            return Err("Wait for the active player support observation to finish".into());
        }
        let native_observation::Capture {
            frame,
            build,
            target,
            environment,
        } = native_observation::capture(self)?;
        let target = target.with_player_support(&environment)?;
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                token.checked_add(1)
            })
            .map_err(|_| "Player support observation identity exhausted")?;
        let permit = self
            .native_activity
            .as_ref()
            .map(|client| {
                client.reserve(native_activity::Phase::PlayerSupport, Some(&target), None)
            })
            .transpose()?;
        let ticket = self
            .player_support_observation
            .worker
            .begin()
            .ok_or("A player support observation is already running")?;
        let identity = Arc::new(());
        let control = Control::default();
        let state = &mut self.player_support_observation;
        state.token = Some(token);
        state.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
            options,
        });
        state.ready = None;
        state.status = "observing";
        state.error = None;
        self.log(actor, "Started passive native player support observation");
        Ok(Request {
            token,
            identity,
            control,
            frame,
            build,
            target,
            environment,
            options,
            permit,
            cleanup: CleanupDisposition::Confirmed,
            _worker: ticket,
        })
    }

    pub fn finish_player_support_observation(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed { request, result } = completed;
        if request.cleanup == CleanupDisposition::Unconfirmed {
            self.native_cleanup_error = Some("Player support cleanup is unconfirmed; inspect retained artifacts before further native activity".into());
        }
        let owned = self
            .player_support_observation
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(&pending.identity, &request.identity))
            && self.player_support_observation.token == Some(request.token);
        if !owned {
            drop(request);
            return Ok(self.player_support_observation());
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
        let state = &mut self.player_support_observation;
        state.pending = None;
        match result {
            Ok(ready) => {
                state.ready = Some(ready);
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
        let status = state.status;
        self.log(actor, format!("Player support observation {status}"));
        drop(request);
        Ok(self.player_support_observation())
    }

    pub fn fail_player_support_observation_start(
        &mut self,
        actor: Actor,
        token: u64,
        error: String,
        cleanup: CleanupDisposition,
    ) -> Result<Value, String> {
        if cleanup == CleanupDisposition::Unconfirmed {
            self.native_cleanup_error = Some(baked_build::bounded_message(&error));
        }
        if self.player_support_observation.token == Some(token)
            && self.player_support_observation.pending.is_some()
        {
            let state = &mut self.player_support_observation;
            state.pending = None;
            state.ready = None;
            state.status = "failed";
            state.error = Some(baked_build::bounded_message(&error));
            self.log(actor, "Player support observation failed to start");
        }
        Err(error)
    }

    pub fn cancel_player_support_observation(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_player_support_token(token)?;
        let pending = self
            .player_support_observation
            .pending
            .as_ref()
            .ok_or("No player support observation is running")?;
        pending.control.cancel()?;
        self.log(actor, "Requested player support observation cancellation");
        Ok(self.player_support_observation())
    }

    pub fn clear_player_support_observation(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_player_support_token(token)?;
        self.retire_player_support_observation();
        self.player_support_observation.token = Some(token);
        self.player_support_observation.status = "cleared";
        self.log(actor, "Cleared player support observation");
        Ok(self.player_support_observation())
    }

    pub fn retire_player_support_observation(&mut self) {
        let state = &mut self.player_support_observation;
        if let Some(pending) = state.pending.take() {
            let _ = pending.control.cancel();
        }
        state.token = None;
        state.ready = None;
        state.status = "idle";
        state.error = None;
    }

    pub fn inspect_player_support_observation(&self, query: Query) -> Result<Value, String> {
        self.check_player_support_token(query.token)?;
        if !(1..=MAX_PAGE_ROWS).contains(&query.limit)
            || query.offset.checked_add(query.limit).is_none()
        {
            return Err("Player support pages require 1–64 rows without overflow".into());
        }
        let ready = self
            .player_support_observation
            .ready
            .as_ref()
            .ok_or("Complete a player support observation first")?;
        let current_error = ready.current(self).err();
        let frame_error = self.player_support_frame_error(ready);
        let mut rows = Vec::new();
        for sample in ready.samples.iter().skip(query.offset).take(query.limit) {
            let mut row = serde_json::to_value(sample).map_err(|error| error.to_string())?;
            row["frame_available"] = json!(frame_error.is_none());
            rows.push(row);
        }
        let report = json!({"token":query.token.to_string(),"build_token":ready.build.token.to_string(),"profile":ready.profile,
            "current":current_error.is_none(),"current_error":current_error,"frame_error":frame_error,
            "page":{"offset":query.offset,"limit":query.limit,"total":ready.samples.len(),"returned":rows.len(),
                "next_offset":query.offset.checked_add(query.limit).filter(|offset|*offset<ready.samples.len()),"rows":rows},
            "policy":POLICY});
        let bytes = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_PAGE_BYTES {
            return Err("Player support inspection exceeds 256 KiB; request fewer rows".into());
        }
        Ok(report)
    }

    pub fn frame_player_support_sample(
        &mut self,
        actor: Actor,
        token: u64,
        index: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_player_support_token(token)?;
        inspection_framing::validate_aspect(aspect)?;
        let ready = self
            .player_support_observation
            .ready
            .as_ref()
            .ok_or("Complete a player support observation first")?;
        if let Some(error) = self.player_support_frame_error(ready) {
            return Err(error);
        }
        let sample = ready.sample(index)?;
        let origin = Vec3::from_array(sample.origin);
        let mins = origin + Vec3::from_array(sample.hull_mins);
        let maxs = origin + Vec3::from_array(sample.hull_maxs);
        if !origin.is_finite() || !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all()
        {
            return Err("Player sample has no finite ordered native movement hull".into());
        }
        let camera = self.inspection_frame_camera(mins, maxs, aspect, "Native player sample")?;
        ready.current(self)?;
        let report = json!({"token":token.to_string(),"build_token":ready.build.token.to_string(),
            "profile":ready.profile,"index":index,"origin":sample.origin,"hull_mins":sample.hull_mins,"hull_maxs":sample.hull_maxs,
            "camera":{"position":camera.position.to_array(),"yaw_degrees":camera.yaw_degrees,"pitch_degrees":camera.pitch_degrees}});
        self.try_set_camera(camera)?;
        self.log(actor, format!("Frame native player sample {index}"));
        Ok(report)
    }
}
