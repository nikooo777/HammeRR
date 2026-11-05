use super::*;
use crate::preference_file::{ObservedSource, Source};
use crate::workspace_snapshot::{Snapshot, Store};
use sha2::{Digest, Sha256};

const MAX_REVIEW_TOKEN_BYTES: usize = 128;

struct Review {
    store: Store,
    snapshot: Snapshot,
    source: ObservedSource,
    core: startup::WorkspaceRestoreReview,
}

struct Candidate {
    document: DocumentHandle,
    selector: String,
    entry: Option<Arc<Entry>>,
    host_attempted: bool,
}

struct CancelOnDrop(compiler::Control);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.0.cancel();
    }
}

impl Workspace {
    fn enqueue_saved(&self, command: Command) -> Result<(), String> {
        let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.ensure_idle()?;
        queue.commands.push_back(command);
        self.0.changed.notify_one();
        Ok(())
    }

    pub async fn saved_workspace(&self) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        self.enqueue_saved(Command::InspectWorkspace(send))?;
        receive
            .await
            .map_err(|_| "The workspace inspection owner stopped".to_owned())?
    }

    pub async fn save_workspace(&self, actor: Actor) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        self.enqueue_saved(Command::SaveWorkspace(actor, send))?;
        receive.await.map_err(|_| {
            "The saved-workspace owner stopped; inspect the saved workspace before retrying"
                .to_owned()
        })?
    }

    pub async fn restore_workspace_review(&self) -> Result<Value, String> {
        let (send, receive) = oneshot::channel();
        self.enqueue_saved(Command::ReviewRestore(send))?;
        receive
            .await
            .map_err(|_| "The workspace restore review owner stopped".to_owned())?
    }

    pub async fn restore_workspace(&self, actor: Actor, token: &str) -> Result<Value, String> {
        self.restore_workspace_with_control(actor, token, compiler::Control::default())
            .await
    }

    pub(super) async fn restore_workspace_with_control(
        &self,
        actor: Actor,
        token: &str,
        control: compiler::Control,
    ) -> Result<Value, String> {
        if token.len() > MAX_REVIEW_TOKEN_BYTES {
            return Err("The workspace restore review token exceeds its limit".into());
        }
        let cancellation = CancelOnDrop(control.clone());
        let (send, receive) = oneshot::channel();
        self.enqueue_saved(Command::RestoreWorkspace(
            actor,
            token.to_owned(),
            control,
            send,
        ))?;
        let result = receive.await.map_err(|_| "The accepted workspace restore owner stopped; inspect open documents before retrying".to_owned())?;
        drop(cancellation);
        result
    }
}

fn load_review(workspace: &Workspace) -> Result<Review, String> {
    let store = workspace.0.factory.snapshot_store()?;
    let (snapshot, source) = store.load_snapshot()?;
    let snapshot = snapshot.ok_or("No workspace is saved; save clean named maps first")?;
    let core = workspace.0.factory.restore_review(&snapshot)?;
    Ok(Review {
        store,
        snapshot,
        source,
        core,
    })
}

impl Review {
    fn token(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"workspace-restore-v1");
        digest.update(self.core.token().as_bytes());
        digest.update(self.store.path().as_os_str().as_encoded_bytes());
        match &self.source.source {
            Source::Missing => digest.update(b"missing"),
            Source::File {
                bytes,
                length,
                modified,
                identity,
            } => {
                digest.update(bytes);
                digest.update(format!("{length}:{modified:?}:{identity:?}").as_bytes());
            }
        }
        format!("restore-v1:{:x}", digest.finalize())
    }

    fn verify_file(&self) -> Result<(), String> {
        let current = self.store.read_current(&self.source)?;
        if current.source != self.source.source {
            return Err("The saved workspace changed; review Restore workspace again".into());
        }
        Ok(())
    }

    fn inspection(&self) -> Value {
        let mut inspection = self.core.inspection();
        inspection["review_token"] = json!(self.token());
        inspection["workspace_file"] = json!(self.store.path().display().to_string());
        inspection["saved_documents"] = json!(self.snapshot.documents);
        inspection["description"] = json!(format!(
            "Open the current disk contents at these saved paths as hidden documents:\n{}\n\nExisting named maps and unsaved work stay open. Only an exactly reviewed pristine startup map may be replaced. The review covers the saved paths and context, not map or content file contents. Use Show document to display a restored map.",
            self.snapshot
                .documents
                .iter()
                .map(|document| document.entry.path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ));
        inspection
    }
}

