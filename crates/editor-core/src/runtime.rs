use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

use crate::game;
use crate::session::{Actor, Session};

mod document;
pub use document::DocumentHandle;

pub type SharedSession = Arc<Mutex<Session>>;

pub fn lock(session: &Mutex<Session>) -> MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn launch(session: &SharedSession, actor: Actor) -> Result<game::Log, String> {
    let request = {
        let mut state = lock(session);
        match state.begin_launch(actor) {
            Ok(request) => request,
            Err(error) => {
                state.log(actor, format!("game launch failed: {error}"));
                return Err(error);
            }
        }
    };
    let result = request.run();
    lock(session).finish_launch(actor, result)
}

pub fn start_launch(session: &SharedSession, actor: Actor) -> Result<(), String> {
    let request = lock(session).begin_launch(actor)?;
    let worker_session = session.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("game-launch".into())
        .spawn(move || {
            let result = request.run();
            let _ = lock(&worker_session).finish_launch(actor, result);
        })
    {
        let error = error.to_string();
        let _ = lock(session).finish_launch(actor, Err(error.clone()));
        return Err(error);
    }
    Ok(())
}

pub(crate) fn finish_compile_worker(
    session: &SharedSession,
    actor: Actor,
    result: &Result<crate::session::compilation::CompileResult, String>,
    play: bool,
) -> Result<Option<game::Log>, String> {
    let launch_request = {
        let mut state = lock(session);
        state.finish_compile(actor, result);
        if let Some(error) = result
            .as_ref()
            .ok()
            .and_then(|result| result.publication_error())
        {
            return Err(error);
        }
        if play && result.as_ref().is_ok_and(|result| result.published()) {
            match state.begin_launch(actor) {
                Ok(request) => Some(request),
                Err(error) => {
                    state.log(actor, format!("game launch failed: {error}"));
                    return Err(error);
                }
            }
        } else {
            None
        }
    };
    launch_request
        .map(|request| {
            let result = request.run();
            lock(session).finish_launch(actor, result)
        })
        .transpose()
}

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
    let (request, status) = {
        let mut state = lock(session);
        if play && (state.game_running() || state.launching()) {
            return Err(
                "CS:S is already launching or running; close it before running another map".into(),
            );
        }
        let request = state.begin_compile_for_generation(actor, path, expected_generation)?;
        (request, state.compile_status())
    };
    let worker_session = session.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("map-compile".into())
        .spawn(move || {
            let result = request.run();
            let _ = finish_compile_worker(&worker_session, actor, &result, play);
        })
    {
        let error = error.to_string();
        lock(session).finish_compile(actor, &Err(error.clone()));
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
        let request = lock(&shared).begin_compile(Actor::Agent, None).unwrap();
        let token = lock(&shared).compile_token().unwrap();
        lock(&shared).cancel_compile(Actor::User, token).unwrap();
        let result = request.run();
        assert!(result.is_err());
        assert!(
            finish_compile_worker(&shared, Actor::Agent, &result, true)
                .unwrap()
                .is_none()
        );
        assert!(!lock(&shared).game_running());
        assert!(!lock(&shared).launching());
        assert!(!directory.path().join("cstrike/downloadlists").exists());
        let request = lock(&shared).begin_compile(Actor::User, None).unwrap();
        let result = request.run();
        assert!(
            finish_compile_worker(&shared, Actor::User, &result, true)
                .unwrap()
                .is_some()
        );
        assert!(lock(&shared).game_running());
        lock(&shared).stop_game(Actor::User).unwrap();
    }
}
