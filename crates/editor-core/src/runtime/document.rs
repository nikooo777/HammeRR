use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

#[cfg(test)]
use super::CompileQueue;
use super::compile_queue::{CompileInput, QueueClient};
use super::{CompileHandle, SharedSession, lock};
use crate::recovery;
use crate::session::Actor;
use serde_json::Value;

const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(feature = "mcp")]
const MAX_CLOSE_TOKEN_BYTES: usize = 128;
static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DocumentId(u64);

impl DocumentId {
    #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
    pub(crate) fn allocate() -> Result<Self, String> {
        NEXT_DOCUMENT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(Self)
            .map_err(|_| "Document ID capacity exhausted".to_owned())
    }

    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentLifecycle {
    Live,
    Draining,
    Finished,
    Failed(String),
}

#[derive(Default)]
struct Operations {
    next: u64,
    active: BTreeMap<u64, String>,
}

struct Admission {
    lifecycle: Mutex<DocumentLifecycle>,
    operations: Mutex<Operations>,
}

struct Operation {
    admission: Arc<Admission>,
    id: u64,
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.admission
            .operations
            .lock()
            .expect("Document operations lock poisoned")
            .active
            .remove(&self.id)
            .expect("Document operation missing");
    }
}

#[derive(Clone)]
pub struct DocumentOperation {
    _operation: Arc<Operation>,
}

impl Admission {
    fn admit(self: &Arc<Self>, activity: &str) -> Result<DocumentOperation, String> {
        let lifecycle = self
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned");
        match &*lifecycle {
            DocumentLifecycle::Live => {
                let mut operations = self
                    .operations
                    .lock()
                    .expect("Document operations lock poisoned");
                let id = operations
                    .next
                    .checked_add(1)
                    .ok_or("Document operation capacity exhausted")?;
                operations.next = id;
                operations.active.insert(id, activity.to_owned());
                Ok(DocumentOperation {
                    _operation: Arc::new(Operation {
                        admission: self.clone(),
                        id,
                    }),
                })
            }
            DocumentLifecycle::Draining => Err("the editor is closing".into()),
            DocumentLifecycle::Finished => Err("the document is closed".into()),
            DocumentLifecycle::Failed(error) => Err(format!("document shutdown failed: {error}")),
        }
    }

    fn activities(&self, exclude: Option<u64>) -> Vec<(String, usize)> {
        let operations = self
            .operations
            .lock()
            .expect("Document operations lock poisoned");
        let mut activities = BTreeMap::new();
        for (id, name) in &operations.active {
            if Some(*id) != exclude {
                *activities.entry(name.clone()).or_insert(0) += 1;
            }
        }
        activities.into_iter().collect()
    }