pub(super) fn inspect(workspace: &Workspace) -> Result<Value, String> {
    let store = workspace.0.factory.snapshot_store()?;
    let (snapshot, _) = store.load_snapshot()?;
    Ok(json!({
        "workspace_file":store.path().display().to_string(),"saved":snapshot.is_some(),
        "documents":snapshot.map(|snapshot|snapshot.documents),"limit":startup::DOCUMENT_LIMIT,
        "policy":"One explicitly saved set of clean named maps and their document contexts. Restore requires a fresh review and source checks; it creates hidden documents and never selects or focuses them."
    }))
}

pub(super) fn save(workspace: &Workspace, _actor: Actor) -> Result<Value, String> {
    let store = workspace.0.factory.snapshot_store()?;
    let (_, source) = store.load_snapshot()?;
    let capture = workspace.0.factory.prepare_workspace_snapshot()?;
    capture.verify()?;
    let publication = store
        .save_checked(capture.snapshot(), &source)
        .map_err(|error| error.message)?;
    let warning = publication.source.err();
    Ok(
        json!({"saved":true,"complete":warning.is_none(),"workspace_file":store.path().display().to_string(),
        "documents":capture.snapshot().documents,"warning":warning,
        "message":if warning.is_none(){"Saved the workspace"}else{"The workspace was written, but its new source identity could not be verified; inspect it before retrying"}}),
    )
}

pub(super) fn review(workspace: &Workspace) -> Result<Value, String> {
    Ok(load_review(workspace)?.inspection())
}

fn visibility(workspace: &Workspace) -> DocumentVisibility {
    if workspace.0.ui.is_some() {
        DocumentVisibility::Hidden
    } else {
        DocumentVisibility::Headless
    }
}

fn rollback(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    prepared: &mut startup::PreparedWorkspace,
    candidates: &[Candidate],
    original: String,
) -> Result<Value, String> {
    let mut errors = Vec::new();
    for candidate in candidates {
        let id = candidate.document.id();
        let mut cleanup = candidate.document.shutdown();
        if let Some(entry) = &candidate.entry
            && let Some(mut owner) = entry
                .owner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        {
            cleanup = combine(cleanup, owner.shutdown());
        }
        if candidate.host_attempted
            && let Some(ui) = &workspace.0.ui
        {
            cleanup = combine(cleanup, ui_wait(runtime, (ui.retire)(id)));
            if cleanup.is_ok() {
                cleanup = ui_wait(runtime, (ui.release)(id));
            }
        }
        if cleanup.is_ok() {
            cleanup = prepared.discard_candidate(id);
        }
        if let Err(error) = cleanup {
            let error = format!("{}: {error}", candidate.selector);
            if let Some(entry) = &candidate.entry {
                workspace
                    .0
                    .entries
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id, entry.clone());
            }
            let retained = prepared.fail_candidate(id, error.clone(), visibility(workspace));
            errors.push(match retained {
                Ok(_) => error,
                Err(retain) => format!("{error}; retaining failed candidate: {retain}"),
            });
        } else {
            workspace
                .0
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
        }
    }
    if errors.is_empty() {
        Err(original)
    } else {
        Err(format!(
            "{original}; restore rollback failed: {}. Inspect open documents; failed candidates retain their source claims",
            errors.join("; ")
        ))
    }
}

