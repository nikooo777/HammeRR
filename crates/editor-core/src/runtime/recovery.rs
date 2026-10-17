use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use super::{SharedSession, lock};
use crate::recovery::{Entry, Kind};
use crate::session::{Actor, RecoveryRequest};

const QUEUED: u8 = 0;
const STARTED: u8 = 1;
#[cfg(any(feature = "mcp", test))]
const CANCELLED: u8 = 2;

pub(crate) struct RecoverySubmission {
    request: Arc<RecoveryRequest>,
    session: SharedSession,
    actor: Actor,
    phase: Arc<AtomicU8>,
}

#[cfg(any(feature = "mcp", test))]
pub(crate) struct RecoveryCancellation(Option<Arc<AtomicU8>>);

#[cfg(feature = "mcp")]
impl RecoveryCancellation {
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

#[cfg(any(feature = "mcp", test))]
impl Drop for RecoveryCancellation {
    fn drop(&mut self) {
        if let Some(phase) = &self.0 {
            let _ = phase.compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

impl RecoverySubmission {
    pub(crate) fn begin(session: &SharedSession, actor: Actor) -> Result<Self, String> {
        let request = lock(session).begin_recovery_snapshot(Kind::Manual)?;
        Ok(Self {
            request: Arc::new(request),
            session: session.clone(),
            actor,
            phase: Arc::new(AtomicU8::new(QUEUED)),
        })
    }

    #[cfg(any(feature = "mcp", test))]
    pub(crate) fn cancellation(&self) -> RecoveryCancellation {
        RecoveryCancellation(Some(self.phase.clone()))
    }

    pub(crate) fn run(self) -> Result<Entry, String> {
        self.run_with(|| {})
    }

    fn run_with(self, before_write: impl FnOnce()) -> Result<Entry, String> {
        if self
            .phase
            .compare_exchange(QUEUED, STARTED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Recovery snapshot cancelled".into());
        }
        crate::recovery::complete_capture_with(
            &self.session,
            self.actor,
            &self.request,
            before_write,
        )
    }

    pub(crate) fn schedule_with<T>(
        self,
        schedule: impl FnOnce(Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let request = self.request.clone();
        let session = self.session.clone();
        let actor = self.actor;
        let scheduled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| schedule(self)))
            .unwrap_or_else(|_| Err("Recovery worker could not be scheduled".into()));
        if let Err(error) = &scheduled {
            let _ = lock(&session).finish_recovery_snapshot(actor, &request, Err(error.clone()));
        }
        scheduled
    }
}

pub fn start_recovery_snapshot(session: &SharedSession, actor: Actor) -> Result<(), String> {
    RecoverySubmission::begin(session, actor)?
        .schedule_with(|submission| {
            std::thread::Builder::new()
                .name("map-snapshot".into())
                .spawn(move || {
                    let _ = submission.run();
                })
                .map_err(|error| error.to_string())
        })
        .map(drop)
}

#[cfg(test)]
mod tests;
