use super::*;
use editor_core::runtime::{DocumentAccess, DocumentContinuation};

pub(super) fn file_access(ui: &SharedUi) -> DocumentAccess {
    ui.borrow().document_access()
}

pub(super) fn complete_terminal_close(
    app: &slint::Weak<AppWindow>,
    owner: &WeakUi,
    continuation: &DocumentContinuation,
    review: &runtime::DocumentCloseReview,
    discard_changes: bool,
) -> Result<bool, String> {
    let (Some(_ui), Some(app)) = (owner.upgrade(), app.upgrade()) else {
        return Ok(false);
    };
    let result = continuation
        .upgrade()
        .and_then(|access| access.try_commit_reviewed_close(review, discard_changes));
    match result {
        Ok(()) => Ok(true),
        Err(error) => {
            app.set_message(error.clone().into());
            Err(error)
        }
    }
}

fn finish_terminal_close(
    app: &slint::Weak<AppWindow>,
    owner: &WeakUi,
    continuation: &DocumentContinuation,
    review: &runtime::DocumentCloseReview,
    discard_changes: bool,
    host: &WeakHost,
) {
    let Some(retained_host) = host.upgrade() else {
        return;
    };
    let workspace = retained_host.borrow().workspace_close.clone();
    if let Some((workspace, selector)) = workspace {
        if owner.upgrade().is_none() || app.upgrade().is_none() {
            return;
        }
        if let Err(error) = workspace.close_native(&selector, review, discard_changes)
            && let Some(app) = app.upgrade()
        {
            app.set_message(error.into());
        }
        return;
    }
    let terminal = retained_host.borrow().terminal_close.clone();
    if let Some(terminal) = terminal {
        if owner.upgrade().is_none() || app.upgrade().is_none() {
            return;
        }
        if let Err(error) = terminal.commit_native(review, discard_changes)
            && let Some(app) = app.upgrade()
        {
            app.set_message(error.into());
        }
        return;
    }
    if complete_terminal_close(app, owner, continuation, review, discard_changes) != Ok(true) {
        return;
    }
    DocumentHost::accept_terminal_close(host);
}

