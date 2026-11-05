use super::*;
use crate::runtime::{DocumentCloseReview, DocumentOperation};
use crate::session::WorkspaceSourceGuard;
use crate::workspace_snapshot::{Snapshot, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{MutexGuard, TryLockError};

fn try_session(document: &DocumentHandle) -> Result<MutexGuard<'_, Session>, String> {
    match document.session().try_lock() {
        Ok(session) => Ok(session),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(TryLockError::WouldBlock) => {
            Err("A document is busy; wait and review the workspace again".into())
        }
    }
}

fn entry_identity(entry: &WorkspaceDocument) -> Value {
    json!({"document_id":entry.selector,"state":entry.state.label(),"visibility":entry.visibility.label()})
}

fn ensure_idle_review(review: &DocumentCloseReview) -> Result<(), String> {
    if review.inspection()["admission"]["active_operations"] != 0 {
        return Err(
            "A document has an admitted request or worker; wait and review the workspace again"
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone)]
struct Replacement {
    entry: WorkspaceDocument,
    review: DocumentCloseReview,
}

#[derive(Clone)]
pub struct WorkspaceRestoreReview {
    nonce: String,
    snapshot: Snapshot,
    entries: Vec<Value>,
    replacement: Option<Replacement>,
}

impl WorkspaceRestoreReview {
    pub fn token(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"workspace-restore-v1");
        digest.update(self.nonce.as_bytes());
        digest.update(
            serde_json::to_vec(&self.snapshot).expect("Workspace snapshot is serializable"),
        );
        digest.update(
            serde_json::to_vec(&self.entries).expect("Workspace identities are serializable"),
        );
        if let Some(replacement) = &self.replacement {
            digest.update(replacement.review.inspection().to_string().as_bytes());
        }
        format!("workspace-restore-v1:{:x}", digest.finalize())
    }

    pub fn inspection(&self) -> Value {
        json!({"review_token":self.token(),"documents":self.entries,
            "restore_count":self.snapshot.documents.len(),
            "replace_initial_document":self.replacement.as_ref().map(|replacement|replacement.entry.selector()),
            "policy":"Review pins the saved workspace record and current document set, not map or content bytes. Restore opens current VMF and content files at the recorded paths, then checks their original preparation guards before enrolling the complete saved set. Only the still-pristine initial unnamed map can be retired; other maps remain open and require free capacity. A failed retirement retains its error and claims and makes the result incomplete."})
    }
}

fn capture_review(
    nonce: &str,
    snapshot: &Snapshot,
    entries: &[WorkspaceDocument],
    startup_placeholder: Option<DocumentId>,
) -> Result<WorkspaceRestoreReview, String> {
    snapshot.validate()?;
    for entry in entries {
        if entry.state != WorkspaceDocumentState::Live
            || entry.document.lifecycle() != DocumentLifecycle::Live
        {
            return Err(format!(
                "{}: {}",
                entry.selector,
                entry
                    .error
                    .as_deref()
                    .unwrap_or("The document is closing or unavailable")
            ));
        }
    }
    let replacement = if let [entry] = entries
        && Some(entry.document.id()) == startup_placeholder
    {
        let session = try_session(&entry.document)?;
        if session.is_initial_workspace_placeholder() {
            let review = entry.document.close_review_for(&session);
            ensure_idle_review(&review)?;
            if review.requires_discard() {
                return Err(
                    "Workspace restore cannot discard map changes or pending drafts".into(),
                );
            }
            Some(Replacement {
                entry: entry.clone(),
                review,
            })
        } else {
            None
        }
    } else {
        None
    };
    if entries.len() - usize::from(replacement.is_some()) + snapshot.documents.len()
        > DOCUMENT_LIMIT
    {
        return Err("The saved workspace does not fit. Existing named, changed or busy maps are preserved; save or close a document before restoring".into());
    }
    Ok(WorkspaceRestoreReview {
        nonce: nonce.to_owned(),
        snapshot: snapshot.clone(),
        entries: entries.iter().map(entry_identity).collect(),
        replacement,
    })
}

