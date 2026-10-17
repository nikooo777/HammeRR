use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{SharedSession, lock};
use crate::recovery;

const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct ShutdownGuard {
    session: SharedSession,
    drained: bool,
}

impl ShutdownGuard {
    fn drain(&mut self) {
        if self.drained {
            return;
        }
        lock(&self.session).shutdown();
        loop {
            let busy = {
                let session = lock(&self.session);
                session.launching()
                    || session.recovery_busy()
                    || session.compiling()
                    || session.instance_workers_busy()
                    || session.definitions_load_busy()
            };
            if !busy {
                break;
            }
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
        self.drained = true;
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.drain();
    }
}

struct DocumentShutdown {
    shutdown: ShutdownGuard,
    recovery_worker: recovery::Worker,
    finished: Option<Result<(), String>>,
}

impl DocumentShutdown {
    fn drain(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        self.shutdown.drain();
        let result = self.recovery_worker.drain().map(|_| ());
        self.finished = Some(result.clone());
        result
    }

    fn drain_on_drop(&mut self, context: &str) {
        if self.finished.is_none()
            && let Err(error) = self.drain()
        {
            eprintln!("{context}: {error}");
        }
    }
}

struct DocumentRuntime {
    shutdown: Mutex<DocumentShutdown>,
    session: SharedSession,
}

impl Drop for DocumentRuntime {
    fn drop(&mut self) {
        self.shutdown
            .get_mut()
            .expect("Document shutdown lock poisoned")
            .drain_on_drop("Document shutdown failed");
    }
}

#[derive(Clone)]
pub struct DocumentHandle(Arc<DocumentRuntime>);

impl DocumentHandle {
    pub(crate) fn start(session: SharedSession) -> Result<Self, String> {
        let recovery_worker = recovery::Worker::start(&session)?;
        Ok(Self(Arc::new(DocumentRuntime {
            shutdown: Mutex::new(DocumentShutdown {
                shutdown: ShutdownGuard {
                    session: session.clone(),
                    drained: false,
                },
                recovery_worker,
                finished: None,
            }),
            session,
        })))
    }

    pub fn session(&self) -> &SharedSession {
        &self.0.session
    }

    /// Call without a Session guard; draining waits for Session work to finish.
    pub fn shutdown(&self) -> Result<(), String> {
        self.0
            .shutdown
            .lock()
            .expect("Document shutdown lock poisoned")
            .drain()
    }

    pub(crate) fn shutdown_on_drop(&self) {
        self.0
            .shutdown
            .lock()
            .expect("Document shutdown lock poisoned")
            .drain_on_drop("Application shutdown failed");
    }
}

#[cfg(test)]
mod tests;
