use std::{cell::RefCell, rc::Rc};

use editor_core::shortcuts::{Bindings, Chord, Command};
use slint::{ComponentHandle, VecModel, platform::Key};

use crate::{
    AppWindow, ShortcutRow, ShortcutsWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
    viewport::key_text,
};

struct Form {
    bindings: Bindings,
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
    let current = form.borrow();
    let factory = Bindings::default();
    let rows = Command::ALL.map(|command| ShortcutRow {
        command: serde_json::to_value(command)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        label: command.label().into(),
        current: current.bindings.label(command).into(),
        factory: factory.label(command).into(),
    });
    window.set_commands(Rc::new(VecModel::from(rows.to_vec())).into());
    let state = session.keyboard_shortcuts_state();
    let stale = current.snapshot != session.shortcuts_stamp();
    let validation = current.bindings.clone().validated();
    window.set_can_save(current.dirty && !stale && state["ready"] == true && validation.is_ok());
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(if let Some(error) = session.shortcuts_error_message() {
        format!("Saved shortcuts unavailable: {error}. Current valid bindings remain active. Reload saved must succeed or explicitly Reset saved shortcuts before Save.")
    } else if stale {
        "Shortcuts changed in another editor. Reload saved discards this form and loads the current saved bindings.".into()
    } else if let Err(error) = validation {
        error
    } else if current.dirty {
        "Form changes are ready. Save shortcuts writes and activates all five command bindings together.".into()
    } else {
        "Current shortcuts are active. Changing this form leaves them unchanged until Save shortcuts.".into()
    }.into());
}

fn adopt(window: &ShortcutsWindow, session: &Session, form: &RefCell<Form>) {
    let mut current = form.borrow_mut();
    current.bindings = session.shortcut_bindings().clone();
    current.snapshot = session.shortcuts_stamp();
    current.dirty = false;
    load_selected(window, &current);
    drop(current);
    refresh(window, session, form);
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
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = ShortcutsWindow::new()?;
        let form = Rc::new(RefCell::new(Form {
            bindings: Bindings::default(),
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
        window.on_choose({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |command| {
                let Ok(command) = serde_json::from_value::<Command>(serde_json::Value::String(
                    command.to_string(),
                )) else {
                    return;
                };
                form.borrow_mut().selected = command;
                load_selected(&window.unwrap(), &form.borrow());
                refresh(&window.unwrap(), &lock(&shared), &form);
            }
        });
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut current = form.borrow_mut();
                let command = current.selected;
                *current.bindings.slots_mut(command) =
                    [read_slot(&window, false), read_slot(&window, true)];
                current.dirty = true;
                drop(current);
                refresh(&window, &lock(&shared), &form);
            }
        });
        for action in ["save", "reload", "reset"] {
            let (callback_window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            let callback = move || {
                let window = callback_window.unwrap();
                let mut session = lock(&shared);
                let (snapshot, bindings) = if action == "save" {
                    let current = form.borrow();
                    (current.snapshot.clone(), Some(current.bindings.clone()))
                } else {
                    (session.shortcuts_stamp(), None)
                };
                match session.change_keyboard_shortcuts(Actor::User, &snapshot, action, bindings) {
                    Ok(state) => {
                        adopt(&window, &session, &form);
                        window.set_message(if state["write_committed"] == true && state["ready"] != true { "Shortcuts were written and activated, but saved storage could not reload. Review its error." } else if action == "reload" && state["stored"] != true { "Factory shortcuts activated; no saved file." } else if action == "reload" { "Saved shortcuts loaded and activated." } else { "Keyboard shortcuts saved and activated." }.into());
                    }
                    Err(error) => {
                        window.set_message(error.into());
                        refresh(&window, &session, &form);
                    }
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

pub fn dispatch_key(
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

pub fn refresh_hint(app: &AppWindow, session: &Session) {
    app.set_shortcuts_error(session.shortcuts_error_message().is_some());
    app.set_frame_shortcut(
        session
            .shortcut_bindings()
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
