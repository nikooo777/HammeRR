use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session, TransformTool};
use crate::pivot_preferences::{Pivot, Store};
use crate::saved_defaults::{Binding, Snapshot};

#[derive(Default)]
pub(super) struct State {
    pub binding: Binding<Store>,
    started: bool,
    pub local_serial: u64,
}

const STALE: &str = "Transform pivot or defaults changed; inspect Transform pivot defaults again";

impl Session {
    pub(super) fn configure_pivot_preferences(&mut self, directory: Result<PathBuf, String>) {
        if self.pivot_preferences.started {
            return;
        }
        let _ = self.pivot_preferences.binding.configure(directory);
        self.apply_startup_pivot_preferences();
    }

    pub(super) fn apply_startup_pivot_preferences(&mut self) {
        if self.pivot_preferences.started {
            return;
        }
        self.pivot_preferences.started = true;
        let saved = self.pivot_preferences.binding.authority().snapshot();
        if saved.loaded {
            if let Err(error) =
                self.apply_pivot_value(Actor::System, saved.saved.unwrap_or_default())
            {
                self.log(
                    Actor::System,
                    format!("Transform pivot defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = saved.error.or(saved.storage_error) {
            self.log(
                Actor::System,
                format!("Transform pivot defaults could not load: {error}"),
            );
        }
    }

    fn pivot_preferences_stamp_for(&self, saved: &Snapshot<Pivot>) -> String {
        format!("{}:{}", saved.serial, self.pivot_preferences.local_serial)
    }

    pub fn pivot_preferences_stamp(&self) -> String {
        self.pivot_preferences_stamp_for(&self.pivot_preferences.binding.authority().snapshot())
    }

    pub fn pivot_preferences_error(&self) -> Option<String> {
        let saved = self.pivot_preferences.binding.authority().snapshot();
        saved.error.or(saved.storage_error)
    }

    fn apply_pivot_value(&mut self, actor: Actor, value: Pivot) -> Result<(), String> {
        value.validate()?;
        self.set_transform_tool(
            actor,
            TransformTool {
                pivot: value.point(),
                ..self.transform_tool
            },
        )
    }

    fn pivot_defaults_apply_admission(&self) -> Result<(), String> {
        if self.history_navigation_blocked().is_some()
            || self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.paint_enabled()
            || !self.component_selection().is_empty()
        {
            return Err("Finish or cancel the active tool, preview, build or launch before applying transform pivot defaults".into());
        }
        Ok(())
    }

    pub fn pivot_preferences_state(&self) -> Value {
        let saved = self.pivot_preferences.binding.authority().snapshot();
        let mut state = saved.report(
            Pivot::from_point(self.transform_tool.pivot),
            self.pivot_preferences_stamp_for(&saved),
        );
        state.as_object_mut().unwrap().extend(json!({
            "limits": {"file_bytes": crate::pivot_preferences::MAX_FILE_BYTES, "finite_coordinates": true},
            "apply_error": self.pivot_defaults_apply_admission().err(),
            "persistence": "Only Save current as defaults and Reset saved defaults write preferences. Saved pivot defaults apply once when a document Session starts. Apply changes only this document's pivot and preserves other transform settings. New/Open retain its current pivot. Picks save only fixed world coordinates when explicitly requested; no surface attachment, selection, pick provenance, camera or map history is stored."
        }).as_object().unwrap().clone());
        state
    }

    pub fn change_pivot_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let saved = self.pivot_preferences.binding.authority().snapshot();
        if snapshot != self.pivot_preferences_stamp_for(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .pivot_preferences
                .binding
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                self.ensure_entity_placement_resolved()?;
                self.pivot_defaults_apply_admission()?;
                let value = self
                    .pivot_preferences
                    .binding
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.apply_pivot_value(actor, value)?;
                self.log(actor, "applied saved transform pivot defaults");
            }
            "save" | "reset" => {
                let value = if action == "reset" {
                    Pivot::default()
                } else {
                    Pivot::from_point(self.transform_tool.pivot)
                };
                self.pivot_preferences.binding.authority().save(
                    saved.serial,
                    STALE,
                    value,
                    action == "reset",
                )?;
                write_committed = true;
                self.log(
                    actor,
                    if action == "reset" {
                        "reset saved transform pivot defaults"
                    } else {
                        "saved current transform pivot defaults"
                    },
                );
            }
            _ => {
                return Err(
                    "Choose save, apply, reload or reset for transform pivot defaults".into(),
                );
            }
        }
        let mut state = self.pivot_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
