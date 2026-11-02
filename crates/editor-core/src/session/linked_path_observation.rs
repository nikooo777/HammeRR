use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use native_runtime::{
    CaptureObserver, CleanupDisposition, MovementMode, MovementReceipt, MovementRequest,
    MovementTrainIdentity,
};
use serde_json::{Value, json};

use super::{Actor, Session, Snapshot, baked_build, worker};
use crate::runtime::{SharedSession, lock, native_activity};

#[derive(Clone)]
pub struct Options {
    pub train: u32,
    pub stationary: bool,
    pub sample_interval_ms: u16,
    pub samples_per_phase: u16,
    pub snapshot: String,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: worker::Slot,
    serial: u64,
    active: Option<Active>,
    last: Option<Last>,
    error: Option<String>,
}

struct Active {
    token: String,
    cancelled: Arc<AtomicBool>,
}

struct Last {
    frame: Frame,
    build: Arc<baked_build::RetainedBuild>,
    target: native_activity::Target,
    report: Value,
}

#[derive(Clone)]
struct Frame {
    snapshot: Snapshot,
    path: Option<std::path::PathBuf>,
    content: u64,
    build: u64,
    options: compiler::Options,
}

impl Frame {
    fn verify(&self, session: &Session) -> Result<(), String> {
        if session.closing
            || !session.current_snapshot(&self.snapshot)
            || session.path != self.path
            || session.content_epoch != self.content
            || session.compile_options() != self.options
            || session
                .compiled
                .as_ref()
                .is_none_or(|build| build.lightmap_identity.token != self.build)
        {
            return Err(
                "The document, build or content changed during movement observation".into(),
            );
        }
        Ok(())
    }
}

pub(crate) struct Request {
    pub(crate) token: String,
    frame: Frame,
    source: compiler::FileIdentity,
    build: Arc<baked_build::RetainedBuild>,
    environment: native_runtime::CaptureEnvironment,
    train: MovementTrainIdentity,
    options: Options,
    cancelled: Arc<AtomicBool>,
    target: native_activity::Target,
    permit: Option<native_activity::Permit>,
    cleanup: CleanupDisposition,
    _worker: worker::Ticket,
}

pub(crate) struct Completed {
    request: Request,
    result: Result<MovementReceipt, String>,
}

struct Observer<'a> {
    request: &'a Request,
    session: &'a SharedSession,
}

impl CaptureObserver for Observer<'_> {
    fn checkpoint(&mut self) -> std::io::Result<()> {
        let result = if self.request.cancelled.load(Ordering::Acquire) {
            Err("Movement observation cancelled".to_owned())
        } else {
            self.request
                .frame
                .verify(&lock(self.session))
                .and_then(|()| self.request.target.verify())
                .and_then(|()| {
                    self.request
                        .source
                        .verify_metadata()
                        .map_err(|error| error.to_string())
                })
                .and_then(|()| {
                    self.request
                        .build
                        .verify_metadata(&compiler::Control::default(), 32 * 1024 * 1024)
                })
        };
        result.map_err(std::io::Error::other)
    }

    fn sample_progress(&mut self, _current: usize, _total: usize) {}
}

impl Request {
    pub(crate) fn run(mut self, session: &SharedSession) -> Completed {
        let result = self.execute(session);
        Completed {
            request: self,
            result,
        }
    }

    fn execute(&mut self, session: &SharedSession) -> Result<MovementReceipt, String> {
        Observer {
            request: self,
            session,
        }
        .checkpoint()
        .map_err(|error| error.to_string())?;
        let control = compiler::Control::default();
        self.source
            .verify(&control)
            .map_err(|error| error.to_string())?;
        self.build.verify(&control, 32 * 1024 * 1024)?;
        let bsp = baked_build::read_owned_bytes(&self.build.output)?;
        let source_sha256 = self
            .source
            .sha256()
            .ok_or("Saved source SHA256 is unavailable")?
            .to_owned();
        let artifacts_parent = crate::recovery::default_directory()?.join("native-movement");
        std::fs::create_dir_all(&artifacts_parent).map_err(|error| error.to_string())?;
        let request = MovementRequest {
            environment: self.environment.clone(),
            bsp,
            map_name: "hammerr_movement".into(),
            source_sha256,
            train: self.train.clone(),
            mode: if self.options.stationary {
                MovementMode::Stationary
            } else {
                MovementMode::StopStartStopStart
            },
            sample_interval_ms: self.options.sample_interval_ms,
            samples_per_phase: self.options.samples_per_phase,
            artifacts_parent,
        };
        if let Some(permit) = &mut self.permit {
            permit.started();
        }
        self.cleanup = CleanupDisposition::Unconfirmed;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_runtime::observe_movement_with_cleanup(
                request,
                &mut Observer {
                    request: self,
                    session,
                },
            )
        }));
        match result {
            Ok(Ok(receipt)) => {
                self.cleanup = receipt.cleanup;
                if let Some(permit) = &mut self.permit {
                    permit.confirmed_clean();
                }
                self.source
                    .verify(&control)
                    .map_err(|error| error.to_string())?;
                self.build.verify(&control, 32 * 1024 * 1024)?;
                Ok(receipt)
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
                Err(error.to_string())
            }
            Err(_) => {
                if let Some(permit) = &mut self.permit {
                    permit.failed_cleanup("Movement worker panicked; cleanup is unconfirmed");
                }
                Err("Movement worker panicked; cleanup is unconfirmed".into())
            }
        }
    }
}

