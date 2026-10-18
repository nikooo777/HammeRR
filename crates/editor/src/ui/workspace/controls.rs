use super::*;
use crate::{OpenDocumentRow, OpenDocumentsWindow};
use serde_json::Value;

pub(super) struct Controller {
    pub(super) window: OpenDocumentsWindow,
    pub(super) origin: runtime::DocumentId,
    save_results: RefCell<Option<Value>>,
}

pub(super) enum Operation {
    New,
    Open(std::path::PathBuf),
    Show(String),
    SaveAll,
}

fn current(owner: &SharedOwner, controller: &Rc<Controller>) -> bool {
    owner
        .borrow()
        .dialog
        .as_ref()
        .is_some_and(|active| Rc::ptr_eq(active, controller))
}

pub(super) fn refresh(owner: &SharedOwner) {
    let (workspace, controller) = {
        let state = owner.borrow();
        let (Some(workspace), Some(controller)) = (&state.workspace, &state.dialog) else {
            return;
        };
        (workspace.clone(), controller.clone())
    };
    match workspace.list_documents() {
        Ok(report) => {
            let documents = report["documents"]
                .as_array()
                .expect("Workspace documents are an array");
            controller.window.set_summary(
                format!(
                    "{} / {} documents · {}",
                    documents.len(),
                    report["limit"],
                    if report["busy"] == true {
                        "Document operation in progress"
                    } else {
                        "Independent selection, Undo, drafts and camera"
                    }
                )
                .into(),
            );
            controller
                .window
                .set_can_create(report["can_create"] == true);
            controller
                .window
                .set_can_save_all(report["busy"] != true && !documents.is_empty());
            let results = controller.save_results.borrow();
            controller.window.set_rows(ModelRc::new(VecModel::from(
                documents
                    .iter()
                    .map(|value| row(value, results.as_ref()))
                    .collect::<Vec<_>>(),
            )));
        }
        Err(error) => controller.window.set_message(error.into()),
    }
}

fn save_status(status: &str) -> &str {
    match status {
        "saved" => "Saved",
        "unchanged" => "Unchanged; no file written",
        "unnamed_clean" => "Unnamed; no committed changes",
        "needs_save_as" => "Needs Save as…",
        "pending_review" => "Pending review",
        "stale" => "Map changed; retry",
        "busy" => "Busy; retry",
        "closing" => "Closing",
        "failed" => "Unavailable",
        "error" => "Save failed",
        _ => "Not saved",
    }
}

fn row(value: &Value, results: Option<&Value>) -> OpenDocumentRow {
    let save = results
        .and_then(|results| results["documents"].as_array())
        .and_then(|documents| {
            documents
                .iter()
                .find(|document| document["document_id"] == value["document_id"])
        })
        .map(|document| {
            format!(
                "\nLast Save all result: {} · {}",
                save_status(document["status"].as_str().unwrap_or("not_saved")),
                document["message"].as_str().unwrap_or("Not saved")
            )
        })
        .unwrap_or_default();
    let title = value["title"].as_str().unwrap_or("Untitled map");
    let dirty = value["unsaved_changes"] == true;
    let error = value["error"]
        .as_str()
        .map(|error| format!("\n{error}"))
        .unwrap_or_default();
    OpenDocumentRow {
        selector: value["document_id"].as_str().unwrap().into(),
        title: format!("{title}{}", if dirty { " · Unsaved changes" } else { "" }).into(),
        detail: format!(
            "{}\nDocument {} · {} · {} · {}{error}{save}",
            value["path"].as_str().unwrap_or("Not saved"),
            value["runtime_document_id"],
            value["lifecycle"].as_str().unwrap_or("unknown"),
            value["visibility"].as_str().unwrap_or("unknown"),
            value["work"].as_str().unwrap_or("Idle")
        )
        .into(),
        live: value["lifecycle"] == "live",
    }
}

pub(super) fn submit(
    owner: &WeakOwner,
    controller: &std::rc::Weak<Controller>,
    operation: Operation,
) {
    let (Some(retained_owner), Some(retained_controller)) = (owner.upgrade(), controller.upgrade())
    else {
        return;
    };
    if !current(&retained_owner, &retained_controller) {
        return;
    }
    let Some(workspace) = retained_owner.borrow().workspace.clone() else {
        return;
    };
    retained_controller.window.set_busy(true);
    retained_controller.window.set_message("Working…".into());
    let (owner, controller) = (owner.clone(), controller.clone());
    slint::spawn_local(async move {
        let result = match operation {
            Operation::New => workspace.new_document(Actor::User, true).await,
            Operation::Open(path) => workspace.open_document(Actor::User, &path, true).await,
            Operation::Show(selector) => workspace.show_document(&selector).await,
            Operation::SaveAll => workspace.save_all_named_maps(Actor::User).await,
        };
        let (Some(owner), Some(controller)) = (owner.upgrade(), controller.upgrade()) else {
            return;
        };
        if !current(&owner, &controller) {
            return;
        }
        controller.window.set_busy(false);
        if let Ok(value) = &result
            && value.get("complete").is_some()
        {
            *controller.save_results.borrow_mut() = Some(value.clone());
        }
        controller.window.set_message(match result {
            Ok(value) => value["message"]
                .as_str()
                .unwrap_or("Document operation completed")
                .into(),
            Err(error) => error.into(),
        });
        refresh(&owner);
    })
    .expect("The UI event loop is running");
}

