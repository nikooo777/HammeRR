use super::*;
use crate::runtime::{DocumentId, DocumentLifecycle, QueueClient};
use crate::session::SessionContent;
use std::fmt;
use std::path::Path;
use std::sync::{Condvar, PoisonError, Weak};

mod save_all;
pub use save_all::SaveAllRequest;

pub const DOCUMENT_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentVisibility {
    Headless,
    Hidden,
    Visible,
}

impl DocumentVisibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Hidden => "hidden",
            Self::Visible => "visible",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceDocumentState {
    Live,
    Closing,
    Failed,
}

impl WorkspaceDocumentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Closing => "closing",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone)]
pub struct WorkspaceDocument {
    document: DocumentHandle,
    selector: String,
    visibility: DocumentVisibility,
    state: WorkspaceDocumentState,
    error: Option<String>,
}

impl WorkspaceDocument {
    pub fn document(&self) -> DocumentHandle {
        self.document.clone()
    }

    pub fn selector(&self) -> &str {
        &self.selector
    }

    pub fn visibility(&self) -> DocumentVisibility {
        self.visibility
    }

    pub fn state(&self) -> WorkspaceDocumentState {
        self.state
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl fmt::Debug for WorkspaceDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceDocument")
            .field("selector", &self.selector)
            .field("state", &self.state)
            .field("visibility", &self.visibility)
            .field("error", &self.error)
            .finish()
    }
}

pub(super) struct Services {
    pub content: SessionContent,
    pub definitions: Result<Catalog, String>,
    pub directory: Result<PathBuf, String>,
    pub engine_root: Option<PathBuf>,
    pub steam_root: Option<PathBuf>,
    pub queue: Option<QueueClient>,
    pub native: Option<crate::runtime::native_activity::Authority>,
    pub path_claims: crate::path_claims::Authority,
    pub material_favorites: material_favorites::Authority,
    pub color_palette: crate::color_palette::Authority,
    pub recent_maps: recent_maps::Authority,
    pub shortcuts: shortcuts::Authority,
    pub saved_defaults: crate::saved_defaults::SavedDefaults,
    pub build_recipes: build_recipes::Authority,
    pub recovery_settings: recovery::Authority,
    pub prefabs: prefabs::Authority,
}

impl Services {
    fn document(&self) -> Result<DocumentHandle, String> {
        let mut session = Session::with_shared_content(self.content.clone());
        session.use_application_path_claims(self.path_claims.clone());
        session.use_application_saved_defaults(self.saved_defaults.clone());
        if self.engine_root.is_some() || self.steam_root.is_some() {
            session.configure_native_capture_runtime(
                Actor::System,
                self.engine_root.as_deref(),
                self.steam_root.as_deref(),
            )?;
        }
        session.use_application_material_favorites(self.material_favorites.clone());
        session.use_application_color_palette(self.color_palette.clone());
        session.use_application_recent_maps(self.recent_maps.clone());
        session.use_application_shortcuts(self.shortcuts.clone());
        session.use_application_build_recipes(self.build_recipes.clone());
        session.use_application_recovery_settings(self.recovery_settings.clone());
        session.use_application_prefabs(self.prefabs.clone());
        session.configure_recovery(self.directory.clone());
        match &self.definitions {
            Ok(catalog) => session.set_definitions(Actor::User, catalog.clone()),
            Err(error) => session.report_definitions_error(error.clone()),
        }
        DocumentHandle::start_with_services(
            Arc::new(Mutex::new(session)),
            self.queue.clone(),
            self.native.as_ref(),
        )
    }
}

struct State {
    accepting: bool,
    serial: u64,
    membership: Option<u64>,
    documents: Vec<WorkspaceDocument>,
}

struct Core {
    nonce: String,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone)]
pub struct Workspace {
    core: Arc<Core>,
    services: Weak<Services>,
}

