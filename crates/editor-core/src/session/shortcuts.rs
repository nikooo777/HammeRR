use std::path::PathBuf;

use glam::DVec3;
use map_core::{ObjectId, edit};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};
use crate::{
    identity::id_string,
    preference_file::ObservedSource,
    shortcuts::{Bindings, Chord, Command, Store},
};

pub(super) struct State {
    store: Result<Store, String>,
    current: Bindings,
    source: Option<ObservedSource>,
    error: Option<String>,
    loaded: bool,
    stored: bool,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: Err("Keyboard shortcut storage is not configured".into()),
            current: Bindings::default(),
            source: None,
            error: None,
            loaded: false,
            stored: false,
            serial: 0,
        }
    }
}

impl Session {
    pub(super) fn configure_shortcuts(&mut self, directory: Result<PathBuf, String>) {
        self.shortcuts.store = directory.map(|directory| Store { directory });
        if let Err(error) = self.reload_shortcuts() {
            self.log(
                Actor::System,
                format!("Saved keyboard shortcuts unavailable: {error}"),
            );
        }
    }

    fn shortcuts_error(&mut self, error: String) -> String {
        self.shortcuts.loaded = false;
        self.shortcuts.error = Some(error.clone());
        self.shortcuts.serial += 1;
        error
    }

    fn reload_shortcuts(&mut self) -> Result<(), String> {
        let read = self
            .shortcuts
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::read_source);
        let result = match read {
            Ok(source) => {
                let result = self.shortcuts.store.as_ref().unwrap().parse(&source);
                self.shortcuts.source = Some(source);
                result
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(saved) => {
                self.shortcuts.stored = saved.is_some();
                self.shortcuts.current = saved.unwrap_or_default();
                self.shortcuts.loaded = true;
                self.shortcuts.error = None;
                self.shortcuts.serial += 1;
                Ok(())
            }
            Err(error) => Err(self.shortcuts_error(error)),
        }
    }

    fn accept_shortcuts_publication(
        &mut self,
        current: Bindings,
        publication: crate::preference_file::Publication,
    ) {
        self.shortcuts.current = current;
        self.shortcuts.stored = true;
        match publication.source {
            Ok(source) => {
                self.shortcuts.source = Some(source);
                self.shortcuts.loaded = true;
                self.shortcuts.error = None;
                self.shortcuts.serial += 1;
            }
            Err(error) => {
                self.shortcuts_error(error);
            }
        }
    }