struct CapturedDocument {
    entry: WorkspaceDocument,
    review: DocumentCloseReview,
    guard: WorkspaceSourceGuard,
    operation: DocumentOperation,
}

#[must_use]
pub struct WorkspaceSnapshotCapture {
    snapshot: Snapshot,
    documents: Vec<CapturedDocument>,
    _membership: Membership,
}

impl WorkspaceSnapshotCapture {
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn verify(&self) -> Result<(), String> {
        for (captured, snapshot) in self.documents.iter().zip(&self.snapshot.documents) {
            {
                let session = try_session(&captured.entry.document)?;
                let current = captured
                    .entry
                    .document
                    .close_review_excluding(&session, Some(&captured.operation));
                if current != captured.review {
                    return Err("A map, view, draft or activity changed while preparing Save workspace; review and save the workspace again".into());
                }
                session.verify_workspace_document_snapshot(snapshot)?;
            }
            captured.guard.verify()?;
        }
        Ok(())
    }
}

pub struct RestoreCandidate {
    document: Option<DocumentHandle>,
    selector: String,
    guard: WorkspaceSourceGuard,
}

impl RestoreCandidate {
    pub fn document(&self) -> DocumentHandle {
        self.document
            .as_ref()
            .expect("Restored candidate was already relinquished")
            .clone()
    }

    pub fn selector(&self) -> &str {
        &self.selector
    }
}

#[derive(Clone)]
pub struct PublishedWorkspace {
    documents: Vec<WorkspaceDocument>,
    replacement: Option<WorkspaceDocument>,
}

impl PublishedWorkspace {
    pub fn documents(&self) -> &[WorkspaceDocument] {
        &self.documents
    }

    pub fn replacement(&self) -> Option<&WorkspaceDocument> {
        self.replacement.as_ref()
    }
}

#[must_use]
pub struct PreparedWorkspace {
    membership: Membership,
    candidates: Vec<RestoreCandidate>,
    review: WorkspaceRestoreReview,
    control: compiler::Control,
    published: Option<PublishedWorkspace>,
    replacement_result: Option<Result<(), String>>,
    aborted: bool,
}

impl Workspace {
    pub fn snapshot_store(&self) -> Result<Store, String> {
        let services = self
            .services
            .upgrade()
            .ok_or("The application owner has finished")?;
        Ok(Store {
            directory: services.directory.clone()?,
        })
    }

    pub fn prepare_workspace_snapshot(&self) -> Result<WorkspaceSnapshotCapture, String> {
        let (membership, entries) = self.reserve_workspace_operation()?;
        if entries.is_empty() {
            return Err("No document is open".into());
        }
        let mut documents = Vec::with_capacity(entries.len());
        let mut snapshots = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.state != WorkspaceDocumentState::Live {
                return Err(
                    "Every workspace document must be live before saving the workspace".into(),
                );
            }
            let operation = entry.document.admit_named("Save workspace")?;
            let (snapshot, guard, review) = {
                let session = try_session(&entry.document)?;
                let review = entry
                    .document
                    .close_review_excluding(&session, Some(&operation));
                ensure_idle_review(&review)?;
                let (snapshot, guard) = session.capture_workspace_document()?;
                (snapshot, guard, review)
            };
            snapshots.push(snapshot);
            documents.push(CapturedDocument {
                entry,
                review,
                guard,
                operation,
            });
        }
        let captured = WorkspaceSnapshotCapture {
            snapshot: Snapshot {
                documents: snapshots,
            },
            documents,
            _membership: membership,
        };
        captured.snapshot.validate()?;
        captured.verify()?;
        Ok(captured)
    }

    fn reserve_workspace_operation(&self) -> Result<(Membership, Vec<WorkspaceDocument>), String> {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure_available(&state)?;
        let membership = Membership::reserve(self.core.clone(), &mut state)?;
        Ok((membership, state.documents.clone()))
    }

    pub fn restore_review(&self, snapshot: &Snapshot) -> Result<WorkspaceRestoreReview, String> {
        let (membership, entries) = self.reserve_workspace_operation()?;
        let placeholder = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .startup_placeholder;
        let result = capture_review(&self.core.nonce, snapshot, &entries, placeholder);
        drop(membership);
        result
    }

    pub fn prepare_restore(
        &self,
        actor: Actor,
        snapshot: &Snapshot,
        review: &WorkspaceRestoreReview,
        control: &compiler::Control,
    ) -> Result<PreparedWorkspace, String> {
        let (membership, entries) = self.reserve_workspace_operation()?;
        let placeholder = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .startup_placeholder;
        let current = capture_review(&self.core.nonce, snapshot, &entries, placeholder)?;
        if current.token() != review.token() {
            return Err("The saved workspace, document set or initial map changed after review; review Restore workspace again".into());
        }
        let services = self
            .services
            .upgrade()
            .ok_or("The application owner has finished")?;
        let mut prepared = PreparedWorkspace {
            membership,
            candidates: Vec::with_capacity(snapshot.documents.len()),
            review: current,
            control: control.clone(),
            published: None,
            replacement_result: None,
            aborted: false,
        };
        for saved in &snapshot.documents {
            control.checkpoint()?;
            let mut session = services.session()?;
            let guard = session.prepare_workspace_document(actor, saved, control)?;
            let document = services.start_document(session)?;
            let selector = format!("{}:{}", self.core.nonce, document.id().value());
            prepared.candidates.push(RestoreCandidate {
                document: Some(document),
                selector,
                guard,
            });
        }
        prepared.verify()?;
        Ok(prepared)
    }
}

