use std::path::PathBuf;

use super::{Actor, Session};
use crate::grid_preferences::{Grid, Store};
use crate::saved_defaults::{Binding, Snapshot};
use serde_json::{Value, json};

pub(super) type State = Binding<Store>;
const STALE: &str = "Grid or defaults changed; inspect Grid defaults again";

impl Session {
    pub(super) fn configure_grid_preferences(&mut self, directory: Result<PathBuf, String>) {
        let Some(result) = self.grid_preferences.configure(directory) else {
            return;
        };
        if let Err(error) = result {
            self.log(
                Actor::System,
                format!("Grid defaults could not load: {error}"),
            );
            return;
        }
        let value = self
            .grid_preferences
            .authority()
            .snapshot()
            .saved
            .unwrap_or_default();
        if let Err(error) = self.set_spatial(Actor::System, value.update()) {
            self.grid_preferences.authority().fail(error.clone());
            self.log(
                Actor::System,
                format!("Grid defaults could not apply: {error}"),
            );
        }
    }

    fn grid_stamp(&self, saved: &Snapshot<Grid>) -> String {
        format!(
            "{}:{}:{}:{}",
            saved.serial, self.spatial.grid_size, self.spatial.snap, self.spatial.grid_visible
        )
    }

    pub fn grid_preferences_stamp(&self) -> String {
        self.grid_stamp(&self.grid_preferences.authority().snapshot())
    }

    pub fn grid_preferences_state(&self) -> Value {
        let saved = self.grid_preferences.authority().snapshot();
        let mut state = saved.report(Grid::from(self.spatial), self.grid_stamp(&saved));
        state.as_object_mut().unwrap().extend(json!({"limits":{"grid_min":crate::spatial::MIN_GRID,"grid_max":crate::spatial::MAX_GRID,"file_bytes":crate::grid_preferences::MAX_FILE_BYTES},
            "persistence":"Only Save current as defaults and Reset saved defaults write user preferences. Temporary grid changes, workplanes and previews are never saved automatically."}).as_object().unwrap().clone());
        state
    }

    pub fn change_grid_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        let saved = self.grid_preferences.authority().snapshot();
        if snapshot != self.grid_stamp(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .grid_preferences
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                let value = self
                    .grid_preferences
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.ensure_entity_placement_resolved()?;
                if self.history_navigation_blocked().is_some() {
                    return Err("Finish or cancel the active preview, build or launch before applying grid defaults".into());
                }
                self.set_spatial(actor, value.update())?;
            }
            "save" | "reset" => {
                let value = if action == "reset" {
                    Grid::default()
                } else {
                    Grid::from(self.spatial)
                };
                self.grid_preferences.authority().save(
                    saved.serial,
                    STALE,
                    value,
                    action == "reset",
                )?;
                write_committed = true;
                self.log(
                    actor,
                    if action == "reset" {
                        "reset saved grid defaults"
                    } else {
                        "saved current grid defaults"
                    },
                );
            }
            _ => return Err("Choose save, apply, reload or reset for grid defaults".into()),
        }
        let mut state = self.grid_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}
