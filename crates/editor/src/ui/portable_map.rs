use std::path::PathBuf;

use slint::ComponentHandle;

use crate::{
    PortableMapWindow,
    runtime::DocumentAccess,
    session::{
        Actor, Session,
        portable_maps::{Action, Options},
    },
};

pub(super) struct Controller {
    pub(super) window: PortableMapWindow,
}

fn status_text(state: &serde_json::Value) -> String {
    if state["enabled"] == true {
        format!(
            "{} files · {} MiB decoded · {} MiB compressed",
            state["files"],
            state["decoded_bytes"].as_u64().unwrap_or(0) / 1048576,
            state["compressed_bytes"].as_u64().unwrap_or(0) / 1048576
        )
    } else {
        "The open VMF has no asset package.".into()
    }
}

fn present(window: &PortableMapWindow, session: &Session) {
    window.set_status(status_text(&session.portable_map_state()).into());
}

#[derive(Clone, Copy)]
enum Field {
    Source,
    Destination,
    Companion,
    ContentFolder,
}

fn browse(window: &slint::Weak<PortableMapWindow>, field: Field) {
    let Some(view) = window.upgrade() else {
        return;
    };
    let current = match field {
        Field::Source => view.get_source_vmf(),
        Field::Destination => view.get_destination(),
        Field::Companion => view.get_companion_bsp(),
        Field::ContentFolder => view
            .get_content_roots()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .into(),
    };
    let mut dialog = rfd::AsyncFileDialog::new();
    let start = path(current).or_else(|| path(view.get_source_vmf()));
    if let Some(directory) = start.as_deref().and_then(|start| {
        if start.is_dir() {
            Some(start)
        } else {
            start.parent()
        }
    }) {
        dialog = dialog.set_directory(directory);
    }
    dialog = match field {
        Field::Source => dialog
            .set_title("Choose source VMF")
            .add_filter("Valve Map File", &["vmf"]),
        Field::Destination if view.get_action_index() == 3 => dialog
            .set_title("Export assets ZIP")
            .add_filter("ZIP archive", &["zip"]),
        Field::Destination => dialog
            .set_title("Choose destination VMF")
            .add_filter("Valve Map File", &["vmf"]),
        Field::Companion => dialog
            .set_title("Choose companion BSP")
            .add_filter("Source map", &["bsp"]),
        Field::ContentFolder => dialog.set_title("Add content folder"),
    };
    let window = window.clone();
    slint::spawn_local(async move {
        let picked = match field {
            Field::Destination => dialog.save_file().await,
            Field::ContentFolder => dialog.pick_folder().await,
            Field::Source | Field::Companion => dialog.pick_file().await,
        };
        let (Some(file), Some(view)) = (picked, window.upgrade()) else {
            return;
        };
        let chosen = file.path().display().to_string();
        match field {
            Field::Source => view.set_source_vmf(chosen.into()),
            Field::Destination => view.set_destination(chosen.into()),
            Field::Companion => view.set_companion_bsp(chosen.into()),
            Field::ContentFolder => {
                let roots = view.get_content_roots();
                let mut lines = roots
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if !lines.iter().any(|line| line.trim() == chosen) {
                    lines.push(chosen);
                }
                view.set_content_roots(lines.join("\n").into());
            }
        }
    })
    .expect("event loop is running");
}

fn path(value: slint::SharedString) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()))
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = PortableMapWindow::new()?;
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.on_browse_source({
            let window = window.as_weak();
            move || browse(&window, Field::Source)
        });
        window.on_browse_destination({
            let window = window.as_weak();
            move || browse(&window, Field::Destination)
        });
        window.on_browse_companion({
            let window = window.as_weak();
            move || browse(&window, Field::Companion)
        });
        window.on_add_content_folder({
            let window = window.as_weak();
            move || browse(&window, Field::ContentFolder)
        });
        window.on_apply({
            let window = window.as_weak();
            let access = access.downgrade();
            move || {
                let Some(view) = window.upgrade() else {
                    return;
                };
                let action = match view.get_action_index() {
                    0 => Action::Attach,
                    1 => Action::Share,
                    2 => Action::ExportPlain,
                    _ => Action::ExportAssets,
                };
                let options = Options {
                    action,
                    source_vmf: if matches!(action, Action::Attach) {
                        None
                    } else {
                        path(view.get_source_vmf())
                    },
                    destination: if matches!(action, Action::Attach) {
                        None
                    } else {
                        path(view.get_destination())
                    },
                    companion_bsp: if matches!(action, Action::Attach | Action::Share) {
                        path(view.get_companion_bsp())
                    } else {
                        None
                    },
                    content_roots: if matches!(action, Action::Attach | Action::Share) {
                        view.get_content_roots()
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| PathBuf::from(line.trim()))
                            .collect()
                    } else {
                        Vec::new()
                    },
                };
                let target = access.clone();
                let completed = window.clone();
                view.set_busy(true);
                view.set_message("".into());
                let result = std::thread::Builder::new()
                    .name("portable-map".into())
                    .spawn(move || {
                        let result = target.upgrade().and_then(|access| {
                            access.with_session(|session| {
                                let report = session.portable_map(Actor::User, options)?;
                                Ok((report, session.portable_map_state()))
                            })
                        });
                        let _ = completed.upgrade_in_event_loop(move |window| {
                            window.set_busy(false);
                            match result {
                                Ok((report, state)) => {
                                    window.set_failed(false);
                                    window.set_status(status_text(&state).into());
                                    let mut message = report["path"].as_str().map_or_else(
                                        || {
                                            "Assets attached. Use Save to keep the portable VMF."
                                                .into()
                                        },
                                        |path| format!("Exported {path}"),
                                    );
                                    if let Some(warning) = report["warning"].as_str() {
                                        message.push('\n');
                                        message.push_str(warning);
                                    }
                                    window.set_message(message.into());
                                }
                                Err(error) => {
                                    window.set_failed(true);
                                    window.set_message(error.into());
                                }
                            }
                        });
                    });
                if let Err(error) = result {
                    view.set_busy(false);
                    view.set_failed(true);
                    view.set_message(error.to_string().into());
                }
            }
        });
        Ok(Self { window })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        present(&self.window, session);
        self.window.show()
    }
}
