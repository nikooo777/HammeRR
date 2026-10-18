use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

use crate::game;
use crate::session::compilation::{CompileOrigin, CompileRequest, CompileResult};
use crate::session::{Actor, LaunchOrigin, LaunchRequest, Session};

mod compile_queue;
pub(crate) mod native_activity;
pub use compile_queue::CompileHandle;
pub(crate) use compile_queue::{CompileQueue, QueueClient, QueueStatus};

mod document;
pub use document::{
    DocumentAccess, DocumentCloseReview, DocumentContinuation, DocumentHandle, DocumentId,
    DocumentLifecycle, DocumentOperation,
};
mod recovery;
#[cfg(feature = "mcp")]
pub(crate) use recovery::RecoverySubmission;
pub use recovery::start_recovery_snapshot;

pub type SharedSession = Arc<Mutex<Session>>;

pub fn lock(session: &Mutex<Session>) -> MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct LaunchSubmission {
    request: Option<LaunchRequest>,
    session: SharedSession,
    actor: Actor,
    origin: LaunchOrigin,
}

#[derive(Clone)]
struct LaunchCompletion {
    session: SharedSession,
    actor: Actor,
    origin: LaunchOrigin,
}

impl LaunchCompletion {
    fn fail(&self, error: String) {
        lock(&self.session).fail_launch_for_origin(self.actor, &self.origin, error);
    }
}

#[cfg(any(feature = "mcp", test))]
pub(crate) struct LaunchCancellation(Option<LaunchOrigin>);

#[cfg(feature = "mcp")]
impl LaunchCancellation {
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

#[cfg(any(feature = "mcp", test))]
impl Drop for LaunchCancellation {
    fn drop(&mut self) {
        if let Some(origin) = &self.0 {
            origin.cancel();
        }
    }
}

impl LaunchSubmission {
    fn completion(&self) -> LaunchCompletion {
        LaunchCompletion {
            session: self.session.clone(),
            actor: self.actor,
            origin: self.origin.clone(),
        }
    }
    pub(crate) fn begin(session: &SharedSession, actor: Actor) -> Result<Self, String> {
        let request = lock(session).begin_launch(actor)?;
        let origin = request.origin();
        Ok(Self {
            request: Some(request),
            session: session.clone(),
            actor,
            origin,
        })
    }

    #[cfg(any(feature = "mcp", test))]
    pub(crate) fn cancellation(&self) -> LaunchCancellation {
        LaunchCancellation(Some(self.origin.clone()))
    }

    #[cfg(any(feature = "mcp", test))]
    pub(crate) fn run(self) -> Result<game::Log, String> {
        self.run_with(LaunchRequest::run)
    }

    fn run_with(
        mut self,
        run: impl FnOnce(LaunchRequest) -> Result<game::Run, String>,
    ) -> Result<game::Log, String> {
        let request = self
            .request
            .take()
            .expect("launch submission owns its request");
        let result = if self.origin.cancelled() {
            drop(request);
            Err("game launch cancelled".into())
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(request)))
                .unwrap_or_else(|_| Err("game launch worker panicked".into()))
        };
        lock(&self.session).finish_launch(self.actor, &self.origin, result)
    }
}

impl Drop for LaunchSubmission {
    fn drop(&mut self) {
        drop(self.request.take());
        self.origin.retire();
    }
}

pub fn launch(session: &SharedSession, actor: Actor) -> Result<game::Log, String> {
    launch_with(session, actor, LaunchRequest::run)
}

fn launch_with(
    session: &SharedSession,
    actor: Actor,
    run: impl FnOnce(LaunchRequest) -> Result<game::Run, String>,
) -> Result<game::Log, String> {
    let submission = match LaunchSubmission::begin(session, actor) {
        Ok(submission) => submission,
        Err(error) => {
            lock(session).log(actor, format!("game launch failed: {error}"));
            return Err(error);
        }
    };
    submission.run_with(run)
}

