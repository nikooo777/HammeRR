use super::*;

/// Asks before throwing away unsaved changes, then runs `then`.
pub(super) fn confirm_discard(ui: &SharedUi, then: impl FnOnce() + 'static) {
    let shared = session_of(ui);
    let (dirty, generation, revision, partition) = {
        let session = lock(&shared);
        (
            session.document().is_dirty() || session.partition_preview_token().is_some(),
            session.document_generation(),
            session.document().revision(),
            session.partition_preview_token(),
        )
    };
    if !dirty {
        then();
        return;
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("The map has unsaved changes. Discard them?")
        .set_buttons(rfd::MessageButtons::YesNoCancel);
    slint::spawn_local(async move {
        if dialog.show().await == rfd::MessageDialogResult::Yes {
            let mut session = lock(&shared);
            if session.document_generation() != generation
                || session.document().revision() != revision
                || session.partition_preview_token() != partition
            {
                session.log(
                    Actor::System,
                    "Map changed while confirming discard. Try again.",
                );
                return;
            }
            drop(session);
            then();
        }
    })
    .expect("event loop is running");
}

pub(super) fn open_dialog(app: &AppWindow, ui: &SharedUi) {
    let start_dir = {
        let shared = session_of(ui);
        let session = lock(&shared);
        session
            .path()
            .and_then(|p| p.parent().map(Path::to_owned))
            .or_else(|| steam::sample_maps_dir(&session.game().game_dir.join("gameinfo.txt")))
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Open map")
        .add_filter("Valve Map File", &["vmf"]);
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = dialog.pick_file().await else {
            return;
        };
        let path = file.path().to_owned();
        let ui2 = ui.clone();
        confirm_discard(&ui, move || {
            let result = lock(&session_of(&ui2)).open(Actor::User, &path, true);
            if let Some(app) = app.upgrade() {
                report(&app, result);
            }
        });
    })
    .expect("event loop is running");
}

pub(super) fn save(app: &AppWindow, ui: &SharedUi) {
    let has_path = lock(&session_of(ui)).path().is_some();
    if has_path {
        let result = lock(&session_of(ui)).save(Actor::User, None);
        report(app, result.map(|p| format!("Saved {}", p.display())));
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Save map")
        .set_file_name("untitled.vmf")
        .add_filter("Valve Map File", &["vmf"]);
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        let result = lock(&session_of(&ui)).save(Actor::User, Some(file.path()));
        if let Some(app) = app.upgrade() {
            report(&app, result.map(|p| format!("Saved {}", p.display())));
        }
    })
    .expect("event loop is running");
}

pub(super) fn start_compile(
    app: &AppWindow,
    session: SharedSession,
    path: Option<&Path>,
    play: bool,
) {
    if let Err(error) = runtime::start_compile(&session, Actor::User, path, play) {
        app.set_message(error.into());
    }
}

pub(super) fn compile_map(app: &AppWindow, ui: &SharedUi, play: bool) {
    let session = session_of(ui);
    if lock(&session).path().is_some() {
        start_compile(app, session, None, play);
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Compile map")
        .set_file_name("untitled.bsp")
        .add_filter("Source map", &["bsp"]);
    let app = app.as_weak();
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        if let Some(app) = app.upgrade() {
            start_compile(&app, session, Some(file.path()), play);
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
