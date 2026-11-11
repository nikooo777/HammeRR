use super::*;

pub(super) fn bind(owner: &SharedOwner) {
    let window = owner.borrow().window.clone_strong();
    window.on_select_document({
        let owner = Rc::downgrade(owner);
        move |selector| select(&owner, selector.to_string())
    });
    window.on_open_documents({
        let owner = Rc::downgrade(owner);
        move || open_documents(&owner)
    });
    window.window().on_close_requested({
        let owner = Rc::downgrade(owner);
        move || {
            close_workspace(&owner);
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
}

pub(super) fn bind_document(owner: &WeakOwner, app: &AppWindow) {
    app.on_select_workspace_document({
        let owner = owner.clone();
        move |selector| select(&owner, selector.to_string())
    });
    app.on_close_workspace_document({
        let owner = owner.clone();
        move || close_active(&owner)
    });
}

fn select(owner: &WeakOwner, selector: String) {
    let Some(retained) = owner.upgrade() else {
        return;
    };
    let Some(workspace) = retained.borrow().workspace.clone() else {
        return;
    };
    let owner = owner.clone();
    slint::spawn_local(async move {
        let result = workspace.show_document(&selector).await;
        if let Some(owner) = owner.upgrade() {
            if let Err(error) = result {
                report(&owner, &error);
            }
            refresh(&owner);
        }
    })
    .expect("The UI event loop is running");
}

fn open_documents(owner: &WeakOwner) {
    let Some(owner) = owner.upgrade() else { return };
    let origin = {
        let state = owner.borrow();
        state.active.or_else(|| {
            state
                .hosts
                .first()
                .map(|(_, host)| host.borrow().document_id())
        })
    };
    if let Some(origin) = origin {
        controls::open(&owner, origin);
    }
}

fn close_active(owner: &WeakOwner) {
    let Some(owner) = owner.upgrade() else { return };
    let selector = {
        let state = owner.borrow();
        state.active.and_then(|active| {
            state.hosts.iter().find_map(|(selector, host)| {
                (host.borrow().document_id() == active).then(|| selector.clone())
            })
        })
    };
    if let Some(selector) = selector {
        controls::request_close(&owner, &selector);
    } else {
        close_workspace(&Rc::downgrade(&owner));
    }
}

fn close_workspace(owner: &WeakOwner) {
    let Some(retained) = owner.upgrade() else {
        return;
    };
    open_documents(owner);
    let controller = retained.borrow().dialog.clone();
    if let Some(controller) = controller {
        controls::review_quit(&retained, &controller);
    }
}

pub(super) fn report(owner: &SharedOwner, message: &str) {
    let state = owner.borrow();
    if let Some(active) = state.active
        && let Some((_, host)) = state
            .hosts
            .iter()
            .find(|(_, host)| host.borrow().document_id() == active)
        && let Some(app) = &host.borrow().window.app
    {
        app.set_message(message.into());
        return;
    }
    state.window.set_message(message.into());
}

pub(super) fn refresh(owner: &SharedOwner) {
    let (workspace, active) = {
        let state = owner.borrow();
        let Some(workspace) = &state.workspace else {
            return;
        };
        let active = state.active.and_then(|active| {
            state.hosts.iter().find_map(|(selector, host)| {
                (host.borrow().document_id() == active).then(|| selector.clone())
            })
        });
        (workspace.clone(), active)
    };
    let report = match workspace.list_documents() {
        Ok(report) => report,
        Err(error) => {
            self::report(owner, &error);
            return;
        }
    };
    let documents = report["documents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|document| {
            let selector = document["document_id"].as_str().unwrap_or_default();
            WorkspaceDocumentTab {
                selector: selector.into(),
                title: document["title"].as_str().unwrap_or("Untitled map").into(),
                selected: active.as_deref() == Some(selector),
                enabled: document["lifecycle"] == "live",
                hidden: document["visibility"] == "hidden",
            }
        })
        .collect::<Vec<_>>();
    let documents = ModelRc::new(VecModel::from(documents));
    let mut state = owner.borrow_mut();
    state.window.set_workspace_documents(documents.clone());
    for (_, host) in &state.hosts {
        if let Some(app) = &host.borrow().window.app {
            app.set_workspace_documents(documents.clone());
        }
    }
    state.presentation_dirty = false;
}