pub fn start_launch(session: &SharedSession, actor: Actor) -> Result<(), String> {
    start_launch_with(session, actor, LaunchRequest::run).map(drop)
}

fn start_launch_with(
    session: &SharedSession,
    actor: Actor,
    run: impl FnOnce(LaunchRequest) -> Result<game::Run, String> + Send + 'static,
) -> Result<std::thread::JoinHandle<()>, String> {
    let submission = LaunchSubmission::begin(session, actor)?;
    match std::thread::Builder::new()
        .name("game-launch".into())
        .spawn(move || {
            let _ = submission.run_with(run);
        }) {
        Ok(worker) => Ok(worker),
        Err(error) => {
            let error = error.to_string();
            lock(session).log(actor, format!("game launch failed: {error}"));
            Err(error)
        }
    }
}

pub(crate) struct CompileSubmission {
    request: CompileRequest,
    completion: CompileCompletion,
    status: Value,
    _operation: Option<DocumentOperation>,
}

#[derive(Clone)]
pub(crate) struct CompileCompletion {
    session: SharedSession,
    actor: Actor,
    origin: CompileOrigin,
    play: Option<Result<game::Install, String>>,
}

struct PreparedCompile {
    result: CompileResult,
    launch: Option<LaunchSubmission>,
    _operation: Option<DocumentOperation>,
}

struct CompileExecution {
    result: Result<CompileResult, String>,
    native: Option<native_activity::Permit>,
}

impl PreparedCompile {
    fn run(self) -> Result<(CompileResult, Option<game::Log>), String> {
        let game = self
            .launch
            .map(|launch| launch.run_with(LaunchRequest::run))
            .transpose()?;
        Ok((self.result, game))
    }
}

impl CompileSubmission {
    pub(crate) fn begin(
        session: &SharedSession,
        actor: Actor,
        path: Option<&std::path::Path>,
        play: bool,
        expected_generation: Option<u64>,
    ) -> Result<Self, String> {
        let (request, status, play) = {
            let mut state = lock(session);
            if play && (state.game_running() || state.launching()) {
                return Err(
                    "CS:S is already launching or running; close it before running another map"
                        .into(),
                );
            }
            let play = play.then(|| state.freeze_play_install());
            let mut request =
                state.begin_compile_for_generation(actor, path, expected_generation)?;
            request.freeze_native_play(play.as_ref().and_then(|install| install.as_ref().ok()));
            (request, state.compile_status(), play)
        };
        let completion = CompileCompletion {
            session: session.clone(),
            actor,
            origin: request.origin(),
            play,
        };
        Ok(Self {
            request,
            completion,
            status,
            _operation: None,
        })
    }

    pub(crate) fn status(&self) -> &Value {
        &self.status
    }

    pub(crate) fn completion(&self) -> CompileCompletion {
        self.completion.clone()
    }

    pub(crate) fn run(self) -> Result<(CompileResult, Option<game::Log>), String> {
        self.prepare()?.run()
    }

    fn prepare(self) -> Result<PreparedCompile, String> {
        self.prepare_with(CompileCompletion::prepare)
    }

    fn prepare_with(
        self,
        finish: impl FnOnce(
            &CompileCompletion,
            &Result<CompileResult, String>,
            &mut Option<native_activity::Permit>,
        ) -> Result<Option<LaunchSubmission>, String>,
    ) -> Result<PreparedCompile, String> {
        let (result, native) = self.request.run_admitted();
        let mut execution = CompileExecution { result, native };
        let launch = finish(&self.completion, &execution.result, &mut execution.native)?;
        Ok(PreparedCompile {
            result: execution.result?,
            launch,
            _operation: self._operation,
        })
    }
}

impl CompileCompletion {
    pub(crate) fn fail(&self, error: String) -> bool {
        lock(&self.session).finish_compile_for_origin(self.actor, &self.origin, &Err(error))
    }

