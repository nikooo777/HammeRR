use super::*;
use crate::runtime::DocumentCloseReview;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct Target {
    entry: WorkspaceDocument,
    review: DocumentCloseReview,
    inspection: Value,
}

#[derive(Clone)]
pub struct WorkspaceQuitReview {
    nonce: String,
    targets: Vec<Target>,
}

impl WorkspaceQuitReview {
    pub fn requires_discard(&self) -> bool {
        self.targets
            .iter()
            .any(|target| target.review.requires_discard())
    }

    pub fn token(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"workspace-quit-v1");
        digest.update(self.nonce.as_bytes());
        for target in &self.targets {
            digest.update(target.inspection.to_string().as_bytes());
        }
        format!("quit-v1:{:x}", digest.finalize())
    }

    pub fn inspection(&self) -> Value {
        json!({"review_token":self.token(),"discard_required":self.requires_discard(),
            "documents":self.targets.iter().map(|target| &target.inspection).collect::<Vec<_>>(),
            "policy":"Quit all closes every reviewed document, including hidden maps. Explicit consent is required for reviewed unsaved changes and pending drafts. Admission and the exact document set are frozen together after all preflight checks pass. Owned work is cancelled and drained; failed cleanup retains the document and source claims. MCP terminal output is delivered before service shutdown."})
    }

    pub fn description(&self) -> String {
        self.targets
            .iter()
            .map(|target| {
                let detail = &target.inspection;
                let drafts = detail["close"]["drafts"]
                    .as_array()
                    .map(|drafts| {
                        drafts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                format!(
                    "{} ({})\n{}\n{}{}",
                    detail["title"].as_str().unwrap_or("Untitled map"),
                    target.entry.visibility.label(),
                    detail["path"].as_str().unwrap_or("Not saved"),
                    if detail["close"]["unsaved_changes"] == true {
                        "Unsaved map changes"
                    } else {
                        "No unsaved map changes"
                    },
                    if drafts.is_empty() {
                        String::new()
                    } else {
                        format!("; pending drafts: {drafts}")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

fn capture(
    nonce: &str,
    mut entries: Vec<WorkspaceDocument>,
) -> Result<WorkspaceQuitReview, String> {
    if entries.is_empty() {
        return Err("No document is open".into());
    }
    entries.sort_by_key(|entry| entry.document.id());
    for entry in &entries {
        if entry.state != WorkspaceDocumentState::Live
            || entry.document.lifecycle() != DocumentLifecycle::Live
        {
            return Err(format!(
                "{}: {}",
                entry.selector,
                entry
                    .error
                    .as_deref()
                    .unwrap_or("The document is closing or unavailable; inspect list_documents")
            ));
        }
    }
    let mut sessions = Vec::with_capacity(entries.len());
    for entry in &entries {
        sessions.push(
            entry
                .document
                .session()
                .try_lock()
                .map_err(|_| "A document is busy; wait and review Quit all again".to_owned())?,
        );
    }
    let targets = entries.iter().zip(&sessions).map(|(entry, session)| {
        let review = entry.document.close_review_for(session);
        let inspection = json!({"document_id":entry.selector,"runtime_document_id":entry.document.id().value(),
            "title":session.title(),"path":session.path().map(|path|path.display().to_string()),
            "generation":session.document_generation(),"revision":session.document().revision(),
            "visibility":entry.visibility.label(),"state":entry.state.label(),"close":review.inspection()});
        Target { entry: entry.clone(), review, inspection }
    }).collect();
    Ok(WorkspaceQuitReview {
        nonce: nonce.to_owned(),
        targets,
    })
}

impl Workspace {
    pub fn quit_review(&self) -> Result<WorkspaceQuitReview, String> {
        let (membership, entries) = {
            let mut state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            ensure_available(&state)?;
            let membership = Membership::reserve(self.core.clone(), &mut state)?;
            (membership, state.documents.clone())
        };
        let review = capture(&self.core.nonce, entries);
        drop(membership);
        review
    }

    #[cfg(any(test, feature = "mcp"))]
    pub(crate) fn commit_quit<R>(
        &self,
        review: &WorkspaceQuitReview,
        discard_changes: bool,
        reserve: impl FnOnce() -> Result<R, String>,
    ) -> Result<(QuitReservation, R), String> {
        let (membership, entries) = {
            let mut state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            ensure_available(&state)?;
            let membership = Membership::reserve(self.core.clone(), &mut state)?;
            (membership, state.documents.clone())
        };
        let current = capture(&self.core.nonce, entries)?;
        if review.token() != current.token() {
            return Err("Documents, maps, drafts or activity changed since Quit all was reviewed; review the workspace and try again".into());
        }
        let targets = current
            .targets
            .iter()
            .map(|target| (target.entry.document(), target.review.clone()))
            .collect::<Vec<_>>();
        let mut reservation = QuitReservation {
            membership,
            entries: current
                .targets
                .into_iter()
                .map(|target| target.entry)
                .collect(),
            committed: false,
            finished: false,
        };
        let reserved =
            DocumentHandle::commit_reviewed_close_set(&targets, discard_changes, || {
                let reserved = reserve()?;
                let mut state = self
                    .core
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if !state.accepting || state.membership != Some(reservation.membership.key) {
                    return Err("The workspace closed before Quit all was accepted".into());
                }
                state.accepting = false;
                for entry in &mut state.documents {
                    entry.state = WorkspaceDocumentState::Closing;
                }
                reservation.committed = true;
                Ok(reserved)
            })?;
        Ok((reservation, reserved))
    }
}

#[must_use]
pub struct QuitReservation {
    membership: Membership,
    entries: Vec<WorkspaceDocument>,
    committed: bool,
    finished: bool,
}

impl QuitReservation {
    pub fn documents(&self) -> &[WorkspaceDocument] {
        &self.entries
    }

    #[cfg(any(test, feature = "mcp"))]
    pub(crate) fn finish_document(
        &mut self,
        id: DocumentId,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.document.id() == id)
            .ok_or("Unknown Quit all document")?;
        let result = result.and_then(|()| entry.document.finalize_closed_claims());
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
                .find(|entry| entry.document.id() == id)
            {
                entry.state = WorkspaceDocumentState::Failed;
                entry.error = Some(error.clone());
            }
        } else {
            state.documents.retain(|entry| entry.document.id() != id);
        }
        result
    }

    #[cfg(any(test, feature = "mcp"))]
    pub(crate) fn finish(&mut self) {
        self.finished = true;
        self.membership.release();
    }
}

impl Drop for QuitReservation {
    fn drop(&mut self) {
        if self.committed && !self.finished {
            let mut state = self
                .membership
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for entry in &mut state.documents {
                if entry.state == WorkspaceDocumentState::Closing {
                    entry.state = WorkspaceDocumentState::Failed;
                    entry.error = Some(
                        "The accepted Quit all owner was lost; cleanup remains required".into(),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