fn finish_replacement(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    prepared: &mut startup::PreparedWorkspace,
    replacement: Option<startup::WorkspaceDocument>,
) -> Result<(), String> {
    let Some(replacement) = replacement else {
        return Ok(());
    };
    let document = replacement.document();
    let id = document.id();
    let mut result = document.shutdown();
    if let Some(ui) = &workspace.0.ui {
        result = combine(result, ui_wait(runtime, (ui.retire)(id)));
    }
    let entry = workspace
        .0
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id)
        .cloned();
    if let Some(entry) = entry
        && let Some(mut owner) = entry
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    {
        result = combine(result, owner.shutdown());
    }
    if result.is_ok()
        && let Some(ui) = &workspace.0.ui
    {
        result = ui_wait(runtime, (ui.release)(id));
    }
    result = prepared.finish_replacement(result);
    if result.is_ok() {
        workspace
            .0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
    result
}

pub(super) fn restore(
    workspace: &Workspace,
    runtime: &tokio::runtime::Runtime,
    actor: Actor,
    token: &str,
    control: &compiler::Control,
) -> Result<Value, String> {
    control.checkpoint()?;
    let review = load_review(workspace)?;
    if review.token() != token {
        return Err(
            "Saved workspace or open documents changed; review Restore workspace again".into(),
        );
    }
    review.verify_file()?;
    let mut prepared =
        workspace
            .0
            .factory
            .prepare_restore(actor, &review.snapshot, &review.core, control)?;
    let mut candidates = prepared
        .candidates()
        .iter()
        .map(|candidate| Candidate {
            document: candidate.document(),
            selector: candidate.selector().to_owned(),
            entry: None,
            host_attempted: false,
        })
        .collect::<Vec<_>>();
    let preparation = (|| {
        for candidate in &mut candidates {
            control.checkpoint()?;
            let entry = workspace.entry(candidate.document.clone())?;
            candidate.entry = Some(entry.clone());
            if let Some(ui) = &workspace.0.ui {
                candidate.host_attempted = true;
                ui_wait(
                    runtime,
                    (ui.create)(
                        candidate.document.clone(),
                        candidate.selector.clone(),
                        entry
                            .server
                            .terminal
                            .clone()
                            .ok_or("Restored document has no Close owner")?,
                        false,
                    ),
                )?;
            }
        }
        control.checkpoint()?;
        prepared.verify()?;
        for candidate in &candidates {
            workspace
                .0
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    candidate.document.id(),
                    candidate
                        .entry
                        .as_ref()
                        .ok_or("Restored document route is not ready")?
                        .clone(),
                );
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = preparation {
        return rollback(workspace, runtime, &mut prepared, &candidates, error);
    }
    let visibilities = vec![visibility(workspace); candidates.len()];
    let published = match prepared.try_publish(&visibilities, || review.verify_file()) {
        Ok(published) => published,
        Err(error) => return rollback(workspace, runtime, &mut prepared, &candidates, error),
    };
    let documents = published.documents().iter().map(|entry| {
        let document = entry.document();
        let session = lock(document.session());
        json!({"document_id":entry.selector(),"runtime_document_id":document.id().value(),"path":session.path().map(|path|path.display().to_string()),"visibility":entry.visibility().label()})
    }).collect::<Vec<_>>();
    let replacement = published.replacement().cloned();
    let replaced = replacement.as_ref().map(metadata);
    let cleanup = finish_replacement(workspace, runtime, &mut prepared, replacement);
    Ok(
        json!({"restored":true,"complete":cleanup.is_ok(),"documents":documents,"replaced":replaced,"error":cleanup.as_ref().err(),
        "message":if cleanup.is_ok(){"Restored the complete saved workspace as hidden documents"}else{"The complete saved workspace is restored, but retiring the pristine startup document failed; inspect open documents"},
        "policy":"Every saved document is published together. Existing named maps and unsaved work stay intact. Restored documents keep fixed Sessions and remain hidden until Show document. Failed cleanup retains the old entry and its claims."}),
    )
}
