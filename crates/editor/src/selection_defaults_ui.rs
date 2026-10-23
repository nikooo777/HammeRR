use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::{
    SelectionDefaultRow, SelectionDefaultsWindow, SelectionProfileRow,
    runtime::DocumentAccess,
    session::{Actor, Session, selection_preferences::ProfileAction},
};

pub struct Controller {
    pub window: SelectionDefaultsWindow,
    snapshot: Rc<RefCell<String>>,
}

fn enabled(value: &Value) -> slint::SharedString {
    match value.as_bool() {
        Some(true) => "On".into(),
        Some(false) => "Off".into(),
        None => "Unavailable".into(),
    }
}

fn refresh(window: &SelectionDefaultsWindow, session: &Session, snapshot: &RefCell<String>) {
    let state = session.selection_profiles_state();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    let rows: Vec<_> = [
        ("world_brushes", "World brushes"),
        ("brush_entities", "Brush entities"),
        ("point_entities", "Point entities & models"),
    ]
    .into_iter()
    .map(|(key, label)| SelectionDefaultRow {
        label: label.into(),
        current: enabled(&state["current"][key]),
        defaults: enabled(&state["defaults"][key]),
    })
    .collect();
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    let profiles: Vec<_> = state["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| SelectionProfileRow {
            id: profile["id"].as_str().unwrap().into(),
            name: profile["name"].as_str().unwrap().into(),
            filters: format!(
                "World {} · brush entities {} · points {}",
                enabled(&profile["filters"]["world_brushes"]),
                enabled(&profile["filters"]["brush_entities"]),
                enabled(&profile["filters"]["point_entities"])
            )
            .into(),
        })
        .collect();
    if !profiles
        .iter()
        .any(|profile| profile.id == window.get_selected_profile())
    {
        window.set_selected_profile("".into());
    }
    window.set_profiles(Rc::new(VecModel::from(profiles)).into());
    window.set_defaults(
        if state["ready"] != true {
            "Saved defaults could not load."
        } else if state["stored"] == true {
            "Saved startup defaults"
        } else {
            "Factory defaults · no saved file"
        }
        .into(),
    );
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(
        state["error"]
            .as_str()
            .or(state["storage_error"].as_str())
            .unwrap_or(
                "Apply changes only this document and ends pending object-selection gestures.",
            )
            .into(),
    );
    window.set_ready(state["ready"] == true);
}

fn act(
    window: &SelectionDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let result = session.change_selection_preferences(Actor::User, action, &snapshot.borrow());
    match result {
        Ok(state) if state["write_committed"] == true && state["ready"] != true => window.set_message("Selection defaults were written but could not reload. Review the storage error before continuing.".into()),
        Ok(_) => window.set_message(
            match action {
                "save" => "Current selection filters saved for future documents.",
                "apply" => "Saved selection filters applied to this document.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Selection defaults reloaded. Current filters are unchanged.",
            }
            .into(),
        ),
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

fn profile_act(
    window: &SelectionDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let id = window.get_selected_profile().to_string();
    let name = window.get_profile_name().to_string();
    let operation = match action {
        "save" => ProfileAction::Save { name },
        "replace" => ProfileAction::Replace { id },
        "apply" => ProfileAction::Apply { id },
        "rename" => ProfileAction::Rename { id, name },
        "delete" => ProfileAction::Delete { id },
        _ => unreachable!(),
    };
    match session.change_selection_profile(Actor::User, operation, &snapshot.borrow()) {
        Ok(state) => {
            if action != "delete" {
                window.set_selected_profile(state["profile_id"].as_str().unwrap().into());
            }
            window.set_message(if state["write_committed"] == true && state["ready"] != true {
                "Selection profiles were written but could not reload. Review the storage error before continuing."
            } else if action == "apply" {
                "Selected profile applied to this document. Startup defaults are unchanged."
            } else {
                "Selection profiles saved. Current filters and startup defaults are unchanged."
            }.into());
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = SelectionDefaultsWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        let callback = |action: &'static str| {
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        act(&window, session, &snapshot, action);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        };
        window.on_save(callback("save"));
        window.on_apply(callback("apply"));
        window.on_reload(callback("reload"));
        window.on_reset(callback("reset"));
        let profile_callback = |action: &'static str| {
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        profile_act(&window, session, &snapshot, action);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        };
        window.on_save_profile(profile_callback("save"));
        window.on_replace_profile(profile_callback("replace"));
        window.on_apply_profile(profile_callback("apply"));
        window.on_rename_profile(profile_callback("rename"));
        window.on_delete_profile(profile_callback("delete"));
        window.on_choose_profile({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move |id| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        let state = session.selection_profiles_state();
                        if state["ready"] != true {
                            return Err(
                                "Reload defaults before choosing a selection profile".into()
                            );
                        }
                        let profile = state["profiles"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|profile| profile["id"].as_str() == Some(id.as_str()))
                            .ok_or("Selection profile no longer exists; refresh the list")?;
                        window.set_selected_profile(id.clone());
                        window.set_profile_name(profile["name"].as_str().unwrap().into());
                        refresh(&window, session, &snapshot);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, snapshot })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.snapshot);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
