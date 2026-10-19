use std::{cell::RefCell, rc::Rc};

use editor_core::shortcuts::{Bindings, Chord, Command, CommandRow, Snapshot};
use slint::{ComponentHandle, Model, VecModel, platform::Key};

use crate::{
    AppWindow, ShortcutRow, ShortcutsWindow,
    runtime::{DocumentAccess, SharedSession, lock},
    session::{Actor, Session},
    viewport::key_text,
};

struct Form {
    bindings: Bindings,
    rows: Rc<VecModel<ShortcutRow>>,
    selected: Command,
    snapshot: String,
    dirty: bool,
}

pub struct Controller {
    pub window: ShortcutsWindow,
    form: Rc<RefCell<Form>>,
}

fn keys() -> Vec<String> {
    std::iter::once("Unbound".into())
        .chain(('A'..='Z').map(|key| key.to_string()))
        .chain((1..=12).map(|key| format!("F{key}")))
        .chain(["Delete".into(), "Backspace".into()])
        .collect()
}

fn load_slot(window: &ShortcutsWindow, slot: Option<&Chord>, alternate: bool) {
    let key = slot
        .and_then(|slot| keys().iter().position(|key| key == &slot.key))
        .unwrap_or(0) as i32;
    let control = slot.is_some_and(|slot| slot.control);
    let shift = slot.is_some_and(|slot| slot.shift);
    if alternate {
        window.set_alternate_key(key);
        window.set_alternate_control(control);
        window.set_alternate_shift(shift);
    } else {
        window.set_primary_key(key);
        window.set_primary_control(control);
        window.set_primary_shift(shift);
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
    let slots = form.bindings.slots(form.selected);
    load_slot(window, slots[0].as_ref(), false);
    load_slot(window, slots[1].as_ref(), true);
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
    match session.find_editor_commands(&window.get_search(), Some(&current.bindings)) {
        Ok(result) => {
            window.set_search_error("".into());
            window.set_result_count(
                format!("{} of {} commands", result.matched, result.total).into(),
            );
            window.set_has_matches(!result.commands.is_empty());
            if !result
                .commands
                .iter()
                .any(|row| row.command == current.selected)
                && let Some(first) = result.commands.first()
            {
                current.selected = first.command;
            }
            load_selected(window, &current);
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
    window.set_can_save(current.dirty && !stale && state["ready"] == true && validation.is_ok());
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
    load_selected(window, &current);
    drop(current);
    refresh_with_snapshot(window, session, &snapshot, form);
}

fn read_slot(window: &ShortcutsWindow, alternate: bool) -> Option<Chord> {
    let (index, control, shift) = if alternate {
        (
            window.get_alternate_key(),
            window.get_alternate_control(),
            window.get_alternate_shift(),
        )
    } else {
        (
            window.get_primary_key(),
            window.get_primary_control(),
            window.get_primary_shift(),
        )
    };
    (index > 0).then(|| Chord {
        key: keys().get(index as usize).cloned().unwrap_or_default(),
        control,
        shift,
    })
}

impl Controller {
    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

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
        }));
        window.set_keys(
            Rc::new(VecModel::from(
                keys()
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<slint::SharedString>>(),
            ))
            .into(),
        );
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
                        if let Ok(command) = serde_json::from_value::<Command>(
                            serde_json::Value::String(command.to_string()),
                        ) {
                            form.borrow_mut().selected = command;
                            load_selected(&window, &form.borrow());
                            refresh(&window, session, &form);
                        }
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
                if !window.get_has_matches() {
                    return;
                }
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        let mut current = form.borrow_mut();
                        let command = current.selected;
                        *current.bindings.slots_mut(command) =
                            [read_slot(&window, false), read_slot(&window, true)];
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
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.window.set_search("".into());
        adopt(&self.window, session, &self.form);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }
}

fn event_key(text: &str) -> Option<String> {
    if text == key_text(Key::Delete) {
        return Some("Delete".into());
    }
    if text == key_text(Key::Backspace) {
        return Some("Backspace".into());
    }
    let functions = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    functions
        .into_iter()
        .position(|key| text == key_text(key))
        .map(|index| format!("F{}", index + 1))
        .or_else(|| editor_core::shortcuts::canonical_key(text))
}

fn dispatch_key(
    app: &AppWindow,
    session: &mut Session,
    text: &str,
    control: bool,
    shift: bool,
    alt: bool,
    meta: bool,
) -> bool {
    let Some(key) = event_key(text) else {
        return false;
    };
    let Some(command) = session.resolve_editor_shortcut(&key, control, shift, alt, meta) else {
        return false;
    };
    let aspect = app.get_viewport_width() / app.get_viewport_height().max(1.0);
    if let Err(error) = session.execute_editor_command(Actor::User, command, aspect) {
        app.set_message(error.into());
    }
    true
}

pub struct ViewportKey<'a> {
    pub text: &'a str,
    pub down: bool,
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

pub fn route_viewport_key(
    app: &AppWindow,
    session: &SharedSession,
    key: ViewportKey<'_>,
    fixed_command: impl FnOnce(&str, bool, bool) -> bool,
    navigation: impl FnOnce(&str, bool) -> bool,
) -> bool {
    if !key.down {
        return navigation(key.text, false);
    }
    if key.alt || key.meta {
        return false;
    }
    let configured = dispatch_key(
        app,
        &mut lock(session),
        key.text,
        key.control,
        key.shift,
        key.alt,
        key.meta,
    );
    if configured || fixed_command(key.text, key.control, key.shift) {
        return true;
    }
    !key.control && navigation(key.text, true)
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
            .map(Chord::label)
            .unwrap_or_default()
            .into(),
    );
}

#[cfg(test)]
mod tests;
