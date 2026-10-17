use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session};
use crate::navigation_preferences::{Navigation, Source, Store};

pub(super) struct State {
    current: Navigation,
    store: Result<Store, String>,
    saved: Option<Navigation>,
    loaded: bool,
    error: Option<String>,
    source: Option<Source>,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            current: Navigation::default(),
            store: Err("Navigation defaults storage is not configured".into()),
            saved: None,
            loaded: false,
            error: None,
            source: None,
            serial: 0,
        }
    }
}

impl Session {
    pub(super) fn configure_navigation_preferences(&mut self, directory: Result<PathBuf, String>) {
        self.navigation_preferences.store = directory.map(|directory| Store { directory });
        match self.read_navigation_defaults() {
            Ok(()) => {
                if let Err(error) = self.apply_navigation_defaults() {
                    self.navigation_preferences.loaded = false;
                    let error = self.navigation_defaults_error(error);
                    self.log(
                        Actor::System,
                        format!("Navigation defaults could not apply: {error}"),
                    );
                }
            }
            Err(error) => {
                self.log(
                    Actor::System,
                    format!("Navigation defaults could not load: {error}"),
                );
            }
        }
    }

    fn navigation_defaults_error(&mut self, error: String) -> String {
        self.navigation_preferences.error = Some(error.clone());
        self.navigation_preferences.serial += 1;
        error
    }