pub(super) fn open(owner: &SharedOwner, origin: runtime::DocumentId) {
    let window = match OpenDocumentsWindow::new() {
        Ok(window) => window,
        Err(error) => {
            report_origin(owner, origin, &error.to_string());
            return;
        }
    };
    let controller = Rc::new(Controller {
        window,
        origin,
        save_results: RefCell::new(None),
    });
    let weak = Rc::downgrade(&controller);
    let target = Rc::downgrade(owner);
    controller.window.on_save_all_named_maps({
        let (owner, controller) = (target.clone(), weak.clone());
        move || submit(&owner, &controller, Operation::SaveAll)
    });
    controller.window.on_new_document({
        let (owner, controller) = (target.clone(), weak.clone());
        move || submit(&owner, &controller, Operation::New)
    });
    controller.window.on_show_document({
        let (owner, controller) = (target.clone(), weak.clone());
        move |selector| submit(&owner, &controller, Operation::Show(selector.to_string()))
    });
    controller.window.on_open_document({
        let (owner, controller) = (target.clone(), weak.clone());
        move || {
            let (Some(retained_owner), Some(retained_controller)) =
                (owner.upgrade(), controller.upgrade())
            else {
                return;
            };
            if !current(&retained_owner, &retained_controller) {
                return;
            }
            retained_controller.window.set_busy(true);
            let dialog = rfd::AsyncFileDialog::new()
                .set_title("Open document")
                .add_filter("Valve Map File", &["vmf"]);
            let (owner, controller) = (owner.clone(), controller.clone());
            slint::spawn_local(async move {
                let file = dialog.pick_file().await;
                let (Some(retained_owner), Some(retained_controller)) =
                    (owner.upgrade(), controller.upgrade())
                else {
                    return;
                };
                if !current(&retained_owner, &retained_controller) {
                    return;
                }
                retained_controller.window.set_busy(false);
                if let Some(file) = file {
                    submit(&owner, &controller, Operation::Open(file.path().to_owned()));
                }
            })
            .expect("The UI event loop is running");
        }
    });
    controller.window.on_close_document({
        let (owner, controller) = (target.clone(), weak.clone());
        move |selector| {
            let (Some(owner), Some(controller)) = (owner.upgrade(), controller.upgrade()) else {
                return;
            };
            if !current(&owner, &controller) {
                return;
            }
            let host = owner
                .borrow()
                .hosts
                .iter()
                .find(|(id, _)| id == selector.as_str())
                .map(|(_, host)| host.clone());
            let Some(host) = host else {
                controller
                    .window
                    .set_message("This document no longer has an open window".into());
                return;
            };
            let request = {
                let host = host.borrow();
                host.window
                    .app
                    .as_ref()
                    .map(|app| (app.clone_strong(), host.window.weak_ui()))
            };
            if let Some((app, ui)) = request {
                request_terminal_close(&app, &ui, &Rc::downgrade(&host));
            }
        }
    });
    controller.window.on_close_list({
        let (owner, controller) = (target.clone(), weak.clone());
        move || close(&owner, &controller)
    });
    controller.window.window().on_close_requested({
        let (owner, controller) = (target.clone(), weak.clone());
        move || {
            close(&owner, &controller);
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    controller.window.on_refresh({
        let owner = target;
        move || {
            if let Some(owner) = owner.upgrade() {
                refresh(&owner);
            }
        }
    });
    let previous = owner.borrow_mut().dialog.take();
    if let Some(previous) = previous
        && let Err(error) = previous.window.hide()
    {
        owner.borrow_mut().dialog = Some(previous);
        report_origin(owner, origin, &error.to_string());
        return;
    }
    owner.borrow_mut().dialog = Some(controller.clone());
    refresh(owner);
    if let Err(error) = controller.window.show() {
        report_origin(owner, origin, &error.to_string());
    }
}

fn close(owner: &WeakOwner, controller: &std::rc::Weak<Controller>) {
    let (Some(owner), Some(controller)) = (owner.upgrade(), controller.upgrade()) else {
        return;
    };
    if !current(&owner, &controller) {
        return;
    }
    if let Err(error) = controller.window.hide() {
        report_origin(&owner, controller.origin, &error.to_string());
        return;
    }
    owner.borrow_mut().dialog.take();
}

fn report_origin(owner: &SharedOwner, origin: runtime::DocumentId, error: &str) {
    if let Some((_, host)) = owner
        .borrow()
        .hosts
        .iter()
        .find(|(_, host)| host.borrow().document_id() == origin)
        && let Some(app) = &host.borrow().window.app
    {
        app.set_message(error.into());
    }
}
