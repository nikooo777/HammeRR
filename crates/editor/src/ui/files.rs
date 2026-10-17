use super::*;
use editor_core::runtime::{DocumentAccess, DocumentContinuation};

pub(super) fn file_access(ui: &SharedUi) -> DocumentAccess {
    let state = ui.borrow();
    match &state._document {
        Some(document) => DocumentAccess::for_document(document.clone()),
        None => DocumentAccess::unmanaged(state.session.clone()),
    }
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
        let result = access.with_session(|session| {
            session.save_for_generation(Actor::User, None, Some(generation))
        });
        report(app, result.map(|p| format!("Saved {}", p.display())));
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Save map")
        .set_file_name("untitled.vmf")
        .add_filter("Valve Map File", &["vmf"]);
    let (app, owner, continuation) = (app.as_weak(), Rc::downgrade(ui), access.downgrade());
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        let Some(_ui) = owner.upgrade() else { return };
        complete_file_action(&app, &continuation, |session| {
            session
                .save_for_generation(Actor::User, Some(file.path()), Some(generation))
                .map(|path| format!("Saved {}", path.display()))
        });
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
    let result = access.with_shared(|session| {
        runtime::start_compile_for_generation(session, Actor::User, path, play, Some(generation))
    });
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

#[cfg(test)]
mod admission_tests;