impl Workspace {
    pub(super) fn new(services: &Arc<Services>) -> Self {
        Self {
            core: Arc::new(Core {
                nonce: recovery::unique_id(),
                state: Mutex::new(State {
                    accepting: true,
                    serial: 0,
                    membership: None,
                    documents: Vec::new(),
                }),
                changed: Condvar::new(),
            }),
            services: Arc::downgrade(services),
        }
    }

    pub fn documents(&self) -> Vec<WorkspaceDocument> {
        self.core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .documents
            .clone()
    }

    pub fn busy(&self) -> bool {
        self.core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .membership
            .is_some()
    }

    pub fn can_create(&self) -> bool {
        let state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.accepting && state.membership.is_none() && state.documents.len() < DOCUMENT_LIMIT
    }

    pub fn resolve(&self, selector: Option<&str>) -> Result<WorkspaceDocument, String> {
        let entry = {
            let state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.accepting {
                return Err("The application is closing".into());
            }
            match selector {
                Some(selector) => state
                    .documents
                    .iter()
                    .find(|entry| entry.selector == selector)
                    .cloned()
                    .ok_or("Unknown or closed document; use list_documents")?,
                None => {
                    let mut live = state
                        .documents
                        .iter()
                        .filter(|entry| entry.state == WorkspaceDocumentState::Live);
                    let first = live.next().cloned();
                    if live.next().is_some() {
                        return Err("More than one document is open; provide document_id from list_documents".into());
                    }
                    first.ok_or("No live document is open; use list_documents")?
                }
            }
        };
        if entry.state != WorkspaceDocumentState::Live {
            return Err(entry
                .error
                .clone()
                .unwrap_or_else(|| "The document is closing".into()));
        }
        match entry.document.lifecycle() {
            DocumentLifecycle::Live => Ok(entry),
            DocumentLifecycle::Draining => Err("The document is closing".into()),
            DocumentLifecycle::Finished => Err("The document is closed".into()),
            DocumentLifecycle::Failed(error) => Err(format!("Document shutdown failed: {error}")),
        }
    }

    pub fn prepare_new(&self, actor: Actor) -> Result<PreparedDocument, String> {
        let membership = self.reserve_create()?;
        let services = self
            .services
            .upgrade()
            .ok_or("The application owner has finished")?;
        let document = services.document()?;
        let selector = format!("{}:{}", self.core.nonce, document.id().value());
        lock(document.session()).log(actor, "New document");
        Ok(PreparedDocument {
            membership,
            document: Some(document),
            selector,
            open_message: None,
            remember: None,
        })
    }

    pub fn prepare_open(&self, actor: Actor, path: &Path) -> Result<PreparedDocument, String> {
        let mut prepared = self.prepare_new(actor)?;
        prepared.open(actor, path)?;
        Ok(prepared)
    }

    fn reserve_create(&self) -> Result<Membership, String> {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure_available(&state)?;
        if state.documents.len() >= DOCUMENT_LIMIT {
            return Err(format!(
                "The workspace is full ({DOCUMENT_LIMIT} documents)"
            ));
        }
        Membership::reserve(self.core.clone(), &mut state)
    }

    pub fn set_visibility(
        &self,
        document: DocumentId,
        visibility: DocumentVisibility,
    ) -> Result<(), String> {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !state.accepting {
            return Err("The application is closing".into());
        }
        let membership_busy = state.membership.is_some();
        let entry = state
            .documents
            .iter_mut()
            .find(|entry| entry.document.id() == document)
            .ok_or("Unknown or closed document")?;
        if entry.state != WorkspaceDocumentState::Live {
            return Err("The document is closing or closed".into());
        }
        if membership_busy
            && entry.visibility == DocumentVisibility::Visible
            && visibility != DocumentVisibility::Visible
        {
            return Err(
                "Wait for the document creation or close before hiding this document".into(),
            );
        }
        entry.visibility = visibility;
        Ok(())
    }

