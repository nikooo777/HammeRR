use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session, selection_filters::SelectionFilters};
use crate::saved_defaults::{Binding, Snapshot};
use crate::selection_preferences::Store;

#[derive(Default)]
pub(super) struct State {
    pub binding: Binding<Store>,
    started: bool,
}

const STALE: &str = "Selection filters or defaults changed; inspect Selection defaults again";

impl Session {
    pub(super) fn configure_selection_preferences(&mut self, directory: Result<PathBuf, String>) {
        if self.selection_preferences.started {
            return;
        }
        let _ = self.selection_preferences.binding.configure(directory);
        self.apply_startup_selection_preferences();
    }

    pub(super) fn apply_startup_selection_preferences(&mut self) {
        if self.selection_preferences.started {
            return;
        }
        self.selection_preferences.started = true;
        let saved = self.selection_preferences.binding.authority().snapshot();
        if saved.loaded {
            if let Err(error) =
                self.set_selection_filters(Actor::System, saved.saved.unwrap_or_default())
            {
                self.log(
                    Actor::System,
                    format!("Selection defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = saved.error.or(saved.storage_error) {
            self.log(
                Actor::System,
                format!("Selection defaults could not load: {error}"),
            );
        }
    }

    fn selection_preferences_stamp_for(&self, saved: &Snapshot<SelectionFilters>) -> String {
        format!("{}:{}", saved.serial, self.selection_filter_revision())
    }

    pub fn selection_preferences_stamp(&self) -> String {
        self.selection_preferences_stamp_for(
            &self.selection_preferences.binding.authority().snapshot(),
        )
    }

    pub fn selection_preferences_error(&self) -> Option<String> {
        let saved = self.selection_preferences.binding.authority().snapshot();
        saved.error.or(saved.storage_error)
    }

    pub fn selection_preferences_state(&self) -> Value {
        let saved = self.selection_preferences.binding.authority().snapshot();
        let mut state = saved.report(
            self.selection_filters(),
            self.selection_preferences_stamp_for(&saved),
        );
        state["limits"] = json!({"file_bytes":crate::selection_preferences::MAX_FILE_BYTES});
        state["persistence"] = json!(
            "Only Save current as defaults and Reset saved defaults write selection preferences. Startup applies defaults once per Session; Apply saved defaults changes only this Session through the ordinary filter setter. Reload and saving defaults never change current filters. New/Open retain current filters. Selection, visibility, map source and history are unchanged."
        );
        state
    }

    pub fn change_selection_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        let saved = self.selection_preferences.binding.authority().snapshot();
        if snapshot != self.selection_preferences_stamp_for(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .selection_preferences
                .binding
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                let filters = self
                    .selection_preferences
                    .binding
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.set_selection_filters(actor, filters)?;
                self.log(actor, "applied saved selection defaults");
            }
            "save" | "reset" => {
                let filters = if action == "reset" {
                    SelectionFilters::default()
                } else {
                    self.selection_filters()
                };
                self.selection_preferences.binding.authority().save(
                    saved.serial,
                    STALE,
                    filters,
                    action == "reset",
                )?;
                write_committed = true;
                self.log(
                    actor,
                    if action == "reset" {
                        "reset saved selection defaults"
                    } else {
                        "saved current selection defaults"
                    },
                );
            }
            _ => return Err("Choose save, apply, reload or reset for selection defaults".into()),
        }
        let mut state = self.selection_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