impl Session {
    fn movement_snapshot(&self) -> String {
        format!(
            "movement-v1:{}:{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.content_epoch,
            self.compiled
                .as_ref()
                .map_or(0, |build| build.lightmap_identity.token)
        )
    }

    pub fn linked_path_observation(&self, include_samples: bool) -> Value {
        let native_activity = self.native_activity_status();
        let current_build = self.compiled.as_ref().is_some_and(|build| {
            self.current_snapshot(&build.snapshot)
                && build.content_epoch == self.content_epoch
                && build.options == self.compile_options()
        });
        let mut last = self.linked_path_observation.last.as_ref().map(|last| {
            let mut report = last.report.clone();
            let current = last
                .frame
                .verify(self)
                .and_then(|()| last.target.verify())
                .and_then(|()| {
                    last.build
                        .verify_metadata(&compiler::Control::default(), 32 * 1024 * 1024)
                });
            report["current"] = json!(current.is_ok());
            report["current_error"] = json!(current.err());
            report
        });
        if !include_samples
            && let Some(last) = &mut last
            && let Some(receipt) = last.get_mut("receipt").and_then(Value::as_object_mut)
        {
            receipt.remove("samples");
        }
        json!({"snapshot":self.movement_snapshot(), "busy":self.linked_path_observation.worker.busy(),
            "ready":cfg!(target_os="linux") && current_build && !self.compiling() && !self.closing
                && self.native_cleanup_result().is_ok()
                && !self.document.is_dirty() && native_activity["available"] == true
                && std::ptr::eq(self.render_document(), self.document()),
            "native_activity":native_activity,
            "cleanup_error":self.native_cleanup_result().err(),
            "token":self.linked_path_observation.active.as_ref().map(|active|&active.token),
            "cancel_requested":self.linked_path_observation.active.as_ref().is_some_and(|active|active.cancelled.load(Ordering::Acquire)),
            "last":last, "platform":"Linux CS:S", "maximum_samples":256,
            "error":self.linked_path_observation.error,
            "notes":"An isolated copy receives Stop/Start controls. Samples are actual native absolute origins and game time; train centers can differ from their origins. Authored map/history/camera remain unchanged. Fresh saved compilation and Steam are required."})
    }

