use super::*;
use crate::runtime::DocumentOperation;
use crate::session::BufferReplacementReview;
use serde_json::{Value, json};
use std::sync::{MutexGuard, TryLockError};

struct Identity {
    review: BufferReplacementReview,
    generation: u64,
    revision: u64,
    path: Option<PathBuf>,
}

struct Target {
    entry: WorkspaceDocument,
    identity: Option<Identity>,
    result: Value,
    _operation: Option<DocumentOperation>,
}

#[must_use]
pub struct SaveAllRequest {
    targets: Vec<Target>,
    actor: Actor,
    _membership: Membership,
}

fn try_session(document: &DocumentHandle) -> Result<MutexGuard<'_, Session>, String> {
    match document.session().try_lock() {
        Ok(session) => Ok(session),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(TryLockError::WouldBlock) => {
            Err("The document is busy; wait and retry Save all named maps".into())
        }
    }
}

fn drafts(session: &Session) -> Value {
    session.document_close_review().inspection()["drafts"].clone()
}

impl Workspace {
    pub fn prepare_save_all_named(&self, actor: Actor) -> Result<SaveAllRequest, String> {
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
        let mut targets = Vec::with_capacity(entries.len());
        for entry in entries {
            let mut result = json!({
                "document_id":entry.selector(),"runtime_document_id":entry.document.id().value(),
                "title":"Unavailable document","path":null,"status":"not_saved","saved":false,
                "unsaved_changes":null,"pending_reviews":[],"message":"The document was not saved",
            });
            let mut identity = None;
            let operation = if entry.state != WorkspaceDocumentState::Live {
                result["status"] = json!(entry.state.label());
                result["message"] =
                    json!(entry.error.as_deref().unwrap_or("The document is closing"));
                None
            } else {
                match entry.document.admit() {
                    Ok(operation) => {
                        match try_session(&entry.document) {
                            Ok(session) => {
                                identity = Some(Identity {
                                    review: session.buffer_replacement_review(),
                                    generation: session.document_generation(),
                                    revision: session.document().revision(),
                                    path: session.path().map(Path::to_owned),
                                });
                                result["title"] = json!(session.title());
                                result["path"] =
                                    json!(session.path().map(|path| path.display().to_string()));
                                result["generation"] = json!(session.document_generation());
                                result["revision"] = json!(session.document().revision());
                                result["unsaved_changes"] = json!(session.document().is_dirty());
                                result["needs_save_as"] = json!(
                                    session.path().is_none() && session.document().is_dirty()
                                );
                                result["pending_reviews"] = drafts(&session);
                            }
                            Err(error) => {
                                result["status"] = json!("busy");
                                result["message"] = json!(error);
                            }
                        }
                        Some(operation)
                    }
                    Err(error) => {
                        result["status"] = json!("closing");
                        result["message"] = json!(error);
                        None
                    }
                }
            };
            targets.push(Target {
                entry,
                identity,
                result,
                _operation: operation,
            });
        }
        Ok(SaveAllRequest {
            targets,
            actor,
            _membership: membership,
        })
    }
}

impl SaveAllRequest {
    pub fn run(mut self) -> Value {
        for target in &mut self.targets {
            let Some(identity) = &target.identity else {
                continue;
            };
            let mut session = lock(target.entry.document.session());
            if session.path() != identity.path.as_deref()
                || session.document_generation() != identity.generation
                || session.document().revision() != identity.revision
                || session
                    .ensure_buffer_replacement_review(&identity.review)
                    .is_err()
            {
                target.result["status"] = json!("stale");
                target.result["message"] = json!(
                    "The map changed after Save all named maps was accepted; retry the current documents"
                );
                continue;
            }
            if session.document_close_review().inspection()["closing"] == true {
                target.result["status"] = json!("closing");
                target.result["message"] = json!("The document is closing; no save was attempted");
                continue;
            }
            let pending = drafts(&session);
            target.result["pending_reviews"] = pending.clone();
            if !pending.as_array().is_none_or(Vec::is_empty) {
                target.result["status"] = json!("pending_review");
                target.result["message"] = json!(if identity.path.is_none()
                    && session.document().is_dirty()
                {
                    "Apply or cancel this map's pending reviews, then use Save as… before retrying"
                } else {
                    "Apply or cancel this map's pending reviews, then retry Save all named maps"
                });
                continue;
            }
            if identity.path.is_none() {
                let dirty = session.document().is_dirty();
                target.result["status"] = json!(if dirty {
                    "needs_save_as"
                } else {
                    "unnamed_clean"
                });
                target.result["message"] = json!(if dirty {
                    "Use Save as… in this map window before retrying"
                } else {
                    "Unnamed map has no committed changes; no file was written"
                });
                continue;
            }
            let dirty = session.document().is_dirty();
            let result = if dirty {
                session.save(self.actor, None).map(|_| ())
            } else {
                session.verify_named_save_source()
            };
            match result {
                Ok(()) => {
                    target.result["status"] = json!(if dirty { "saved" } else { "unchanged" });
                    target.result["saved"] = json!(dirty);
                    target.result["message"] = json!(if dirty {
                        "Saved the named map"
                    } else {
                        "Unchanged; original source identity verified and no file written"
                    });
                }
                Err(error) => {
                    target.result["status"] = json!("error");
                    target.result["error"] = json!(error);
                    target.result["message"] = json!(error);
                }
            }
            target.result["unsaved_changes"] = json!(session.document().is_dirty());
        }
        let documents = self
            .targets
            .iter()
            .map(|target| target.result.clone())
            .collect::<Vec<_>>();
        let complete = documents.iter().all(|document| {
            matches!(
                document["status"].as_str(),
                Some("saved" | "unchanged" | "unnamed_clean")
            )
        });
        let saved = documents
            .iter()
            .filter(|document| document["saved"] == true)
            .count();
        let unchanged = documents
            .iter()
            .filter(|document| document["status"] == "unchanged")
            .count();
        let not_saved = documents
            .iter()
            .filter(|document| {
                !matches!(
                    document["status"].as_str(),
                    Some("saved" | "unchanged" | "unnamed_clean")
                )
            })
            .count();
        json!({"status":if complete {"complete"}else{"not_complete"},"complete":complete,
            "saved":saved,"unchanged":unchanged,"not_saved":not_saved,"documents":documents,
            "message":format!("Save all named maps {}: {saved} saved, {unchanged} unchanged, {not_saved} need attention",if complete {"complete"}else{"not complete"}),
            "policy":"Fixed admitted document set; sequential per-document save results, not an atomic workspace snapshot. Clean named maps verify their original source identity without writing. Unnamed clean maps are retained without a file; unnamed dirty maps require Save as. Pending reviews are preserved and block that document. No destination selection, focus, window presentation or automatic retry."})
    }
}
