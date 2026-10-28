use super::*;
use crate::mcp::{TerminalAttempt, close::Output};
use crate::startup::{QuitReservation, WorkspaceQuitReview};
use rmcp::service::RequestContext;

const MAX_REVIEW_TOKEN_BYTES: usize = 128;

pub(super) struct Work {
    reservation: QuitReservation,
    attempt: TerminalAttempt,
    output: Option<Output>,
    response: Option<oneshot::Sender<CallToolResult>>,
}

fn available(queue: &Queue) -> Result<(), String> {
    if queue.stopping || queue.quitting {
        Err("The workspace is closing".into())
    } else if queue.executing || !queue.commands.is_empty() || !queue.completed.is_empty() {
        Err("A workspace operation is in progress; wait and review Quit all again".into())
    } else {
        Ok(())
    }
}

impl Workspace {
    pub fn quit_review(&self) -> Result<WorkspaceQuitReview, String> {
        let queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        available(&queue)?;
        self.0.factory.quit_review()
    }

    pub fn close_requires_quit_all(&self, selector: &str) -> Result<bool, String> {
        self.0.factory.close_requires_quit_all(selector)
    }

    pub fn quit_all_native(
        &self,
        review: &WorkspaceQuitReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        available(&queue)?;
        let attempt = TerminalAttempt::new()?;
        let (reservation, ()) = self
            .0
            .factory
            .commit_quit(review, discard_changes, || Ok(()))?;
        queue.quitting = true;
        queue.quit = Some(Work {
            reservation,
            attempt,
            output: None,
            response: None,
        });
        self.0.changed.notify_one();
        Ok(())
    }

    pub(super) fn quit_all_mcp(
        &self,
        token: &str,
        discard_changes: bool,
        context: &RequestContext<rmcp::RoleServer>,
    ) -> Result<oneshot::Receiver<CallToolResult>, String> {
        if token.len() > MAX_REVIEW_TOKEN_BYTES {
            return Err("The Quit all review token exceeds its limit".into());
        }
        let mut queue = self.0.queue.lock().unwrap_or_else(PoisonError::into_inner);
        available(&queue)?;
        let review = self.0.factory.quit_review()?;
        if review.token() != token {
            return Err("Documents, maps, drafts or activity changed since Quit all was reviewed; review the workspace and try again".into());
        }
        let entry = self
            .0
            .factory
            .documents()
            .into_iter()
            .next()
            .ok_or("No document is open")?;
        let server = self.fixed_server(&entry)?;
        let attempt = TerminalAttempt::new()?;
        let (send, receive) = oneshot::channel();
        let (reservation, output) = self.0.factory.commit_quit(&review, discard_changes, || {
            if context.ct.is_cancelled() {
                return Err("The Quit all request was cancelled before acceptance".into());
            }
            Output::reserve(&server, context, attempt.clone())
        })?;
        queue.quitting = true;
        queue.quit = Some(Work {
            reservation,
            attempt,
            output: Some(output),
            response: Some(send),
        });
        self.0.changed.notify_one();
        Ok(receive)
    }
}

pub(super) fn finish(workspace: &Workspace, runtime: &tokio::runtime::Runtime, mut work: Work) {
    let mut documents = Vec::new();
    for entry in work.reservation.documents().to_vec() {
        let document = entry.document();
        let id = document.id();
        let drained = document.shutdown();
        let retired = workspace
            .0
            .ui
            .as_ref()
            .map_or(Ok(()), |ui| ui_wait(runtime, (ui.retire)(id)));
        let ui_state = if workspace.0.ui.is_none() {
            "headless"
        } else if retired.is_ok() {
            "retired"
        } else {
            "failed"
        };
        let owner = workspace
            .0
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned();
        let joined = owner
            .and_then(|entry| {
                entry
                    .owner
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
            })
            .map_or(Ok(()), |mut owner| owner.shutdown());
        let mut result = combine(combine(drained, retired), joined);
        if result.is_ok()
            && let Some(ui) = &workspace.0.ui
        {
            result = ui_wait(runtime, (ui.release)(id));
        }
        result = work.reservation.finish_document(id, result);
        if result.is_ok() {
            workspace
                .0
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
        }
        if let Err(error) = &result {
            workspace
                .0
                .errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}: {error}", entry.selector()));
        }
        documents.push(
            json!({"document_id":entry.selector(),"runtime_document_id":id.value(),
            "closed":result.is_ok(),"state":if result.is_ok(){"closed"}else{"failed"},"ui":ui_state,
            "error":result.as_ref().err().map(|error|crate::mcp::close::bounded_error(error))}),
        );
    }
    work.reservation.finish();
    let complete = documents.iter().all(|document| document["closed"] == true);
    let mut result = CallToolResult::structured(
        json!({"attempt":work.attempt.value().to_string(),"closed":complete,"complete":complete,
        "status":if complete{"complete"}else{"partial_cleanup_failure"},"terminal":true,"documents":documents}),
    );
    result.is_error = Some(!complete);
    if let (Some(output), Some(response)) = (work.output, work.response)
        && let Err(error) = output.deliver(runtime, response, result)
    {
        workspace
            .0
            .errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }
    finish_transport(workspace, runtime);
}
