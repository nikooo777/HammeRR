use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session};
use crate::preview_preferences::{Preview, Source, Store};

pub(super) struct State {
    store: Result<Store, String>,
    saved: Option<Preview>,
    loaded: bool,
    error: Option<String>,
    source: Option<Source>,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: Err("Preview defaults storage is not configured".into()),
            saved: None,
            loaded: false,
            error: None,
            source: None,
            serial: 0,
        }
    }
}

impl Session {
    pub(super) fn configure_preview_preferences(&mut self, directory: Result<PathBuf, String>) {
        self.preview_preferences.store = directory.map(|directory| Store { directory });
        match self.read_preview_defaults() {
            Ok(()) => {
                if let Err(error) = self.apply_preview_defaults() {
                    self.preview_preferences.loaded = false;
                    let error = self.preview_defaults_error(error);
                    self.log(
                        Actor::System,
                        format!("Preview defaults could not apply: {error}"),
                    );
                }
            }
            Err(error) => {
                self.log(
                    Actor::System,
                    format!("Preview defaults could not load: {error}"),
                );
            }
        }
    }

    fn preview_defaults_error(&mut self, error: String) -> String {
        self.preview_preferences.error = Some(error.clone());
        self.preview_preferences.serial += 1;
        error
    }

    fn read_preview_defaults(&mut self) -> Result<(), String> {
        let result = self
            .preview_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| {
                let source = store.read_source()?;
                let result = store.parse_source(&source);
                self.preview_preferences.source = Some(source);
                result
            });
        match result {
            Ok(saved) => {
                self.preview_preferences.saved = saved;
                self.preview_preferences.loaded = true;
                self.preview_preferences.error = None;
                self.preview_preferences.serial += 1;
                Ok(())
            }
            Err(error) => {
                self.preview_preferences.loaded = false;
                Err(self.preview_defaults_error(error))
            }
        }
    }

    fn check_preview_defaults_source(&mut self) -> Result<(), String> {
        let current = self
            .preview_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::read_source);
        let error = match current {
            Ok(current) if self.preview_preferences.source.as_ref() == Some(&current) => {
                return Ok(());
            }
            Ok(current) => {
                self.preview_preferences.source = Some(current);
                "Saved preview defaults changed outside this session; Reload defaults before using them".into()
            }
            Err(error) => error,
        };
        self.preview_preferences.loaded = false;
        Err(self.preview_defaults_error(error))
    }

    fn apply_preview_defaults(&mut self) -> Result<(), String> {
        self.ensure_entity_placement_resolved()?;
        if self.history_navigation_blocked().is_some() || self.viewport_pick_conflict() {
            return Err("Finish or cancel the active preview, viewport tool, build or launch before applying preview defaults".into());
        }
        let preview = self.preview_preferences.saved.unwrap_or_default();
        let view = self.resolve_view_update(preview.update())?;
        self.set_view(view);
        Ok(())
    }

    pub fn preview_preferences_stamp(&self) -> String {
        let preview = Preview::from(self.view);
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.preview_preferences.serial,
            preview.selection_outlines,
            preview.entity_images,
            preview.entity_model_guides,
            preview.rope_preview,
            preview.sky_preview
        )
    }

    pub fn preview_preferences_error(&self) -> Option<&str> {
        self.preview_preferences.error.as_deref().or_else(|| {
            self.preview_preferences
                .store
                .as_ref()
                .err()
                .map(String::as_str)
        })
    }

    pub fn preview_preferences_state(&self) -> Value {
        json!({"snapshot":self.preview_preferences_stamp(),"current":Preview::from(self.view),
            "defaults":self.preview_preferences.loaded.then(||self.preview_preferences.saved.unwrap_or_default()),
            "saved":self.preview_preferences.saved,"stored":self.preview_preferences.loaded && self.preview_preferences.saved.is_some(),
            "ready":self.preview_preferences.loaded,"error":self.preview_preferences.error,
            "path":self.preview_preferences.store.as_ref().ok().map(Store::path),
            "storage_error":self.preview_preferences.store.as_ref().err(),
            "factory_defaults":Preview::default(),"limits":{"file_bytes":crate::preview_preferences::MAX_FILE_BYTES},
            "persistence":"Only Save current as defaults and Reset saved defaults write user preferences. Ordinary viewport toggles and temporary render_view overrides never save automatically. Only the five reported booleans are stored."})
    }

    pub fn change_preview_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.preview_preferences_stamp() {
            return Err(
                "Preview settings or defaults changed; inspect Preview defaults again".into(),
            );
        }
        let mut write_committed = false;
        match action {
            "reload" => self.read_preview_defaults()?,
            "apply" => {
                if !self.preview_preferences.loaded {
                    return Err("Preview defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_preview_defaults_source()?;
                self.apply_preview_defaults()?;
                self.log(actor, "applied saved preview defaults");
            }
            "save" | "reset" => {
                if action == "save" && !self.preview_preferences.loaded {
                    return Err("Preview defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_preview_defaults_source()?;
                let preview = if action == "reset" {
                    Preview::default()
                } else {
                    Preview::from(self.view)
                };
                let result = self
                    .preview_preferences
                    .store
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|store| store.save(preview, action == "reset"));
                if let Err(error) = result {
                    self.preview_preferences.loaded = false;
                    return Err(self.preview_defaults_error(error));
                }
                write_committed = true;
                self.preview_preferences.saved = Some(preview);
                if let Err(error) = self.read_preview_defaults() {
                    self.log(
                        actor,
                        format!("Preview defaults were written but could not reload: {error}"),
                    );
                }
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