impl PreparedWorkspace {
    pub fn candidates(&self) -> &[RestoreCandidate] {
        &self.candidates
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.aborted || self.published.is_some() {
            return Err("The workspace restore was already published or aborted".into());
        }
        self.control.checkpoint()?;
        for candidate in &self.candidates {
            candidate.guard.verify()?;
            self.control.checkpoint()?;
        }
        Ok(())
    }

    pub fn try_publish(
        &mut self,
        visibilities: &[DocumentVisibility],
        guard: impl FnOnce() -> Result<(), String>,
    ) -> Result<PublishedWorkspace, String> {
        if visibilities.len() != self.candidates.len() {
            return Err("Every restored document needs its acknowledged presentation state".into());
        }
        if self.aborted || self.published.is_some() {
            return Err("The workspace restore was already published or aborted".into());
        }
        guard()?;
        self.verify()?;
        let documents = self
            .candidates
            .iter()
            .zip(visibilities)
            .map(|(candidate, &visibility)| WorkspaceDocument {
                document: candidate.document(),
                selector: candidate.selector.clone(),
                visibility,
                state: WorkspaceDocumentState::Live,
                error: None,
            })
            .collect::<Vec<_>>();
        let replacement = self.review.replacement.clone();
        let publish = || {
            self.control.checkpoint()?;
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.accepting || state.membership != Some(self.membership.key) {
                return Err("The application closed while preparing workspace restore".into());
            }
            if state
                .documents
                .iter()
                .map(entry_identity)
                .collect::<Vec<_>>()
                != self.review.entries
            {
                return Err("The document set changed while preparing workspace restore".into());
            }
            let removed = usize::from(replacement.is_some());
            if state.documents.len().saturating_sub(removed) + documents.len() > DOCUMENT_LIMIT {
                return Err(
                    "The saved workspace no longer fits; no restored document was enrolled".into(),
                );
            }
            if let Some(replacement) = &replacement {
                let entry = state
                    .documents
                    .iter_mut()
                    .find(|entry| entry.selector == replacement.entry.selector)
                    .ok_or("The initial document is no longer open")?;
                entry.state = WorkspaceDocumentState::Closing;
                state.startup_placeholder = None;
            }
            state.documents.extend(documents.clone());
            Ok(())
        };
        if let Some(replacement) = &replacement {
            replacement.entry.document.commit_reviewed_close_with(
                &replacement.review,
                false,
                publish,
            )?;
        } else {
            publish()?;
        }
        for candidate in &mut self.candidates {
            candidate.document = None;
        }
        let published = PublishedWorkspace {
            documents,
            replacement: replacement.map(|replacement| replacement.entry),
        };
        if published.replacement.is_none() {
            self.replacement_result = Some(Ok(()));
            self.membership.release();
        }
        self.published = Some(published.clone());
        Ok(published)
    }

