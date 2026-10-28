use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Actor, Session, selection_filters::SelectionFilters};
use crate::saved_defaults::{Binding, Snapshot};
use crate::selection_preferences::{Edit, SelectionDefaults, Store};

#[derive(Default)]
pub(super) struct State {
    pub binding: Binding<Store>,
    started: bool,
}

const STALE: &str = "Selection filters or defaults changed; inspect Selection defaults again";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProfileAction {
    Save { name: String },
    Replace { id: String },
    Apply { id: String },
    Rename { id: String, name: String },
    Delete { id: String },
}

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
                self.set_selection_filters(Actor::System, saved.saved.unwrap_or_default().filters)
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

    fn selection_preferences_stamp_for(&self, saved: &Snapshot<SelectionDefaults>) -> String {
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
        self.selection_preferences_report(&saved)
    }

    fn selection_preferences_report(&self, saved: &Snapshot<SelectionDefaults>) -> Value {
        let filters = saved.saved.as_ref().map(|settings| settings.filters);
        json!({
            "snapshot":self.selection_preferences_stamp_for(saved),
            "current":self.selection_filters(),
            "defaults":saved.loaded.then(||filters.unwrap_or_default()),
            "saved":filters,"stored":saved.loaded && saved.saved.is_some(),"ready":saved.loaded,
            "error":saved.error,"storage_error":saved.storage_error,"path":saved.path,
            "factory_defaults":SelectionFilters::default(),
            "limits":{"file_bytes":crate::selection_preferences::MAX_FILE_BYTES},
            "persistence":"Startup defaults and named profiles share one file. Startup applies only defaults once per Session. Saving, replacing, renaming, deleting, reloading or resetting never applies filters. Apply changes only this Session through the ordinary filter setter. New/Open retain current filters. Reset saves factory startup filters and preserves retained valid profiles; unreadable profiles with no valid retained copy cannot be recovered by Reset. Selection, visibility, map source and history are unchanged."
        })
    }

    pub fn selection_profiles_state(&self) -> Value {
        let saved = self.selection_preferences.binding.authority().snapshot();
        self.selection_profiles_report(&saved)
    }

    fn selection_profiles_report(&self, saved: &Snapshot<SelectionDefaults>) -> Value {
        let mut state = self.selection_preferences_report(saved);
        state["profiles"] = json!(
            saved
                .saved
                .as_ref()
                .map(|settings| settings.profiles.as_slice())
                .unwrap_or_default()
        );
        state["limits"] = json!({"file_bytes":crate::selection_preferences::MAX_FILE_BYTES,"profiles":crate::selection_preferences::MAX_PROFILES,"name_bytes":crate::selection_preferences::MAX_NAME_BYTES});
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
                    .checked_value(saved.serial, STALE)?
                    .filters;
                self.set_selection_filters(actor, filters)?;
                self.log(actor, "applied saved selection defaults");
            }
            "save" | "reset" => {
                let filters = if action == "reset" {
                    SelectionFilters::default()
                } else {
                    self.selection_filters()
                };
                self.selection_preferences.binding.authority().update(
                    saved.serial,
                    STALE,
                    action == "reset",
                    |saved| {
                        let mut settings = saved.cloned().unwrap_or_default();
                        settings.filters = filters;
                        Ok(settings)
                    },
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

    pub fn change_selection_profile(
        &mut self,
        actor: Actor,
        action: ProfileAction,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        let saved = self.selection_preferences.binding.authority().snapshot();
        if snapshot != self.selection_preferences_stamp_for(&saved) {
            return Err(STALE.into());
        }
        let filters = self.selection_filters();
        let (profile_id, write_committed, observed) = match action {
            ProfileAction::Apply { id } => {
                let settings = self
                    .selection_preferences
                    .binding
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                let profile = settings.profile(&id)?;
                self.set_selection_filters(actor, profile.filters)?;
                self.log(actor, format!("applied selection profile {}", profile.name));
                (Some(id), false, saved)
            }
            edit => {
                let mut profile_id = None;
                let observed = self.selection_preferences.binding.authority().update(
                    saved.serial,
                    STALE,
                    false,
                    |saved| {
                        let mut settings = saved.cloned().unwrap_or_default();
                        let replace_name = match &edit {
                            ProfileAction::Replace { id } => {
                                Some(settings.profile(id)?.name.clone())
                            }
                            _ => None,
                        };
                        let operation = match &edit {
                            ProfileAction::Save { name } => Edit::Save {
                                name,
                                replace: None,
                                filters,
                            },
                            ProfileAction::Replace { id } => Edit::Save {
                                name: replace_name.as_deref().unwrap(),
                                replace: Some(id),
                                filters,
                            },
                            ProfileAction::Rename { id, name } => Edit::Rename { id, name },
                            ProfileAction::Delete { id } => Edit::Delete { id },
                            ProfileAction::Apply { .. } => unreachable!(),
                        };
                        profile_id = Some(settings.edit(operation)?);
                        Ok(settings)
                    },
                )?;
                self.log(actor, "updated saved selection profiles");
                (profile_id, true, observed)
            }
        };
        let mut state = self.selection_profiles_report(&observed);
        state["write_committed"] = json!(write_committed);
        state["profile_id"] = json!(profile_id);
        Ok(state)
    }
}
