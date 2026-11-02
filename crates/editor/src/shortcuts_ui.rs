use std::{cell::RefCell, rc::Rc};

use editor_core::shortcuts::{
    Binding, Bindings, Chord, Command, CommandRow, LOGICAL_KEYS, PHYSICAL_KEYS, Snapshot,
};
use slint::{ComponentHandle, Model, VecModel};

use crate::{
    AppWindow, ShortcutBinding, ShortcutRow, ShortcutStroke, ShortcutsWindow,
    runtime::DocumentAccess,
    session::{Actor, Session},
};

struct Form {
    bindings: Bindings,
    rows: Rc<VecModel<ShortcutRow>>,
    selected: Command,
    snapshot: String,
    dirty: bool,
    edit_error: Option<String>,
}

pub struct Controller {
    pub window: ShortcutsWindow,
    form: Rc<RefCell<Form>>,
}

fn key_model(keys: &[&str]) -> slint::ModelRc<slint::SharedString> {
    Rc::new(VecModel::from(
        std::iter::once("Choose key")
            .chain(keys.iter().copied())
            .map(Into::into)
            .collect::<Vec<_>>(),
    ))
    .into()
}

fn load_stroke(chord: Option<&Chord>) -> ShortcutStroke {
    chord.map_or_else(ShortcutStroke::default, |chord| ShortcutStroke {
        key: chord.key.as_str().into(),
        physical: chord.physical,
        control: chord.control,
        shift: chord.shift,
        alt: chord.alt,
        meta: chord.meta,
    })
}

fn load_slot(binding: Option<&Binding>) -> ShortcutBinding {
    let strokes = binding.map(Binding::strokes).unwrap_or_default();
    ShortcutBinding {
        count: strokes.len() as i32,
        first: load_stroke(strokes.first()),
        second: load_stroke(strokes.get(1)),
    }
}

fn load_selected(window: &ShortcutsWindow, form: &Form) {
    window.set_selected(
        serde_json::to_value(form.selected)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
    );
    window.set_selected_label(form.selected.label().into());
    window.set_selected_camera(form.selected.is_camera());
    let slots = form.bindings.slots(form.selected);
    window.set_primary(load_slot(slots[0].as_ref()));
    window.set_alternate(load_slot(slots[1].as_ref()));
}

fn refresh(window: &ShortcutsWindow, session: &Session, form: &RefCell<Form>) {
    refresh_with_snapshot(
        window,
        session,
        &session.keyboard_shortcuts_snapshot(),
        form,
    );
}

fn command_rows(rows: Vec<CommandRow>) -> Vec<ShortcutRow> {
    rows.into_iter()
        .map(|row| ShortcutRow {
            command: serde_json::to_value(row.command)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            label: row.label.into(),
            current: row.display.into(),
            factory: row.factory_display.into(),
        })
        .collect()
}

fn refresh_with_snapshot(
    window: &ShortcutsWindow,
    session: &Session,
    snapshot: &Snapshot,
    form: &RefCell<Form>,
) {
    let mut current = form.borrow_mut();
    if !current.dirty && current.snapshot != snapshot.stamp() {
        current.bindings = snapshot.current.clone();
        current.snapshot = snapshot.stamp();
        current.edit_error = None;
        load_selected(window, &current);
    }
    match session.find_editor_commands(&window.get_search(), Some(&current.bindings)) {
        Ok(result) => {
            window.set_search_error("".into());
            window.set_result_count(
                format!("{} of {} commands", result.matched, result.total).into(),
            );
            window.set_has_matches(!result.commands.is_empty());
            window.set_selected_visible(
                result
                    .commands
                    .iter()
                    .any(|row| row.command == current.selected),
            );
            let rows = command_rows(result.commands);
            if rows.len() != current.rows.row_count() {
                current.rows.set_vec(rows);
            } else {
                for (index, row) in rows.into_iter().enumerate() {
                    if current.rows.row_data(index).as_ref() != Some(&row) {
                        current.rows.set_row_data(index, row);
                    }
                }
            }
        }
        Err(error) => window.set_search_error(error.into()),
    }
    let state = snapshot.report();
    let stale = current.snapshot != snapshot.stamp();
    let validation = current.bindings.clone().validated();
    window.set_can_save(
        current.dirty
            && !stale
            && state["ready"] == true
            && validation.is_ok()
            && current.edit_error.is_none(),
    );
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(if let Some(error) = &snapshot.error {
        format!("Saved shortcuts unavailable: {error}. Current valid bindings remain active. Reload saved must succeed or explicitly Reset saved shortcuts before Save.")
    } else if stale {
        "Shortcuts changed in another editor. Reload saved discards this form and loads the current saved bindings.".into()
    } else if let Some(error) = &current.edit_error {
        error.clone()
    } else if let Err(error) = validation {
        error
    } else if current.dirty {
        "Form changes are ready. Save shortcuts writes and activates all command bindings together.".into()
    } else {
        "Current shortcuts are active. Changing this form leaves them unchanged until Save shortcuts.".into()
    }.into());
}