    fn check_shortcuts_source(&mut self) -> Result<(), String> {
        let read = self
            .shortcuts
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| match self.shortcuts.source.as_ref() {
                Some(expected) => store.read_current(expected),
                None => store.read_source(),
            });
        let error = match read {
            Ok(source)
                if self
                    .shortcuts
                    .source
                    .as_ref()
                    .is_some_and(|expected| expected.source == source.source) =>
            {
                return Ok(());
            }
            Ok(source) => {
                self.shortcuts.source = Some(source);
                "Saved shortcuts changed outside this session; Reload saved or explicitly Reset saved shortcuts".into()
            }
            Err(error) => error,
        };
        Err(self.shortcuts_error(error))
    }

    pub fn shortcuts_stamp(&self) -> String {
        self.shortcuts.serial.to_string()
    }

    pub fn shortcuts_error_message(&self) -> Option<&str> {
        self.shortcuts
            .error
            .as_deref()
            .or_else(|| self.shortcuts.store.as_ref().err().map(String::as_str))
    }

    pub fn shortcut_bindings(&self) -> &Bindings {
        &self.shortcuts.current
    }

    pub fn resolve_editor_shortcut(
        &self,
        key: &str,
        control: bool,
        shift: bool,
        alt: bool,
        meta: bool,
    ) -> Option<Command> {
        if alt || meta {
            return None;
        }
        let key = crate::shortcuts::canonical_key(key)?;
        self.shortcuts.current.resolve(&Chord {
            key,
            control,
            shift,
        })
    }

    pub fn keyboard_shortcuts_state(&self) -> Value {
        let factory = Bindings::default();
        let rows = Command::ALL.map(|command| json!({"command":command,"label":command.label(),"bindings":self.shortcuts.current.slots(command),"display":self.shortcuts.current.label(command),"factory_display":factory.label(command)}));
        json!({"snapshot":self.shortcuts_stamp(),"current":self.shortcuts.current,"factory_defaults":factory,
            "ready":self.shortcuts.loaded,"stored":self.shortcuts.loaded && self.shortcuts.stored,
            "error":self.shortcuts_error_message(),"path":self.shortcuts.store.as_ref().ok().map(Store::path),"commands":rows,
            "limits":{"slots_per_command":2,"file_bytes":crate::shortcuts::MAX_FILE_BYTES},
            "scope":"Viewport focus only. Logical A–Z, F1–F12, Delete and Backspace with exact Ctrl/Shift modifiers; Alt/Meta are unavailable. Escape, arrows/PageUp/PageDown, camera W/A/S/D/Q/E (including Shift) and Ctrl+N/O/S/C/X/V are reserved. Text fields keep native editing shortcuts. Save and Reset persist and activate; failed Reload preserves current bindings. No map or Undo changes."})
    }

    pub fn change_keyboard_shortcuts(
        &mut self,
        actor: Actor,
        snapshot: &str,
        action: &str,
        bindings: Option<Bindings>,
    ) -> Result<Value, String> {
        if snapshot != self.shortcuts_stamp() {
            return Err("Keyboard shortcuts changed; Reload saved before saving this form".into());
        }
        let mut write_committed = false;
        match action {
            "reload" if bindings.is_none() => self.reload_shortcuts()?,
            "save" | "reset" => {
                let current = match (action, bindings) {
                    ("save", Some(bindings)) => bindings.validated()?,
                    ("reset", None) => Bindings::default(),
                    _ => {
                        return Err(
                            "Save requires all five bindings; Reset does not accept bindings"
                                .into(),
                        );
                    }
                };
                if action == "save" && !self.shortcuts.loaded {
                    return Err("Saved shortcuts unavailable; Reload saved must succeed or explicitly Reset saved shortcuts first".into());
                }
                self.check_shortcuts_source()?;
                let result = self
                    .shortcuts
                    .store
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|store| {
                        let expected = self
                            .shortcuts
                            .source
                            .as_ref()
                            .ok_or("Inspect saved shortcuts before writing")?;
                        store
                            .save_checked(current.clone(), expected)
                            .map_err(|error| error.message)
                    });
                let publication = match result {
                    Ok(publication) => publication,
                    Err(error) => return Err(self.shortcuts_error(error)),
                };
                write_committed = true;
                self.accept_shortcuts_publication(current, publication);
            }
            _ => return Err("Choose save, reload or reset for keyboard shortcuts".into()),
        }
        self.log(
            actor,
            match action {
                "save" => "Saved keyboard shortcuts",
                "reset" => "Reset saved keyboard shortcuts",
                _ => "Reloaded saved keyboard shortcuts",
            },
        );
        let mut state = self.keyboard_shortcuts_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }

    pub fn delete_objects(&mut self, actor: Actor, ids: &[ObjectId]) -> Result<(), String> {
        self.edit(actor, &format!("delete {} objects", ids.len()), |tx| {
            edit::delete(tx, ids)
        })
    }

    pub fn duplicate_objects(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        offset: DVec3,
    ) -> Result<Vec<ObjectId>, String> {
        let copies = self.edit(actor, &format!("duplicate {} objects", ids.len()), |tx| {
            edit::duplicate(tx, ids, offset)
        })?;
        self.select(&copies, SelectMode::Replace)?;
        Ok(copies)
    }

    pub fn execute_editor_command(
        &mut self,
        actor: Actor,
        command: Command,
        aspect: f32,
    ) -> Result<Value, String> {
        let object_command = matches!(
            command,
            Command::DeleteSelection | Command::DuplicateSelection
        );
        if object_command
            && (self.component_picking().is_some() || !self.component_selection().is_empty())
        {
            return Err("Use Reshape brushes or its move handles. Finish component editing before editing whole objects.".into());
        }
        let mut result = json!({"command":command});
        match command {
            Command::Undo => result["label"] = json!(self.undo(actor)),
            Command::Redo => result["label"] = json!(self.redo(actor)),
            Command::FrameSelection => {
                self.frame_selection(actor, aspect)?;
                result["camera_changed"] = json!(true);
            }
            Command::DeleteSelection | Command::DuplicateSelection => {
                let ids = self.selected_ids();
                if ids.is_empty() {
                    return Err("Nothing selected".into());
                }
                if command == Command::DeleteSelection {
                    self.delete_objects(actor, &ids)?;
                    result["deleted"] = json!(ids.into_iter().map(id_string).collect::<Vec<_>>());
                } else {
                    let grid = self.spatial.grid_size;
                    let copies =
                        self.duplicate_objects(actor, &ids, DVec3::new(grid, grid, 0.0))?;
                    result["copies"] = json!(copies.into_iter().map(id_string).collect::<Vec<_>>());
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
