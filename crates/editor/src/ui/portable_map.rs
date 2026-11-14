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

fn present(window: &PortableMapWindow, session: &Session) {
    let state = session.portable_map_state();
    let text = if state["enabled"] == true {
        format!(
            "{} files · {} MiB decoded · {} MiB compressed",
            state["files"],
            state["decoded_bytes"].as_u64().unwrap_or(0) / 1048576,
            state["compressed_bytes"].as_u64().unwrap_or(0) / 1048576
        )
    } else {
        "The open VMF has no asset package.".into()
    };
    window.set_status(text.into());
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
                                    window.set_status(
                                        format!(
                                            "{} attached files · {} decoded bytes",
                                            state["files"], state["decoded_bytes"]
                                        )
                                        .into(),
                                    );
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