    fn read_navigation_defaults(&mut self) -> Result<(), String> {
        let result = self
            .navigation_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| {
                let source = store.read_source()?;
                let result = store.parse_source(&source);
                self.navigation_preferences.source = Some(source);
                result
            });
        match result {
            Ok(saved) => {
                self.navigation_preferences.saved = saved;
                self.navigation_preferences.loaded = true;
                self.navigation_preferences.error = None;
                self.navigation_preferences.serial += 1;
                Ok(())
            }
            Err(error) => {
                self.navigation_preferences.loaded = false;
                Err(self.navigation_defaults_error(error))
            }
        }
    }

    fn accept_navigation_publication(
        &mut self,
        navigation: Navigation,
        publication: crate::preference_file::Publication,
    ) {
        self.navigation_preferences.saved = Some(navigation);
        match publication.source {
            Ok(source) => {
                self.navigation_preferences.source = Some(source);
                self.navigation_preferences.loaded = true;
                self.navigation_preferences.error = None;
                self.navigation_preferences.serial += 1;
            }
            Err(error) => {
                self.navigation_preferences.loaded = false;
                self.navigation_defaults_error(error);
            }
        }
    }

    fn check_navigation_defaults_source(&mut self) -> Result<(), String> {
        let current = self
            .navigation_preferences
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| match self.navigation_preferences.source.as_ref() {
                Some(expected) => store.read_current(expected),
                None => store.read_source(),
            });
        let error = match current {
            Ok(current)
                if self
                    .navigation_preferences
                    .source
                    .as_ref()
                    .is_some_and(|expected| expected.source == current.source) =>
            {
                return Ok(());
            }
            Ok(current) => {
                self.navigation_preferences.source = Some(current);
                "Saved navigation defaults changed outside this session; Reload defaults before using them".into()
            }
            Err(error) => error,
        };
        self.navigation_preferences.loaded = false;
        Err(self.navigation_defaults_error(error))
    }

    fn apply_navigation_defaults(&mut self) -> Result<(), String> {
        self.set_fly_speed(
            self.navigation_preferences
                .saved
                .unwrap_or_default()
                .fly_speed,
        )
    }

    pub fn fly_speed(&self) -> f32 {
        self.navigation_preferences.current.fly_speed
    }

    pub fn set_fly_speed(&mut self, fly_speed: f32) -> Result<(), String> {
        let navigation = Navigation { fly_speed };
        navigation.validate()?;
        if self.navigation_preferences.current != navigation {
            self.navigation_preferences.current = navigation;
            self.navigation_preferences.serial += 1;
        }
        Ok(())
    }

    pub fn adjust_fly_speed(&mut self, wheel_delta: f32) -> Result<(), String> {
        if !wheel_delta.is_finite() {
            return Err("Fly speed wheel delta must be finite".into());
        }
        let speed = (self.fly_speed() * 2f32.powf(wheel_delta / 240.0)).clamp(
            crate::navigation_preferences::MIN_FLY_SPEED,
            crate::navigation_preferences::MAX_FLY_SPEED,
        );
        self.set_fly_speed(speed)
    }

    pub fn navigation_preferences_stamp(&self) -> String {
        format!(
            "{}:{}",
            self.navigation_preferences.serial,
            self.fly_speed().to_bits()
        )
    }

    pub fn navigation_preferences_serial(&self) -> u64 {
        self.navigation_preferences.serial
    }

    pub fn navigation_preferences_error(&self) -> Option<&str> {
        self.navigation_preferences.error.as_deref().or_else(|| {
            self.navigation_preferences
                .store
                .as_ref()
                .err()
                .map(String::as_str)
        })
    }

    pub fn navigation_preferences_state(&self) -> Value {
        json!({"snapshot":self.navigation_preferences_stamp(),"current":self.navigation_preferences.current,
            "defaults":self.navigation_preferences.loaded.then(||self.navigation_preferences.saved.unwrap_or_default()),
            "saved":self.navigation_preferences.saved,"stored":self.navigation_preferences.loaded && self.navigation_preferences.saved.is_some(),
            "ready":self.navigation_preferences.loaded,"error":self.navigation_preferences.error,
            "path":self.navigation_preferences.store.as_ref().ok().map(Store::path),
            "storage_error":self.navigation_preferences.store.as_ref().err(),
            "factory_defaults":Navigation::default(),"limits":{"file_bytes":crate::navigation_preferences::MAX_FILE_BYTES,"min_fly_speed":crate::navigation_preferences::MIN_FLY_SPEED,"max_fly_speed":crate::navigation_preferences::MAX_FLY_SPEED},"units":"Hammer units per second", "shift_multiplier":crate::navigation_preferences::FAST_MULTIPLIER, "right_button_wheel":"Perspective right-button wheel multiplies speed by 2^(delta/240), clamped to the reported range; planar wheel zooms and ordinary perspective wheel dollies",
            "persistence":"Only Save current as defaults and Reset saved defaults write navigation preferences. Fly speed edits and right-button wheel changes never save automatically. Only fly_speed is stored; cameras, positions, sensitivity and active tools are unchanged."})
    }

    pub fn change_navigation_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.navigation_preferences_stamp() {
            return Err(
                "Navigation settings or defaults changed; inspect Navigation defaults again".into(),
            );
        }
        let mut write_committed = false;
        match action {
            "reload" => self.read_navigation_defaults()?,
            "apply" => {
                if !self.navigation_preferences.loaded {
                    return Err("Navigation defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_navigation_defaults_source()?;
                self.apply_navigation_defaults()?;
                self.log(actor, "applied saved navigation defaults");
            }
            "save" | "reset" => {
                if action == "save" && !self.navigation_preferences.loaded {
                    return Err("Navigation defaults did not load; Reload defaults or explicitly Reset saved defaults first".into());
                }
                self.check_navigation_defaults_source()?;
                let navigation = if action == "reset" {
                    Navigation::default()
                } else {
                    self.navigation_preferences.current
                };
                let result = self
                    .navigation_preferences
                    .store
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|store| {
                        let expected = self
                            .navigation_preferences
                            .source
                            .as_ref()
                            .ok_or("Inspect defaults before writing")?;
                        store
                            .save_checked(navigation, action == "reset", expected)
                            .map_err(|error| error.message)
                    });
                let publication = match result {
                    Ok(publication) => publication,
                    Err(error) => {
                        self.navigation_preferences.loaded = false;
                        return Err(self.navigation_defaults_error(error));
                    }
                };
                write_committed = true;
                self.accept_navigation_publication(navigation, publication);
                self.log(
                    actor,
                    if action == "reset" {
                        "reset saved navigation defaults"
                    } else {
                        "saved current navigation defaults"
                    },
                );
            }
            _ => return Err("Choose save, apply, reload or reset for navigation defaults".into()),
        }
        let mut state = self.navigation_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