    #[cfg(test)]
    fn finish_with(
        &self,
        result: &Result<CompileResult, String>,
        run: impl FnOnce(LaunchRequest) -> Result<game::Run, String>,
    ) -> Result<Option<game::Log>, String> {
        self.prepare(result, &mut None)?
            .map(|submission| submission.run_with(run))
            .transpose()
    }

    fn prepare(
        &self,
        result: &Result<CompileResult, String>,
        native: &mut Option<native_activity::Permit>,
    ) -> Result<Option<LaunchSubmission>, String> {
        let launch_request = {
            let mut state = lock(&self.session);
            if !state.finish_compile_for_origin(self.actor, &self.origin, result) {
                return Ok(None);
            }
            if let Some(error) = result
                .as_ref()
                .ok()
                .and_then(|result| result.publication_error())
            {
                return Err(error);
            }
            if let Some(install) = &self.play
                && result.as_ref().is_ok_and(|result| result.published())
            {
                match state.begin_launch_frozen(self.actor, install.clone(), native.take()) {
                    Ok(request) => Some(request),
                    Err(error) => {
                        state.log(self.actor, format!("game launch failed: {error}"));
                        return Err(error);
                    }
                }
            } else {
                None
            }
        };
        Ok(launch_request.map(|request| LaunchSubmission {
            origin: request.origin(),
            request: Some(request),
            session: self.session.clone(),
            actor: self.actor,
        }))
    }
}

#[cfg(test)]
mod compile_tests;
#[cfg(test)]
mod launch_submission_tests;
#[cfg(test)]
mod launch_tests;

pub fn start_compile(
    session: &SharedSession,
    actor: Actor,
    path: Option<&std::path::Path>,
    play: bool,
) -> Result<Value, String> {
    start_compile_for_generation(session, actor, path, play, None)
}

pub fn start_compile_for_generation(
    session: &SharedSession,
    actor: Actor,
    path: Option<&std::path::Path>,
    play: bool,
    expected_generation: Option<u64>,
) -> Result<Value, String> {
    start_compile_with_operation(session, actor, path, play, expected_generation, None)
}

pub(crate) fn start_compile_with_operation(
    session: &SharedSession,
    actor: Actor,
    path: Option<&std::path::Path>,
    play: bool,
    expected_generation: Option<u64>,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let mut submission = CompileSubmission::begin(session, actor, path, play, expected_generation)?;
    submission._operation = operation;
    let status = submission.status().clone();
    let completion = submission.completion();
    if let Err(error) = std::thread::Builder::new()
        .name("map-compile".into())
        .spawn(move || {
            let _ = submission.run();
        })
    {
        let error = error.to_string();
        completion.fail(error.clone());
        return Err(error);
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_compile_and_play_never_starts_the_game() {
        let (directory, game, _) = crate::test_assets::game_process(true);
        let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
        let vmf = directory.path().join("room.vmf");
        std::fs::write(&vmf, crate::test_assets::room().write(&Default::default())).unwrap();
        lock(&shared).open(Actor::Agent, &vmf, true).unwrap();
        let submission = CompileSubmission::begin(&shared, Actor::Agent, None, true, None).unwrap();
        let token = lock(&shared).compile_token().unwrap();
        lock(&shared).cancel_compile(Actor::User, token).unwrap();
        assert_eq!(
            lock(&shared).compile_status()["job"]["status"],
            "cancelling"
        );
        let result = submission.run();
        assert!(result.is_err());
        assert!(!lock(&shared).compiling());
        assert_eq!(lock(&shared).compile_status()["job"]["status"], "cancelled");
        assert!(!lock(&shared).game_running());
        assert!(!lock(&shared).launching());
        assert!(!directory.path().join("cstrike/downloadlists").exists());
        let submission = CompileSubmission::begin(&shared, Actor::User, None, true, None).unwrap();
        assert!(submission.run().unwrap().1.is_some());
        assert!(lock(&shared).game_running());
        lock(&shared).stop_game(Actor::User).unwrap();
    }
}
