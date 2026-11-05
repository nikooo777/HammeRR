use map_core::fog::PLAYER_UPDATE_INTERVAL_SECONDS;
use serde_json::Value;

use super::preview_clock::{INTERVAL, wait_for_restart};
use super::{
    DocumentAccess, DocumentContinuation, DocumentHandle, DocumentOperation, SharedSession, lock,
};
use crate::session::Actor;
use crate::session::fog::{ClockLease, Command, Overrides, Request, Update};

pub struct FogRenderGuard(crate::session::fog::RenderGuard);

impl FogRenderGuard {
    pub fn verify_original_sources(&self) -> Result<(), String> {
        self.0.verify_original_sources()
    }
}

fn run_request(session: &SharedSession, request: Request) -> Result<(), String> {
    let token = request.token().to_owned();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let completed = request.run();
        lock(session).finish_fog_preview(completed)
    })) {
        Ok(result) => result,
        Err(_) => lock(session).fail_fog_preview(&token, "Fog environment task panicked".into()),
    }
}

fn clock_loop(continuation: DocumentContinuation, lease: ClockLease) {
    while !lease.cancelled() {
        {
            match continuation.upgrade() {
                Ok(access) => {
                    let _ = access.step_fog_clock(&lease.scope);
                }
                Err(_) => return,
            }
        }
        std::thread::sleep(INTERVAL);
    }
}

fn ensure_clock(session: &SharedSession, continuation: DocumentContinuation) -> Result<(), String> {
    let scope = lock(session).fog_clock_scope();
    let result = wait_for_restart(|| {
        let mut session = lock(session);
        if !session.fog_clock_current(&scope) {
            return Err("Fog clock scope changed before restart".into());
        }
        if session.fog_clock_draining() {
            Ok(None)
        } else {
            Ok(Some(session.begin_fog_clock()))
        }
    });
    let Some(lease) = (match result {
        Ok(lease) => lease,
        Err(error) => {
            lock(session).fail_fog_clock(&scope, error.clone());
            return Err(error);
        }
    }) else {
        return Ok(());
    };
    let scope = lease.scope.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("fog-clock".into())
        .spawn(move || clock_loop(continuation, lease))
    {
        let error = format!("Could not start the fog preview clock: {error}");
        lock(session).fail_fog_clock(&scope, error.clone());
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
    if lock(session).fog_preview_snapshot() != expected {
        return Err("Fog source or controls changed; refresh Fog preview before applying".into());
    }
    if background
        && matches!(
            command,
            Command::Settings(_)
                | Command::Play
                | Command::Pause
                | Command::Cancel
                | Command::Clear
        )
    {
        return Err("Background is only available for environment preparation and timeline inputs, Seek, Advance or Reset".into());
    }
    let guard = {
        let session = lock(session);
        let settings = match command {
            Command::Settings(settings) => settings,
            _ => session.fog_preview_settings(),
        };
        if settings.enabled
            && settings.selection == crate::session::fog::SelectionMode::Environment
            && session.has_current_fog_environment()
            && matches!(
                command,
                Command::Settings(_)
                    | Command::Play
                    | Command::Input { .. }
                    | Command::SwitchPlayerController { .. }
                    | Command::Seek { .. }
                    | Command::Advance { .. }
                    | Command::Reset
            )
        {
            Some(session.fog_preview_settings_render_guard(settings)?)
        } else {
            None
        }
    };
    if let Some(guard) = guard {
        verify_internal(session, guard)?;
    }
    let needs_clock = match command {
        Command::Pause | Command::Cancel | Command::Clear => false,
        Command::Settings(settings) => settings.enabled,
        _ => true,
    };
    if needs_clock {
        ensure_clock(session, continuation)?;
    }
    let update = lock(session).fog_preview_command(actor, expected, command)?;
    match update {
        Update::State(report) => Ok(report),
        Update::Work(request) => {
            if !background {
                run_request(session, *request)?;
                return Ok(lock(session).fog_preview_state());
            }
            let token = request.token().to_owned();
            let worker = (request, session.clone(), operation);
            if let Err(error) = std::thread::Builder::new()
                .name("fog-environment".into())
                .spawn(move || {
                    let (request, session, _operation) = worker;
                    let _ = run_request(&session, *request);
                })
            {
                let error = format!("Could not start fog environment preparation: {error}");
                lock(session).fail_fog_preview(&token, error.clone())?;
                return Err(error);
            }
            Ok(lock(session).fog_preview_state())
        }
    }
}

pub(super) fn inspect(
    session: &SharedSession,
    _operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let guard = {
        let mut session = lock(session);
        session.preflight_fog_preview();
        if session.fog_verification_due() {
            Some(session.fog_preview_render_guard(Overrides::default())?)
        } else {
            None
        }
    };
    if let Some(guard) = guard {
        verify_internal(session, guard)?;
    }
    Ok(lock(session).fog_preview_state())
}

fn verify_internal(
    session: &SharedSession,
    guard: crate::session::fog::RenderGuard,
) -> Result<(), String> {
    let verified = guard.run();
    lock(session).finish_fog_render_guard(verified)
}

pub(super) fn render_guard(
    session: &SharedSession,
    overrides: Overrides,
    _operation: Option<DocumentOperation>,
) -> Result<FogRenderGuard, String> {
    let guard = FogRenderGuard(lock(session).fog_preview_render_guard(overrides)?);
    verify_guard(session, &guard, None)?;
    Ok(guard)
}

pub(super) fn verify_guard(
    session: &SharedSession,
    guard: &FogRenderGuard,
    _operation: Option<DocumentOperation>,
) -> Result<(), String> {
    verify_internal(session, guard.0.clone())
}

pub(super) fn tick(
    session: &SharedSession,
    scope: &str,
    _operation: Option<DocumentOperation>,
) -> Result<bool, String> {
    let (changed, guard) = {
        let mut session = lock(session);
        let changed = session.tick_fog_clock(scope, PLAYER_UPDATE_INTERVAL_SECONDS)?;
        let guard = if session.fog_verification_due() {
            Some(session.fog_preview_render_guard(Overrides::default())?)
        } else {
            None
        };
        (changed, guard)
    };
    if let Some(guard) = guard {
        verify_internal(session, guard)?;
    }
    Ok(changed)
}

impl DocumentHandle {
    pub fn fog_preview(
        &self,
        actor: Actor,
        expected_snapshot: &str,
        command: Command,
        background: bool,
    ) -> Result<Value, String> {
        DocumentAccess::for_document(self.clone()).fog_preview(
            actor,
            expected_snapshot,
            command,
            background,
        )
    }

    pub fn set_fog_presentation_active(&self, active: bool) -> Result<(), String> {
        DocumentAccess::for_document(self.clone()).set_fog_presentation_active(active)
    }
}