    fn count(&self) -> usize {
        self.operations
            .lock()
            .expect("Document operations lock poisoned")
            .active
            .len()
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
    queue: Option<(QueueClient, DocumentId)>,
}

impl ShutdownGuard {
    fn drain(&mut self) {
        if self.drained {
            return;
        }
        self.admission.begin_drain();
        lock(&self.session).shutdown();
        if let Some((queue, document)) = &self.queue {
            queue.cancel_document(*document);
        }
        loop {
            let busy = {
                let session = lock(&self.session);
                session.launching()
                    || session.recovery_busy()
                    || session.compiling()
                    || session.instance_workers_busy()
                    || session.definitions_load_busy()
            };
            if !busy && self.admission.count() == 0 {
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
        let recovery = self.recovery_worker.drain().map(|_| ());
        let native = lock(&self.shutdown.session).native_cleanup_result();
        let result = match (recovery, native) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(recovery), Err(native)) => Err(format!(
                "{recovery}; native activity cleanup failed: {native}"
            )),
        };
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
    queue: Option<QueueClient>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentCloseReview {
    document: DocumentId,
    session: crate::session::DocumentCloseReview,
    activities: Vec<(String, usize)>,
}

impl DocumentCloseReview {
    pub(crate) fn token(&self) -> String {
        format!(
            "document-close-v1:{}:{}",
            self.document.value(),
            self.session.token()
        )
    }

    pub fn requires_discard(&self) -> bool {
        self.session.requires_discard()
    }

    pub fn inspection(&self) -> Value {
        let mut result = self.session.inspection();
        result["document_id"] = serde_json::json!(self.document.value());
        result["review_token"] = serde_json::json!(self.token());
        result["admission"] = admission_inspection(&self.activities);
        result
    }
}

fn admission_inspection(activities: &[(String, usize)]) -> Value {
    serde_json::json!({"active_operations": activities.iter().map(|(_, count)| count).sum::<usize>(),
        "activities": activities.iter().map(|(activity, count)| serde_json::json!({"activity":activity,"count":count})).collect::<Vec<_>>()})
}

impl DocumentHandle {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn start(session: SharedSession) -> Result<Self, String> {
        Self::start_with_queue(session, None)
    }

    #[cfg(test)]
    pub(crate) fn start_managed(
        session: SharedSession,
        queue: &CompileQueue,
    ) -> Result<Self, String> {
        Self::start_with_services(session, Some(queue.client()), Some(queue.native()))
    }

    #[cfg(any(test, feature = "test-support"))]
    fn start_with_queue(
        session: SharedSession,
        queue: Option<QueueClient>,
    ) -> Result<Self, String> {
        Self::start_with_services(session, queue, None)
    }

    pub(crate) fn start_with_services(
        session: SharedSession,
        queue: Option<QueueClient>,
        native: Option<&super::native_activity::Authority>,
    ) -> Result<Self, String> {
        let id = DocumentId::allocate()?;
        if let Some(native) = native {
            lock(&session).use_application_native_activity(native.client(id));
        }
        let admission = Arc::new(Admission {
            lifecycle: Mutex::new(DocumentLifecycle::Live),
            operations: Mutex::default(),
        });
        let recovery_worker = recovery::Worker::start(&session)?;
        Ok(Self(Arc::new(DocumentRuntime {
            shutdown: Mutex::new(DocumentShutdown {
                shutdown: ShutdownGuard {
                    session: session.clone(),
                    admission: admission.clone(),
                    drained: false,
                    queue: queue.clone().map(|queue| (queue, id)),
                },
                recovery_worker,
                finished: None,
            }),
            session,
            admission,
            id,
            queue,
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
        self.admit_named("Document operation")
    }

    pub(crate) fn admit_named(&self, activity: &str) -> Result<DocumentOperation, String> {
        self.0.admission.admit(activity)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn admission_inspection(&self) -> Value {
        admission_inspection(&self.0.admission.activities(None))
    }

    pub fn document_close_review(&self) -> Result<DocumentCloseReview, String> {
        let operation = self.admit_named("Close inspection")?;
        Ok(self.close_review_excluding(&lock(self.session()), Some(&operation)))
    }

    pub(crate) fn close_review_for(
        &self,
        session: &crate::session::Session,
    ) -> DocumentCloseReview {
        self.close_review_excluding(session, None)
    }

    pub(crate) fn close_review_excluding(
        &self,
        session: &crate::session::Session,
        operation: Option<&DocumentOperation>,
    ) -> DocumentCloseReview {
        let exclude = operation
            .filter(|operation| Arc::ptr_eq(&operation._operation.admission, &self.0.admission))
            .map(|operation| operation._operation.id);
        DocumentCloseReview {
            document: self.id(),
            session: session.document_close_review(),
            activities: self.0.admission.activities(exclude),
        }
    }

    pub fn try_commit_reviewed_close(
        &self,
        review: &DocumentCloseReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        self.commit_close(Some(review), None, discard_changes, || Ok(()))
    }

    #[cfg(any(test, feature = "mcp"))]
    pub(crate) fn commit_reviewed_close_set<R>(
        targets: &[(Self, DocumentCloseReview)],
        discard_changes: bool,
        reserve: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        if targets
            .windows(2)
            .any(|pair| pair[0].0.id() >= pair[1].0.id())
        {
            return Err("Quit all requires distinct documents in stable identity order".into());
        }
        let mut shutdowns = Vec::with_capacity(targets.len());
        for (document, _) in targets {
            shutdowns.push(document.0.shutdown.try_lock().map_err(|_| {
                "A document shutdown is busy; wait and review Quit all again".to_owned()
            })?);
        }
        let mut sessions = Vec::with_capacity(targets.len());
        for (document, _) in targets {
            sessions.push(
                document
                    .session()
                    .try_lock()
                    .map_err(|_| "A document is busy; wait and review Quit all again".to_owned())?,
            );
        }
        let mut lifecycles = Vec::with_capacity(targets.len());
        for (document, _) in targets {
            lifecycles.push(
                document
                    .0
                    .admission
                    .lifecycle
                    .lock()
                    .expect("Document admission lock poisoned"),
            );
        }
        for (((document, review), session), lifecycle) in
            targets.iter().zip(&sessions).zip(&lifecycles)
        {
            document.ensure_reviewed_close(session, review, discard_changes, lifecycle)?;
            if review != &document.close_review_for(session) {
                return Err("Map, draft or activity changed since Quit all was reviewed; review the workspace and try again".into());
            }
        }
        let reserved = reserve()?;
        for lifecycle in &mut lifecycles {
            **lifecycle = DocumentLifecycle::Draining;
        }
        drop(lifecycles);
        for session in &mut sessions {
            session.commit_terminal_closing();
        }
        Ok(reserved)
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn commit_close_token<R>(
        &self,
        token: &str,
        discard_changes: bool,
        reserve: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        if token.len() > MAX_CLOSE_TOKEN_BYTES {
            return Err("The Close review token exceeds its limit".into());
        }
        self.commit_close(None, Some(token), discard_changes, reserve)
    }

    fn commit_close<R>(
        &self,
        review: Option<&DocumentCloseReview>,
        token: Option<&str>,
        discard_changes: bool,
        reserve: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        if review.is_some_and(|review| review.document != self.id()) {
            return Err(
                "Close was reviewed for another document; review this document and try again"
                    .into(),
            );
        }
        let _shutdown = self
            .0
            .shutdown
            .lock()
            .expect("Document shutdown lock poisoned");
        let state = self.lifecycle();
        match state {
            DocumentLifecycle::Live => {}
            DocumentLifecycle::Draining => return Err("the editor is closing".into()),
            DocumentLifecycle::Finished => return Err("the document is closed".into()),
            DocumentLifecycle::Failed(error) => {
                return Err(format!("document shutdown failed: {error}"));
            }
        }
        let mut session = lock(self.session());
        let current;
        let review = match review {
            Some(review) => review,
            None => {
                current = self.close_review_for(&session);
                if token != Some(current.token().as_str()) {
                    return Err("Map, draft or activity changed since Close was reviewed; review the current document and try again".into());
                }
                &current
            }
        };
        let mut lifecycle = self
            .0
            .admission
            .lifecycle
            .lock()
            .expect("Document admission lock poisoned");
        self.ensure_reviewed_close(&session, review, discard_changes, &lifecycle)?;
        let reserved = reserve()?;
        *lifecycle = DocumentLifecycle::Draining;
        drop(lifecycle);
        session.commit_terminal_closing();
        Ok(reserved)
    }

    fn ensure_reviewed_close(
        &self,
        session: &crate::session::Session,
        review: &DocumentCloseReview,
        discard_changes: bool,
        lifecycle: &DocumentLifecycle,
    ) -> Result<(), String> {
        session.ensure_document_close_review(&review.session, discard_changes)?;
        if lifecycle != &DocumentLifecycle::Live {
            return Err("The document is already closing or closed".into());
        }
        let activities = self.0.admission.activities(None);
        if !activities.is_empty() {
            let names = activities
                .iter()
                .map(|(name, count)| format!("{name} ({count})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Close is busy with an admitted request or worker: {names}. Wait for it to finish, or cancel it using its normal control, then review Close again"
            ));
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn commit_reviewed_close_with<R>(
        &self,
        review: &DocumentCloseReview,
        discard_changes: bool,
        reserve: impl FnOnce() -> Result<R, String>,
    ) -> Result<R, String> {
        self.commit_close(Some(review), None, discard_changes, reserve)
    }

    #[cfg(test)]
    pub(crate) fn active_operations(&self) -> usize {
        self.0.admission.count()
    }

    /// Call without a Session guard; draining waits for Session work to finish.
    pub fn shutdown(&self) -> Result<(), String> {
        self.0
            .shutdown
            .lock()
            .expect("Document shutdown lock poisoned")
            .drain()
    }

    pub fn finalize_closed_claims(&self) -> Result<(), String> {
        let shutdown = self
            .0
            .shutdown
            .lock()
            .expect("Document shutdown lock poisoned");
        match &shutdown.finished {
            Some(Ok(())) => {
                lock(self.session()).release_closed_claims();
                Ok(())
            }
            Some(Err(error)) => Err(error.clone()),
            None => Err("The document has not completed physical shutdown".into()),
        }
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
    pub fn finish_reviewed_close(&self) -> Result<(), String> {
        let document = self
            .document
            .as_ref()
            .ok_or_else(|| "An owning document is required for reviewed Close".to_owned())?;
        if document.lifecycle() == DocumentLifecycle::Live {
            return Err("Close has not been accepted".into());
        }
        document.shutdown()
    }

    pub fn document_close_review(&self) -> Result<DocumentCloseReview, String> {
        self.document
            .as_ref()
            .ok_or_else(|| "An owning document is required for reviewed Close".to_owned())?
            .document_close_review()
    }

    pub fn try_commit_reviewed_close(
        &self,
        review: &DocumentCloseReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        self.document
            .as_ref()
            .ok_or_else(|| "An owning document is required for reviewed Close".to_owned())?
            .try_commit_reviewed_close(review, discard_changes)
    }

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

    pub fn start_sky3d_preview(&self, actor: Actor) -> Result<Value, String> {
        let operation = self
            .document
            .as_ref()
            .map(|document| document.admit_named("3D sky preview"))
            .transpose()?;
        let request = lock(&self.session).begin_sky3d_preview(actor)?;
        let token = request.token();
        let worker = (request, self.session.clone(), operation);
        let spawned = std::thread::Builder::new()
            .name("sky3d-preview".into())
            .spawn(move || {
                let (request, session, _operation) = worker;
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request.run()));
                match result {
                    Ok(completed) => {
                        let _ = lock(&session).finish_sky3d_preview(actor, completed);
                    }
                    Err(_) => {
                        let _ = lock(&session)
                            .fail_sky3d_preview(token, "3D sky preparation worker panicked".into());
                    }
                }
            });
        if let Err(error) = spawned {
            return lock(&self.session).fail_sky3d_preview(
                token,
                format!("Could not start 3D sky preparation: {error}"),
            );
        }
        Ok(lock(&self.session).sky3d_preview_state())
    }

    pub fn start_compile(
        &self,
        actor: Actor,
        path: Option<&std::path::Path>,
        play: bool,
        generation: Option<u64>,
    ) -> Result<Value, String> {
        if self.managed() {
            self.submit_compile(actor, path, play, generation)
                .map(|handle| handle.status().clone())
        } else {
            let operation = self
                .document
                .as_ref()
                .map(DocumentHandle::admit)
                .transpose()?;
            super::start_compile_with_operation(
                &self.session,
                actor,
                path,
                play,
                generation,
                operation,
            )
        }
    }

    pub fn submit_compile(
        &self,
        actor: Actor,
        path: Option<&std::path::Path>,
        play: bool,
        generation: Option<u64>,
    ) -> Result<CompileHandle, String> {
        let document = self
            .document
            .as_ref()
            .ok_or("An owning document is required for queued compilation")?;
        let operation = document.admit_named("Compile")?;
        let queue = document
            .0
            .queue
            .as_ref()
            .ok_or("This document uses the explicit unmanaged runtime")?;
        queue.submit(
            document.id(),
            &self.session,
            operation,
            CompileInput {
                actor,
                path,
                play,
                generation,
            },
        )
    }

    pub fn cancel_compile(&self, actor: Actor, token: u64) -> Result<Value, String> {
        let _operation = self
            .document
            .as_ref()
            .map(DocumentHandle::admit)
            .transpose()?;
        let origin = {
            let mut session = lock(&self.session);
            session.cancel_compile(actor, token)?;
            session.compile_origin(token)?
        };
        if let Some(document) = &self.document
            && let Some(queue) = &document.0.queue
        {
            queue.cancel_queued(document.id(), &origin);
        }
        Ok(lock(&self.session).compile_status())
    }

    pub(crate) fn managed(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(|document| document.0.queue.is_some())
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

#[cfg(test)]
mod close_tests;
