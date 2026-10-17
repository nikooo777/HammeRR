use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use super::{SharedSession, lock};
use crate::recovery;

const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);
static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentLifecycle {
    Live,
    Draining,
    Finished,
    Failed(String),
}

struct Admission {
    lifecycle: Mutex<DocumentLifecycle>,
    operations: AtomicUsize,
}

struct Operation(Arc<Admission>);

impl Drop for Operation {
    fn drop(&mut self) {
        self.0.operations.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct DocumentOperation {
    _operation: Arc<Operation>,
}

impl Admission {
    fn admit(self: &Arc<Self>) -> Result<DocumentOperation, String> {
        let lifecycle = self
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned");
        match &*lifecycle {
            DocumentLifecycle::Live => {
                self.operations.fetch_add(1, Ordering::AcqRel);
                Ok(DocumentOperation {
                    _operation: Arc::new(Operation(self.clone())),
                })
            }
            DocumentLifecycle::Draining => Err("the editor is closing".into()),
            DocumentLifecycle::Finished => Err("the document is closed".into()),
            DocumentLifecycle::Failed(error) => Err(format!("document shutdown failed: {error}")),
        }
    }

    fn begin_drain(&self) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned");
        if *lifecycle == DocumentLifecycle::Live {
            *lifecycle = DocumentLifecycle::Draining;
        }
    }

    fn finish(&self, result: &Result<(), String>) {
        *self
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned") = match result {
            Ok(()) => DocumentLifecycle::Finished,
            Err(error) => DocumentLifecycle::Failed(error.clone()),
        };
    }
}

struct ShutdownGuard {
    session: SharedSession,
    admission: Arc<Admission>,
    drained: bool,
}

impl ShutdownGuard {
    fn drain(&mut self) {
        if self.drained {
            return;
        }
        self.admission.begin_drain();
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
            if !busy && self.admission.operations.load(Ordering::Acquire) == 0 {
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
        self.shutdown.admission.finish(&result);
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
    admission: Arc<Admission>,
    id: DocumentId,
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
        let id = NEXT_DOCUMENT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(DocumentId)
            .map_err(|_| "Document ID capacity exhausted".to_owned())?;
        let admission = Arc::new(Admission {
            lifecycle: Mutex::new(DocumentLifecycle::Live),
            operations: AtomicUsize::new(0),
        });
        let recovery_worker = recovery::Worker::start(&session)?;
        Ok(Self(Arc::new(DocumentRuntime {
            shutdown: Mutex::new(DocumentShutdown {
                shutdown: ShutdownGuard {
                    session: session.clone(),
                    admission: admission.clone(),
                    drained: false,
                },
                recovery_worker,
                finished: None,
            }),
            session,
            admission,
            id,
        })))
    }

    pub fn session(&self) -> &SharedSession {
        &self.0.session
    }

    pub fn id(&self) -> DocumentId {
        self.0.id
    }

    pub fn lifecycle(&self) -> DocumentLifecycle {
        self.0
            .admission
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned")
            .clone()
    }

    pub fn admit(&self) -> Result<DocumentOperation, String> {
        self.0.admission.admit()
    }

    #[cfg(test)]
    pub(crate) fn active_operations(&self) -> usize {
        self.0.admission.operations.load(Ordering::Acquire)
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

#[derive(Clone)]
pub struct DocumentAccess {
    document: Option<DocumentHandle>,
    session: SharedSession,
}

#[derive(Clone)]
pub struct DocumentContinuation {
    document: Option<Weak<DocumentRuntime>>,
    session: Weak<Mutex<crate::session::Session>>,
}

impl DocumentAccess {
    pub fn for_document(document: DocumentHandle) -> Self {
        Self {
            session: document.session().clone(),
            document: Some(document),
        }
    }

    pub fn unmanaged(session: SharedSession) -> Self {
        Self {
            document: None,
            session,
        }
    }

    pub fn with_shared<R>(
        &self,
        operation: impl FnOnce(&SharedSession) -> Result<R, String>,
    ) -> Result<R, String> {
        let _admission = self
            .document
            .as_ref()
            .map(DocumentHandle::admit)
            .transpose()?;
        operation(&self.session)
    }

    pub fn with_session<R>(
        &self,
        operation: impl FnOnce(&mut crate::session::Session) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_shared(|session| operation(&mut lock(session)))
    }

    pub fn downgrade(&self) -> DocumentContinuation {
        DocumentContinuation {
            document: self
                .document
                .as_ref()
                .map(|document| Arc::downgrade(&document.0)),
            session: Arc::downgrade(&self.session),
        }
    }
}

impl DocumentContinuation {
    pub fn upgrade(&self) -> Result<DocumentAccess, String> {
        match &self.document {
            Some(document) => document
                .upgrade()
                .map(DocumentHandle)
                .map(DocumentAccess::for_document),
            None => self.session.upgrade().map(DocumentAccess::unmanaged),
        }
        .ok_or_else(|| "the document is closed".into())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod admission_tests;
