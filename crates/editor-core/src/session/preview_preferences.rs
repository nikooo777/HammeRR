use std::path::PathBuf;

use super::{Actor, Session};
use crate::preview_preferences::{Preview, Store};
use crate::saved_defaults::{Binding, Snapshot};
use serde_json::{Value, json};

pub(super) type State = Binding<Store>;
const STALE: &str = "Preview settings or defaults changed; inspect Preview defaults again";

impl Session {
    pub(super) fn configure_preview_preferences(&mut self, directory: Result<PathBuf, String>) {
        let Some(result) = self.preview_preferences.configure(directory) else {
            return;
        };
        if let Err(error) = result {
            self.log(
                Actor::System,
                format!("Preview defaults could not load: {error}"),
            );
            return;
        }
        let value = self
            .preview_preferences
            .authority()
            .snapshot()
            .saved
            .unwrap_or_default();
        if let Err(error) = self.apply_preview_value(value) {
            self.preview_preferences.authority().fail(error.clone());
            self.log(
                Actor::System,
                format!("Preview defaults could not apply: {error}"),
            );
        }
    }

    fn preview_stamp(&self, saved: &Snapshot<Preview>) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            saved.serial,
            self.view.selection_outlines,
            self.view.selection_tint,
            self.view.entity_images,
            self.view.entity_model_guides,
            self.view.rope_preview,
            self.view.sky_preview
        )
    }

    pub fn preview_preferences_stamp(&self) -> String {
        self.preview_stamp(&self.preview_preferences.authority().snapshot())
    }

    pub(super) fn apply_preview_value(&mut self, preview: Preview) -> Result<(), String> {
        self.ensure_entity_placement_resolved()?;
        if self.history_navigation_blocked().is_some() || self.viewport_pick_conflict() {
            return Err("Finish or cancel the active preview, viewport tool, build or launch before applying preview defaults".into());
        }
        let view = self.resolve_view_update(preview.update())?;
        self.set_view(view);
        Ok(())
    }

    pub fn preview_preferences_error(&self) -> Option<String> {
        let saved = self.preview_preferences.authority().snapshot();
        saved.error.or(saved.storage_error)
    }

    pub fn preview_preferences_state(&self) -> Value {
        let saved = self.preview_preferences.authority().snapshot();
        let mut state = saved.report(Preview::from(self.view), self.preview_stamp(&saved));
        state.as_object_mut().unwrap().extend(json!({"limits":{"file_bytes":crate::preview_preferences::MAX_FILE_BYTES},
            "persistence":"Only Save current as defaults and Reset saved defaults write user preferences. Ordinary viewport toggles and temporary render_view overrides never save automatically. Only the six reported booleans are stored."}).as_object().unwrap().clone());
        state
    }

    pub fn change_preview_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        let saved = self.preview_preferences.authority().snapshot();
        if snapshot != self.preview_stamp(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .preview_preferences
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                let value = self
                    .preview_preferences
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.apply_preview_value(value)?;
                self.log(actor, "applied saved preview defaults");
            }
            "save" | "reset" => {
                let value = if action == "reset" {
                    Preview::default()
                } else {
                    Preview::from(self.view)
                };
                self.preview_preferences.authority().save(
                    saved.serial,
                    STALE,
                    value,
                    action == "reset",
                )?;
                write_committed = true;
                self.log(
                    actor,
                    if action == "reset" {
                        "reset saved preview defaults"
                    } else {
                        "saved current preview defaults"
                    },
                );
            }
            _ => return Err("Choose save, apply, reload or reset for preview defaults".into()),
        }
        let mut state = self.preview_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
