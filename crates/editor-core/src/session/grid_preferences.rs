use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session};
use crate::grid_preferences::{Grid, Source, Store};

pub(super) struct State {
    store: Result<Store, String>,
    saved: Option<Grid>,
    loaded: bool,
    error: Option<String>,
    source: Option<Source>,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: Err("Grid defaults storage is not configured".into()),
            saved: None,
            loaded: false,
            error: None,
            source: None,
            serial: 0,
        }
    }
}

impl Session {
    pub(super) fn configure_grid_preferences(&mut self, directory: Result<PathBuf, String>) {
        self.grid_preferences.store = directory.map(|directory| Store { directory });
        match self.read_grid_defaults() {
            Ok(()) => {
                let grid = self.grid_preferences.saved.unwrap_or_default();
                if let Err(error) = self.set_spatial(Actor::System, grid.update()) {
                    self.grid_preferences.loaded = false;
                    let error = self.grid_defaults_error(error);
                    self.log(
                        Actor::System,
                        format!("Grid defaults could not apply: {error}"),
                    );
                }
            }
            Err(error) => {
                self.log(
                    Actor::System,
                    format!("Grid defaults could not load: {error}"),
                );
            }
        }
    }

    fn grid_defaults_error(&mut self, error: String) -> String {
        self.grid_preferences.error = Some(error.clone());
        self.grid_preferences.serial += 1;
        self.view_serial += 1;
        error
    }

    fn read_grid_defaults(&mut self) -> Result<(), String> {
        let result = self
            .grid_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| {
                let source = store.read_source()?;
                let result = store.parse_source(&source);
                self.grid_preferences.source = Some(source);
                result
            });
        match result {
            Ok(saved) => {
                self.grid_preferences.saved = saved;
                self.grid_preferences.loaded = true;
                self.grid_preferences.error = None;
                self.grid_preferences.serial += 1;
                self.view_serial += 1;
                Ok(())
            }
            Err(error) => {
                self.grid_preferences.loaded = false;
                Err(self.grid_defaults_error(error))
            }
        }
    }

    fn check_grid_defaults_source(&mut self) -> Result<(), String> {
        let current = self
            .grid_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::read_source);
        let error = match current {
            Ok(current) if self.grid_preferences.source.as_ref() == Some(&current) => return Ok(()),
            Ok(current) => {
                self.grid_preferences.source = Some(current);
                "Saved grid defaults changed outside this session; Reload defaults before using them".into()
            }
            Err(error) => error,
        };
        self.grid_preferences.loaded = false;
        Err(self.grid_defaults_error(error))
    }

    pub fn grid_preferences_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.grid_preferences.serial,
            self.spatial.grid_size,
            self.spatial.snap,
            self.spatial.grid_visible
        )
    }

    pub fn grid_preferences_state(&self) -> Value {
        json!({"snapshot":self.grid_preferences_stamp(),"current":Grid::from(self.spatial),
            "defaults":self.grid_preferences.loaded.then(||self.grid_preferences.saved.unwrap_or_default()),
            "saved":self.grid_preferences.saved,"stored":self.grid_preferences.loaded && self.grid_preferences.saved.is_some(),
            "ready":self.grid_preferences.loaded,"error":self.grid_preferences.error,
            "path":self.grid_preferences.store.as_ref().ok().map(Store::path),
            "storage_error":self.grid_preferences.store.as_ref().err(),
            "factory_defaults":Grid::default(),"limits":{"grid_min":crate::spatial::MIN_GRID,"grid_max":crate::spatial::MAX_GRID,"file_bytes":crate::grid_preferences::MAX_FILE_BYTES},
            "persistence":"Only Save current as defaults and Reset saved defaults write user preferences. Temporary grid changes, workplanes and previews are never saved automatically."})
    }

    pub fn change_grid_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.grid_preferences_stamp() {
            return Err("Grid or defaults changed; inspect Grid defaults again".into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self.read_grid_defaults()?,
            "apply" => {
                if !self.grid_preferences.loaded {
                    return Err("Grid defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_grid_defaults_source()?;
                self.ensure_entity_placement_resolved()?;
                if self.history_navigation_blocked().is_some() {
                    return Err("Finish or cancel the active preview, build or launch before applying grid defaults".into());
                }
                let grid = self.grid_preferences.saved.unwrap_or_default();
                self.set_spatial(actor, grid.update())?;
            }
            "save" | "reset" => {
                if action == "save" && !self.grid_preferences.loaded {
                    return Err("Grid defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_grid_defaults_source()?;
                let grid = if action == "reset" {
                    Grid::default()
                } else {
                    Grid::from(self.spatial)
                };
                let result = self
                    .grid_preferences
                    .store
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|store| store.save(grid, action == "reset"));
                if let Err(error) = result {
                    self.grid_preferences.loaded = false;
                    return Err(self.grid_defaults_error(error));
                }
                write_committed = true;
                self.grid_preferences.saved = Some(grid);
                if let Err(error) = self.read_grid_defaults() {
                    self.log(
                        actor,
                        format!("Grid defaults were written but could not reload: {error}"),
                    );
                }
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

#[cfg(test)]
mod tests;
