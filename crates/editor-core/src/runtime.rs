use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

use crate::game;
use crate::session::compilation::{CompileRequest, CompileResult};
use crate::session::{Actor, LaunchOrigin, LaunchRequest, Session};

mod document;
pub use document::DocumentHandle;

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
}

#[derive(Clone)]
pub(crate) struct CompileCompletion {
    session: SharedSession,
    actor: Actor,
    token: u64,
    play: bool,
}

impl CompileSubmission {
    pub(crate) fn begin(
        session: &SharedSession,
        actor: Actor,
        path: Option<&std::path::Path>,
        play: bool,
        expected_generation: Option<u64>,
    ) -> Result<Self, String> {
        let (request, status) = {
            let mut state = lock(session);
            if play && (state.game_running() || state.launching()) {
                return Err(
                    "CS:S is already launching or running; close it before running another map"
                        .into(),
                );
            }
            let request = state.begin_compile_for_generation(actor, path, expected_generation)?;
            (request, state.compile_status())
        };
        let completion = CompileCompletion {
            session: session.clone(),
            actor,
            token: request.token(),
            play,
        };
        Ok(Self {
            request,
            completion,
            status,
        })
    }

    pub(crate) fn status(&self) -> &Value {
        &self.status
    }

    pub(crate) fn completion(&self) -> CompileCompletion {
        self.completion.clone()
    }

    pub(crate) fn run(self) -> Result<(CompileResult, Option<game::Log>), String> {
        let result = self.request.run();
        let game = self.completion.finish_with(&result, LaunchRequest::run)?;
        Ok((result?, game))
    }
}

impl CompileCompletion {
    pub(crate) fn fail(&self, error: String) -> bool {
        lock(&self.session).finish_compile_for_token(self.actor, self.token, &Err(error))
    }

    fn finish_with(
        &self,
        result: &Result<CompileResult, String>,
        run: impl FnOnce(LaunchRequest) -> Result<game::Run, String>,
    ) -> Result<Option<game::Log>, String> {
        let launch_request = {
            let mut state = lock(&self.session);
            if !state.finish_compile_for_token(self.actor, self.token, result) {
                return Ok(None);
            }
            if let Some(error) = result
                .as_ref()
                .ok()
                .and_then(|result| result.publication_error())
            {
                return Err(error);
            }
            if self.play && result.as_ref().is_ok_and(|result| result.published()) {
                match state.begin_launch(self.actor) {
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
        launch_request
            .map(|request| {
                let origin = request.origin();
                let result = run(request);
                lock(&self.session).finish_launch(self.actor, &origin, result)
            })
            .transpose()
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
    let submission = CompileSubmission::begin(session, actor, path, play, expected_generation)?;
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