    pub(crate) fn begin_linked_path_observation(
        &mut self,
        actor: Actor,
        options: Options,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        self.native_cleanup_result()?;
        if !cfg!(target_os = "linux") {
            return Err("Native movement observation requires Linux CS:S".into());
        }
        if options.snapshot != self.movement_snapshot() {
            return Err("Movement observation source changed; refresh its snapshot".into());
        }
        if self.compiling() || !std::ptr::eq(self.render_document(), self.document()) {
            return Err("Finish builds and geometry previews before observing movement".into());
        }
        if self.linked_path_observation.worker.busy() {
            return Err("A movement observation is already running".into());
        }
        if self.document.is_dirty() {
            return Err("Save and compile the map before observing native movement".into());
        }
        if !(100..=1000).contains(&options.sample_interval_ms)
            || !(3..=64).contains(&options.samples_per_phase)
            || usize::from(options.sample_interval_ms)
                * usize::from(options.samples_per_phase)
                * if options.stationary { 1 } else { 4 }
                > 60_000
        {
            return Err(
                "Use 100–1000 ms intervals and 3–64 samples per phase within 60 seconds".into(),
            );
        }
        let status = baked_build::build_state(self);
        if status["current"] != true {
            return Err(format!(
                "Movement observation requires a current saved build: {}",
                status["diagnostics"]
            ));
        }
        let entity = self
            .document
            .entity(options.train)
            .ok_or("Train entity does not exist")?;
        if entity.hidden
            || !matches!(entity.classname(), Some("func_train" | "func_tracktrain"))
            || entity
                .properties
                .get("parentname")
                .is_some_and(|parent| !parent.trim().is_empty())
        {
            return Err("Observe a visible, unparented func_train or func_tracktrain".into());
        }
        let train = MovementTrainIdentity {
            hammer_id: i32::try_from(options.train)
                .map_err(|_| "Train ID exceeds native integer bounds")?,
            classname: entity
                .classname()
                .ok_or("Train classname is missing")?
                .to_owned(),
            targetname: entity
                .properties
                .get("targetname")
                .filter(|name| !name.trim().is_empty())
                .ok_or("Train needs an exact targetname")?
                .to_owned(),
        };
        let compiled = self
            .compiled
            .as_ref()
            .ok_or("Compile the saved map first")?;
        let source = compiled
            .lightmap_identity
            .source
            .as_ref()
            .map_err(Clone::clone)?
            .as_ref()
            .ok_or("Save and compile the map first")?
            .clone();
        if source.sha256().is_none() {
            return Err("Saved source identity has no content hash".into());
        }
        let frame = Frame {
            snapshot: compiled.snapshot.clone(),
            path: self.path.clone(),
            content: self.content_epoch,
            build: compiled.lightmap_identity.token,
            options: self.compile_options(),
        };
        let build = baked_build::RetainedBuild::capture(compiled)?;
        let environment = self.capture_environment()?;
        let target = native_activity::Target::capture(&environment)?;
        let permit = self
            .native_activity
            .as_ref()
            .map(|client| client.reserve(native_activity::Phase::Movement, Some(&target), None))
            .transpose()?;
        let serial = self
            .linked_path_observation
            .serial
            .checked_add(1)
            .ok_or("Movement job identity exhausted")?;
        let token = format!("{}:{serial}", self.movement_snapshot());
        let cancelled = Arc::new(AtomicBool::new(false));
        let ticket = self
            .linked_path_observation
            .worker
            .begin()
            .ok_or("Movement observation is busy")?;
        self.linked_path_observation.serial = serial;
        self.linked_path_observation.error = None;
        self.linked_path_observation.active = Some(Active {
            token: token.clone(),
            cancelled: cancelled.clone(),
        });
        self.log(
            actor,
            format!(
                "Started native movement observation for entity:{}",
                options.train
            ),
        );
        Ok(Request {
            token,
            frame,
            source,
            build,
            environment,
            train,
            options,
            cancelled,
            target,
            permit,
            cleanup: CleanupDisposition::Confirmed,
            _worker: ticket,
        })
    }

    pub fn cancel_linked_path_observation(
        &mut self,
        actor: Actor,
        token: &str,
    ) -> Result<Value, String> {
        let active = self
            .linked_path_observation
            .active
            .as_ref()
            .filter(|active| active.token == token)
            .ok_or("Movement observation token is stale or foreign")?;
        active.cancelled.store(true, Ordering::Release);
        self.log(actor, "Requested native movement observation cancellation");
        Ok(self.linked_path_observation(false))
    }

    pub(crate) fn finish_linked_path_observation(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        let Completed { request, result } = completed;
        if request.cleanup == CleanupDisposition::Unconfirmed {
            self.native_cleanup_error = Some("Native movement cleanup is unconfirmed; inspect its retained artifacts before further native activity".into());
        }
        let context = if request.cancelled.load(Ordering::Acquire) {
            Err("Movement observation cancelled".to_owned())
        } else {
            request.frame.verify(self)
        };
        let result = match (result, context) {
            (Ok(receipt), Ok(())) => Ok(receipt),
            (Err(error), Err(context)) => Err(format!("{error}; {context}")),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        };
        let report = match result {
            Ok(receipt) => {
                json!({"status":"complete","token":request.token,"sample_count":receipt.samples.len(),"receipt":receipt})
            }
            Err(error) => {
                json!({"status":"failed","token":request.token,"error":error,"cleanup":request.cleanup})
            }
        };
        self.linked_path_observation.active = None;
        self.linked_path_observation.error = report["error"].as_str().map(str::to_owned);
        self.linked_path_observation.last = Some(Last {
            frame: request.frame.clone(),
            build: request.build.clone(),
            target: request.target.clone(),
            report: report.clone(),
        });
        self.log(
            actor,
            format!("Native movement observation {}", report["status"]),
        );
        drop(request);
        Ok(self.linked_path_observation(false))
    }

    pub(crate) fn fail_linked_path_observation_start(
        &mut self,
        token: &str,
        error: String,
        cleanup: CleanupDisposition,
    ) -> Result<Value, String> {
        if self
            .linked_path_observation
            .active
            .as_ref()
            .is_some_and(|active| active.token == token)
        {
            if cleanup == CleanupDisposition::Unconfirmed {
                self.native_cleanup_error = Some(error.clone());
            }
            self.linked_path_observation.active = None;
            self.linked_path_observation.error = Some(error.clone());
        }
        Err(error)
    }
}