    pub fn reserve_close(&self, selector: &str) -> Result<CloseReservation, String> {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure_available(&state)?;
        let entry = state
            .documents
            .iter()
            .find(|entry| entry.selector == selector)
            .cloned()
            .ok_or("Unknown or closed document")?;
        if entry.state != WorkspaceDocumentState::Live {
            return Err(entry
                .error
                .clone()
                .unwrap_or_else(|| "The document is already closing".into()));
        }
        let peers: Vec<_> = state
            .documents
            .iter()
            .filter(|peer| {
                peer.document.id() != entry.document.id()
                    && peer.state == WorkspaceDocumentState::Live
            })
            .collect();
        let disposition = if peers.is_empty() {
            CloseDisposition::Terminal
        } else {
            if entry.visibility == DocumentVisibility::Visible
                && peers
                    .iter()
                    .all(|peer| peer.visibility == DocumentVisibility::Hidden)
            {
                return Err(
                    "Show the other document before closing the last visible document".into(),
                );
            }
            CloseDisposition::DocumentOnly
        };
        let membership = Membership::reserve(self.core.clone(), &mut state)?;
        Ok(CloseReservation {
            membership,
            entry,
            disposition,
            committed: false,
            finished: None,
        })
    }

    pub fn close_admission(&self) {
        self.core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepting = false;
    }

