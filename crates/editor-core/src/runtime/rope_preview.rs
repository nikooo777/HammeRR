use serde_json::Value;

use super::preview_clock::{INTERVAL, wait_for_restart};
use super::{
    DocumentAccess, DocumentContinuation, DocumentHandle, DocumentOperation, SharedSession, lock,
};
use crate::session::Actor;
use crate::session::ropes::{ClockLease, Command, Request, Update};

pub struct RopeRenderGuard(crate::session::ropes::RenderGuard);

impl RopeRenderGuard {
    pub fn verify_original_sources(&self) -> Result<(), String> {
        self.0.verify_original_sources()
    }
}

fn run_request(session: &SharedSession, request: Request) -> Result<(), String> {
    let token = request.token().to_owned();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let completed = request.run();
        lock(session).finish_rope_preview(completed)
    })) {
        Ok(result) => result,
        Err(_) => lock(session).fail_rope_preview(&token, "Rope preview task panicked".into()),
    }
}

fn clock_loop(continuation: DocumentContinuation, lease: ClockLease) {
    while !lease.cancelled() {
        {
            match continuation.upgrade() {
                Ok(access) => {
                    let _ = access.step_rope_clock(&lease.scope);
                }
                Err(_) => return,
            }
        }
        std::thread::sleep(INTERVAL);
    }
}

fn ensure_clock(session: &SharedSession, continuation: DocumentContinuation) -> Result<(), String> {
    let scope = lock(session).rope_clock_scope();
    let result = wait_for_restart(|| {
        let mut session = lock(session);
        if !session.rope_clock_current(&scope) {
            return Err("Rope clock scope changed before restart".into());
        }
        if session.rope_clock_draining() {
            Ok(None)
        } else {
            Ok(Some(session.begin_rope_clock()))
        }
    });
    let Some(lease) = (match result {
        Ok(lease) => lease,
        Err(error) => {
            lock(session).fail_rope_clock(&scope, error.clone());
            return Err(error);
        }
    }) else {
        return Ok(());
    };
    let scope = lease.scope.clone();
    let result = std::thread::Builder::new()
        .name("rope-clock".into())
        .spawn(move || clock_loop(continuation, lease));
    if let Err(error) = result {
        let error = format!("Could not start the rope preview clock: {error}");
        lock(session).fail_rope_clock(&scope, error.clone());
        return Err(error);
    }
    Ok(())
}

pub(super) fn execute(
    session: &SharedSession,
    continuation: DocumentContinuation,
    actor: Actor,
    expected: &str,
    command: Command,
    background: bool,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    if !matches!(command, Command::Pause | Command::Cancel) {
        inspect(session, None)?;
        ensure_clock(session, continuation)?;
    }
    let update = lock(session).rope_preview_command(actor, expected, command)?;
    match update {
        Update::State(report) => Ok(report),
        Update::Work(request) => {
            if !background {
                run_request(session, *request)?;
                return Ok(lock(session).rope_preview_state());
            }
            let token = request.token().to_owned();
            let shared = session.clone();
            let worker = (request, shared, operation);
            let spawned = std::thread::Builder::new()
                .name("rope-preview".into())
                .spawn(move || {
                    let (request, session, _operation) = worker;
                    let _ = run_request(&session, *request);
                });
            if let Err(error) = spawned {
                let error = format!("Could not start rope preview preparation: {error}");
                lock(session).fail_rope_preview(&token, error.clone())?;
                return Err(error);
            }
            Ok(lock(session).rope_preview_state())
        }
    }
}

pub(super) fn inspect(
    session: &SharedSession,
    _operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let request = lock(session).begin_rope_verification(false)?;
    if let Some(request) = request {
        run_request(session, request)?;
    }
    Ok(lock(session).rope_preview_state())
}

pub(super) fn verify(
    session: &SharedSession,
    _operation: Option<DocumentOperation>,
) -> Result<(), String> {
    render_guard(session, _operation).map(drop)
}

pub(super) fn render_guard(
    session: &SharedSession,
    _operation: Option<DocumentOperation>,
) -> Result<RopeRenderGuard, String> {
    let guard = RopeRenderGuard(lock(session).rope_preview_render_guard()?);
    verify_guard(session, &guard, None)?;
    Ok(guard)
}

pub(super) fn verify_guard(
    session: &SharedSession,
    guard: &RopeRenderGuard,
    _operation: Option<DocumentOperation>,
) -> Result<(), String> {
    let verified = guard.0.clone().run();
    lock(session).finish_rope_render_guard(verified)
}

pub(super) fn tick(
    session: &SharedSession,
    scope: &str,
    _operation: Option<DocumentOperation>,
) -> Result<bool, String> {
    let request = {
        let mut session = lock(session);
        if !session.rope_clock_current(scope) {
            return Err("Rope preview clock is expired".into());
        }
        session.begin_rope_tick(render::ropes::STEP_SECONDS)?
    };
    if let Some(request) = request {
        run_request(session, request)?;
        return Ok(true);
    }
    Ok(false)
}

impl DocumentHandle {
    pub fn rope_preview(
        &self,
        actor: Actor,
        expected_snapshot: &str,
        command: Command,
        background: bool,
    ) -> Result<Value, String> {
        DocumentAccess::for_document(self.clone()).rope_preview(
            actor,
            expected_snapshot,
            command,
            background,
        )
    }

    pub fn set_rope_presentation_active(&self, active: bool) -> Result<(), String> {
        DocumentAccess::for_document(self.clone()).set_rope_presentation_active(active)
    }
}