fn adopt(window: &ShortcutsWindow, session: &Session, form: &RefCell<Form>) {
    let snapshot = session.keyboard_shortcuts_snapshot();
    let mut current = form.borrow_mut();
    current.bindings = snapshot.current.clone();
    current.snapshot = snapshot.stamp();
    current.dirty = false;
    current.edit_error = None;
    load_selected(window, &current);
    drop(current);
    refresh_with_snapshot(window, session, &snapshot, form);
}

fn read_stroke(stroke: ShortcutStroke) -> Chord {
    Chord {
        key: stroke.key.to_string(),
        physical: stroke.physical,
        control: stroke.control,
        shift: stroke.shift,
        alt: stroke.alt,
        meta: stroke.meta,
    }
}

fn read_slot(binding: ShortcutBinding) -> Result<Option<Binding>, String> {
    match binding.count {
        0 => Ok(None),
        1 => Ok(Some(Binding::Single(read_stroke(binding.first)))),
        2 => Ok(Some(Binding::Sequence {
            sequence: [read_stroke(binding.first), read_stroke(binding.second)],
        })),
        _ => Err("Choose Unbound, Single stroke or Two-stroke sequence".into()),
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = ShortcutsWindow::new()?;
        let bindings = Bindings::default();
        let rows = Rc::new(VecModel::from(command_rows(bindings.command_rows())));
        window.set_commands(rows.clone().into());
        let form = Rc::new(RefCell::new(Form {
            bindings,
            rows,
            selected: Command::Undo,
            snapshot: String::new(),
            dirty: false,
            edit_error: None,
        }));
        window.set_logical_keys(key_model(LOGICAL_KEYS));
        window.set_physical_keys(key_model(PHYSICAL_KEYS));
        window.on_filter_commands({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        refresh(&window, session, &form);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_choose({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |command| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        let command = serde_json::from_value::<Command>(serde_json::Value::String(
                            command.to_string(),
                        ))
                        .map_err(|_| "Choose an available shortcut command")?;
                        form.borrow_mut().selected = command;
                        load_selected(&window, &form.borrow());
                        refresh(&window, session, &form);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_edited({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        let mut current = form.borrow_mut();
                        let command = current.selected;
                        let bindings = read_slot(window.get_primary()).and_then(|primary| {
                            read_slot(window.get_alternate()).map(|alternate| [primary, alternate])
                        });
                        match bindings {
                            Ok(bindings) => {
                                *current.bindings.slots_mut(command) = bindings;
                                current.edit_error = None;
                            }
                            Err(error) => current.edit_error = Some(error),
                        }
                        current.dirty = true;
                        drop(current);
                        refresh(&window, session, &form);
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        for action in ["save", "reload", "reset"] {
            let (callback_window, target, form) =
                (window.as_weak(), access.downgrade(), form.clone());
            let callback = move || {
                let Some(window) = callback_window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        let (snapshot, bindings) = if action == "save" {
                            let current = form.borrow();
                            (current.snapshot.clone(), Some(current.bindings.clone()))
                        } else {
                            (session.shortcuts_stamp(), None)
                        };
                        match session.change_keyboard_shortcuts(Actor::User, &snapshot, action, bindings) {
                            Ok(state) => {
                                adopt(&window, session, &form);
                                window.set_message(if state["write_committed"] == true && state["ready"] != true { "Shortcuts were written and activated, but saved storage could not reload. Review its error." } else if action == "reload" && state["stored"] != true { "Factory shortcuts activated; no saved file." } else if action == "reload" { "Saved shortcuts loaded and activated." } else { "Keyboard shortcuts saved and activated." }.into());
                            }
                            Err(error) => {
                                window.set_message(error.into());
                                refresh(&window, session, &form);
                            }
                        }
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            };
            match action {
                "save" => window.on_save(callback),
                "reload" => window.on_reload(callback),
                _ => window.on_reset(callback),
            }
        }
        window.on_cancel({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        if self.window.window().is_visible() {
            refresh(&self.window, session, &self.form);
        } else {
            self.window.set_message("".into());
            self.window.set_search("".into());
            adopt(&self.window, session, &self.form);
        }
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }
}

pub fn refresh_hint(app: &AppWindow, session: &Session) {
    let snapshot = session.keyboard_shortcuts_snapshot();
    app.set_shortcuts_error(snapshot.error.is_some());
    app.set_frame_shortcut(
        snapshot
            .current
            .slots(Command::FrameSelection)
            .iter()
            .flatten()
            .next()
            .map(Binding::label)
            .unwrap_or_default()
            .into(),
    );
}