    pub(super) fn shutdown(&self) -> Result<(), String> {
        let documents = {
            let mut state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.accepting = false;
            while state.membership.is_some() {
                state = self
                    .core
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.documents.clone()
        };
        let mut errors = Vec::new();
        for entry in documents {
            if let Some(error) = entry.error {
                errors.push(format!("{}: {error}", entry.selector));
            }
            if let Err(error) = entry.document.shutdown() {
                errors.push(format!("{}: {error}", entry.selector));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn ensure_available(state: &State) -> Result<(), String> {
    if !state.accepting {
        Err("The application is closing".into())
    } else if state.membership.is_some() {
        Err("Another workspace document operation is in progress; wait and retry".into())
    } else {
        Ok(())
    }
}

struct Membership {
    core: Arc<Core>,
    key: u64,
    released: bool,
}

impl Membership {
    fn reserve(core: Arc<Core>, state: &mut State) -> Result<Self, String> {
        state.serial = state
            .serial
            .checked_add(1)
            .ok_or("Workspace command identity exhausted")?;
        let key = state.serial;
        state.membership = Some(key);
        Ok(Self {
            core,
            key,
            released: false,
        })
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.membership == Some(self.key) {
            state.membership = None;
        }
        self.released = true;
        drop(state);
        self.core.changed.notify_all();
    }
}

impl Drop for Membership {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct PreparedDocument {
    membership: Membership,
    document: Option<DocumentHandle>,
    selector: String,
    open_message: Option<String>,
    remember: Option<PathBuf>,
}

impl PreparedDocument {
    pub fn document(&self) -> DocumentHandle {
        self.document
            .as_ref()
            .expect("Prepared document was already published")
            .clone()
    }

    pub fn selector(&self) -> &str {
        &self.selector
    }

    pub fn open_message(&self) -> Option<&str> {
        self.open_message.as_deref()
    }

    pub(super) fn open(&mut self, actor: Actor, path: &Path) -> Result<(), String> {
        let message = lock(self.document().session()).open_workspace_candidate(actor, path)?;
        self.open_message = Some(message);
        self.remember = Some(path.to_owned());
        Ok(())
    }

    pub fn publish(mut self, visibility: DocumentVisibility) -> Result<WorkspaceDocument, String> {
        self.try_publish(visibility)
    }

    pub fn try_publish(
        &mut self,
        visibility: DocumentVisibility,
    ) -> Result<WorkspaceDocument, String> {
        let document = self
            .document
            .as_ref()
            .ok_or("The prepared document was already published")?
            .clone();
        let entry = WorkspaceDocument {
            document: document.clone(),
            selector: self.selector.clone(),
            visibility,
            state: WorkspaceDocumentState::Live,
            error: None,
        };
        {
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.accepting || state.membership != Some(self.membership.key) {
                return Err("The application closed during document preparation".into());
            }
            state.documents.push(entry.clone());
        }
        self.document = None;
        if let Some(path) = &self.remember {
            lock(document.session()).remember_recent_map(path);
        }
        self.membership.release();
        Ok(entry)
    }

    pub fn fail(
        mut self,
        error: String,
        visibility: DocumentVisibility,
    ) -> Result<WorkspaceDocument, String> {
        let document = self.document();
        let error = match document.shutdown() {
            Ok(()) => error,
            Err(drain) => format!("{error}; document cleanup failed: {drain}"),
        };
        let entry = WorkspaceDocument {
            document,
            selector: self.selector.clone(),
            visibility,
            state: WorkspaceDocumentState::Failed,
            error: Some(error),
        };
        {
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            assert_eq!(state.membership, Some(self.membership.key));
            state.documents.push(entry.clone());
        }
        self.document = None;
        self.membership.release();
        Ok(entry)
    }
}

impl Drop for PreparedDocument {
    fn drop(&mut self) {
        if let Some(document) = self.document.take()
            && let Err(error) = document
                .shutdown()
                .and_then(|()| document.finalize_closed_claims())
        {
            eprintln!("Unpublished document cleanup failed: {error}");
            self.membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .documents
                .push(WorkspaceDocument {
                    document,
                    selector: self.selector.clone(),
                    visibility: DocumentVisibility::Hidden,
                    state: WorkspaceDocumentState::Failed,
                    error: Some(error),
                });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDisposition {
    DocumentOnly,
    Terminal,
}

pub struct CloseReservation {
    membership: Membership,
    entry: WorkspaceDocument,
    disposition: CloseDisposition,
    committed: bool,
    finished: Option<Result<(), String>>,
}

impl CloseReservation {
    pub fn document(&self) -> DocumentHandle {
        self.entry.document()
    }
    pub fn selector(&self) -> &str {
        self.entry.selector()
    }
    pub fn disposition(&self) -> CloseDisposition {
        self.disposition
    }

    pub fn validate(&self) -> Result<(), String> {
        let state = self
            .membership
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.committed || state.membership != Some(self.membership.key) {
            return Err("The document close reservation is no longer available".into());
        }
        if !state.documents.iter().any(|entry| {
            entry.selector == self.entry.selector && entry.state == WorkspaceDocumentState::Live
        }) {
            return Err("The document is already closing or closed".into());
        }
        Ok(())
    }

    pub fn commit(&mut self) {
        let mut state = self
            .membership
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(state.membership, Some(self.membership.key));
        let entry = state
            .documents
            .iter_mut()
            .find(|entry| entry.selector == self.entry.selector)
            .expect("Reserved document missing");
        entry.state = WorkspaceDocumentState::Closing;
        if self.disposition == CloseDisposition::Terminal {
            state.accepting = false;
        }
        self.committed = true;
    }

    pub fn finish(&mut self, result: Result<(), String>) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        if !self.committed {
            return Err("The document close has not been accepted".into());
        }
        let result = result.and_then(|()| self.entry.document.finalize_closed_claims());
        {
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Err(error) = &result {
                let entry = state
                    .documents
                    .iter_mut()
                    .find(|entry| entry.selector == self.entry.selector)
                    .expect("Accepted document missing");
                entry.state = WorkspaceDocumentState::Failed;
                entry.error = Some(error.clone());
            } else {
                state
                    .documents
                    .retain(|entry| entry.selector != self.entry.selector);
            }
        }
        self.finished = Some(result.clone());
        self.membership.release();
        result
    }
}

impl Drop for CloseReservation {
    fn drop(&mut self) {
        if self.committed && self.finished.is_none() {
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = state
                .documents
                .iter_mut()
                .find(|entry| entry.selector == self.entry.selector)
            {
                entry.state = WorkspaceDocumentState::Failed;
                entry.error = Some("The accepted document close owner was lost".into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