    pub fn finish_replacement(&mut self, result: Result<(), String>) -> Result<(), String> {
        let published = self
            .published
            .as_ref()
            .ok_or("Workspace restore has not been published")?;
        let Some(replacement) = &published.replacement else {
            return Ok(());
        };
        if let Some(result) = &self.replacement_result {
            return result.clone();
        }
        let result = result.and_then(|()| replacement.document.finalize_closed_claims());
        let mut state = self
            .membership
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = &result {
            if let Some(entry) = state
                .documents
                .iter_mut()
                .find(|entry| entry.selector == replacement.selector)
            {
                entry.state = WorkspaceDocumentState::Failed;
                entry.error = Some(error.clone());
            }
        } else {
            state
                .documents
                .retain(|entry| entry.selector != replacement.selector);
        }
        drop(state);
        self.replacement_result = Some(result.clone());
        self.membership.release();
        result
    }

    pub fn discard_candidate(&mut self, id: DocumentId) -> Result<(), String> {
        if self.published.is_some() {
            return Err("Enrolled workspace documents cannot be rolled back".into());
        }
        self.aborted = true;
        let candidate = self
            .candidates
            .iter_mut()
            .find(|candidate| {
                candidate
                    .document
                    .as_ref()
                    .is_some_and(|document| document.id() == id)
            })
            .ok_or("Unknown workspace restore candidate")?;
        candidate.document().finalize_closed_claims()?;
        candidate.document = None;
        Ok(())
    }

    pub fn fail_candidate(
        &mut self,
        id: DocumentId,
        error: String,
        visibility: DocumentVisibility,
    ) -> Result<WorkspaceDocument, String> {
        if self.published.is_some() {
            return Err("Enrolled workspace documents cannot be rolled back".into());
        }
        self.aborted = true;
        let candidate = self
            .candidates
            .iter_mut()
            .find(|candidate| {
                candidate
                    .document
                    .as_ref()
                    .is_some_and(|document| document.id() == id)
            })
            .ok_or("Unknown workspace restore candidate")?;
        let document = candidate.document();
        let error = match document.shutdown() {
            Ok(()) => error,
            Err(drain) => format!("{error}; document cleanup failed: {drain}"),
        };
        let entry = WorkspaceDocument {
            document,
            selector: candidate.selector.clone(),
            visibility,
            state: WorkspaceDocumentState::Failed,
            error: Some(error),
        };
        self.membership
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .documents
            .push(entry.clone());
        candidate.document = None;
        Ok(entry)
    }
}

impl Drop for PreparedWorkspace {
    fn drop(&mut self) {
        if let Some(published) = &self.published {
            if self.replacement_result.is_none()
                && let Some(replacement) = &published.replacement
            {
                let mut state = self
                    .membership
                    .core
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let Some(entry) = state
                    .documents
                    .iter_mut()
                    .find(|entry| entry.selector == replacement.selector)
                {
                    entry.state = WorkspaceDocumentState::Failed;
                    entry.error=Some("The accepted workspace restore lost its initial-document retirement owner; restored maps remain enrolled".into());
                }
            }
            return;
        }
        for candidate in &mut self.candidates {
            if let Some(document) = candidate.document.take()
                && let Err(error) = document
                    .shutdown()
                    .and_then(|()| document.finalize_closed_claims())
            {
                eprintln!("Workspace restore candidate cleanup failed: {error}");
                self.membership
                    .core
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .documents
                    .push(WorkspaceDocument {
                        document,
                        selector: candidate.selector.clone(),
                        visibility: DocumentVisibility::Hidden,
                        state: WorkspaceDocumentState::Failed,
                        error: Some(error),
                    });
            }
        }
    }
}