pub(super) fn request_terminal_close(app: &AppWindow, owner: &WeakUi, host: &WeakHost) {
    let Some(ui) = owner.upgrade() else {
        return;
    };
    let access = file_access(&ui);
    drop(ui);
    let review = match access.document_close_review() {
        Ok(review) => review,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let continuation = access.downgrade();
    let (app, owner, host) = (app.as_weak(), owner.clone(), host.clone());
    if !review.requires_discard() {
        finish_terminal_close(&app, &owner, &continuation, &review, false, &host);
        return;
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved changes and drafts")
        .set_description("Discard the reviewed unsaved map changes and pending drafts, and close this document? Other open documents remain available. Owned game and sound playback will stop; owned work will be cancelled and drained.")
        .set_buttons(rfd::MessageButtons::YesNoCancel);
    slint::spawn_local(async move {
        if dialog.show().await == rfd::MessageDialogResult::Yes {
            finish_terminal_close(&app, &owner, &continuation, &review, true, &host);
        }
    })
    .expect("event loop is running");
}

fn complete_file_action(
    app: &slint::Weak<AppWindow>,
    continuation: &DocumentContinuation,
    operation: impl FnOnce(&mut Session) -> Result<String, String>,
) {
    let Some(app) = app.upgrade() else { return };
    let result = continuation
        .upgrade()
        .and_then(|access| access.with_session(operation));
    report(&app, result);
}

pub(super) fn confirm_discard(
    app: &AppWindow,
    ui: &SharedUi,
    then: impl FnOnce(DocumentContinuation, editor_core::session::BufferReplacementReview) + 'static,
) {
    let access = file_access(ui);
    let observation = access.with_session(|session| {
        Ok((
            session.document().is_dirty() || session.partition_preview_token().is_some(),
            session.buffer_replacement_review(),
        ))
    });
    let (dirty, review) = match observation {
        Ok(observation) => observation,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let continuation = access.downgrade();
    if !dirty {
        then(continuation, review);
        return;
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("The map has unsaved changes. Discard them?")
        .set_buttons(rfd::MessageButtons::YesNoCancel);
    slint::spawn_local(async move {
        if dialog.show().await == rfd::MessageDialogResult::Yes {
            then(continuation, review);
        }
    })
    .expect("event loop is running");
}

pub(super) fn new_map(app: &AppWindow, ui: &SharedUi) {
    let weak = app.as_weak();
    let owner = Rc::downgrade(ui);
    confirm_discard(app, ui, move |continuation, review| {
        let Some(_ui) = owner.upgrade() else { return };
        complete_file_action(&weak, &continuation, |session| {
            session
                .new_map_reviewed(Actor::User, true, &review)
                .map(|()| "New map".into())
        });
    });
}

pub(super) fn open_dialog(app: &AppWindow, ui: &SharedUi) {
    let start_dir = file_access(ui).with_session(|session| {
        Ok(session
            .path()
            .and_then(|p| p.parent().map(Path::to_owned))
            .or_else(|| steam::sample_maps_dir(&session.game().game_dir.join("gameinfo.txt"))))
    });
    let start_dir = match start_dir {
        Ok(directory) => directory,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Open map")
        .add_filter("Valve Map File", &["vmf"]);
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    let (app, ui) = (app.as_weak(), Rc::downgrade(ui));
    slint::spawn_local(async move {
        let Some(file) = dialog.pick_file().await else {
            return;
        };
        let (Some(ui), Some(window)) = (ui.upgrade(), app.upgrade()) else {
            return;
        };
        let path = file.path().to_owned();
        let owner = Rc::downgrade(&ui);
        confirm_discard(&window, &ui, move |continuation, review| {
            let Some(_ui) = owner.upgrade() else { return };
            complete_file_action(&app, &continuation, |session| {
                session.open_reviewed(Actor::User, &path, true, &review)
            });
        });
    })
    .expect("event loop is running");
}

pub(super) fn save(app: &AppWindow, ui: &SharedUi) {
    save_map(app, ui, false);
}

pub(super) fn save_as(app: &AppWindow, ui: &SharedUi) {
    save_map(app, ui, true);
}

fn complete_save_dialog(
    app: &slint::Weak<AppWindow>,
    continuation: &DocumentContinuation,
    path: Option<&Path>,
    generation: u64,
) {
    let Some(path) = path else { return };
    complete_file_action(app, continuation, |session| {
        session
            .save_for_generation(Actor::User, Some(path), Some(generation))
            .map(|path| format!("Saved {}", path.display()))
    });
}

fn save_map(app: &AppWindow, ui: &SharedUi, choose_destination: bool) {
    let access = file_access(ui);
    let observation = access.with_session(|session| {
        Ok((
            session.path().map(Path::to_owned),
            session.document_generation(),
        ))
    });
    let (path, generation) = match observation {
        Ok(observation) => observation,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    if path.is_some() && !choose_destination {
        let result = access.with_session(|session| {
            session.save_for_generation(Actor::User, None, Some(generation))
        });
        report(app, result.map(|p| format!("Saved {}", p.display())));
        return;
    }
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title(if choose_destination {
            "Save map as"
        } else {
            "Save map"
        })
        .set_file_name("untitled.vmf")
        .add_filter("Valve Map File", &["vmf"]);
    if let Some(path) = path {
        if let Some(name) = path.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(directory) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            dialog = dialog.set_directory(directory);
        }
    }
    let (app, owner, continuation) = (app.as_weak(), Rc::downgrade(ui), access.downgrade());
    slint::spawn_local(async move {
        let file = dialog.save_file().await;
        let Some(_ui) = owner.upgrade() else { return };
        complete_save_dialog(
            &app,
            &continuation,
            file.as_ref().map(|file| file.path()),
            generation,
        );
    })
    .expect("event loop is running");
}

pub(super) fn start_compile(
    app: &AppWindow,
    access: &DocumentAccess,
    path: Option<&Path>,
    play: bool,
    generation: u64,
) {
    let result = access.start_compile(Actor::User, path, play, Some(generation));
    if let Err(error) = result {
        app.set_message(error.into());
    }
}

pub(super) fn compile_map(app: &AppWindow, ui: &SharedUi, play: bool) {
    let access = file_access(ui);
    let observation = access
        .with_session(|session| Ok((session.path().is_some(), session.document_generation())));
    let (has_path, generation) = match observation {
        Ok(observation) => observation,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    if has_path {
        start_compile(app, &access, None, play, generation);
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Compile map")
        .set_file_name("untitled.bsp")
        .add_filter("Source map", &["bsp"]);
    let (app, owner, continuation) = (app.as_weak(), Rc::downgrade(ui), access.downgrade());
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        if let (Some(app), Some(_ui)) = (app.upgrade(), owner.upgrade()) {
            match continuation.upgrade() {
                Ok(access) => start_compile(&app, &access, Some(file.path()), play, generation),
                Err(error) => app.set_message(error.into()),
            }
        }
    })
    .expect("event loop is running");
}

pub(super) fn show_game_log(app: &AppWindow, ui: &SharedUi) {
    let session = session_of(ui);
    let log = match lock(&session).game_log(game::DEFAULT_LOG_LINES) {
        Ok(log) => log,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let mut ui = ui.borrow_mut();
    if ui.game_log_window.is_none() {
        match GameLogWindow::new() {
            Ok(window) => {
                window.on_cancel({
                    let window = window.as_weak();
                    move || {
                        let _ = window.unwrap().hide();
                    }
                });
                ui.game_log_window = Some(window);
            }
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    if let Some(window) = &ui.game_log_window {
        window.set_content(log.text().into());
        if let Err(error) = window.show() {
            app.set_message(error.to_string().into());
        }
    }
}
