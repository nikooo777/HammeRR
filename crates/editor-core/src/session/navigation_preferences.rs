use std::path::PathBuf;

use super::{Actor, Session};
use crate::navigation_preferences::{Navigation, Store};
use crate::saved_defaults::{Binding, Snapshot};
use serde_json::{Value, json};

pub(super) type State = Binding<Store>;
const STALE: &str = "Navigation settings or defaults changed; inspect Navigation defaults again";

impl Session {
    pub(super) fn configure_navigation_preferences(&mut self, directory: Result<PathBuf, String>) {
        let Some(result) = self.navigation_preferences.configure(directory) else {
            return;
        };
        if let Err(error) = result {
            self.log(
                Actor::System,
                format!("Navigation defaults could not load: {error}"),
            );
            return;
        }
        let value = self
            .navigation_preferences
            .authority()
            .snapshot()
            .saved
            .unwrap_or_default();
        if let Err(error) = self.set_fly_speed(value.fly_speed) {
            self.navigation_preferences.authority().fail(error.clone());
            self.log(
                Actor::System,
                format!("Navigation defaults could not apply: {error}"),
            );
        }
    }

    fn navigation_stamp(&self, saved: &Snapshot<Navigation>) -> String {
        format!(
            "{}:{}:{}",
            saved.serial,
            self.navigation_local_serial,
            self.fly_speed().to_bits()
        )
    }

    pub fn navigation_preferences_stamp(&self) -> String {
        self.navigation_stamp(&self.navigation_preferences.authority().snapshot())
    }

    pub fn fly_speed(&self) -> f32 {
        self.navigation_current.fly_speed
    }

    pub fn set_fly_speed(&mut self, fly_speed: f32) -> Result<(), String> {
        let navigation = Navigation { fly_speed };
        navigation.validate()?;
        if self.navigation_current != navigation {
            self.navigation_current = navigation;
            self.navigation_local_serial += 1;
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

    pub fn navigation_preferences_serial(&self) -> (u64, u64) {
        (
            self.navigation_preferences.authority().snapshot().serial,
            self.navigation_local_serial,
        )
    }
    pub fn navigation_preferences_error(&self) -> Option<String> {
        let saved = self.navigation_preferences.authority().snapshot();
        saved.error.or(saved.storage_error)
    }

    pub fn navigation_preferences_state(&self) -> Value {
        let saved = self.navigation_preferences.authority().snapshot();
        let mut state = saved.report(self.navigation_current, self.navigation_stamp(&saved));
        state.as_object_mut().unwrap().extend(json!({"limits":{"file_bytes":crate::navigation_preferences::MAX_FILE_BYTES,"min_fly_speed":crate::navigation_preferences::MIN_FLY_SPEED,"max_fly_speed":crate::navigation_preferences::MAX_FLY_SPEED},"units":"Hammer units per second", "shift_multiplier":crate::navigation_preferences::FAST_MULTIPLIER, "right_button_wheel":"Perspective right-button wheel multiplies speed by 2^(delta/240), clamped to the reported range; planar wheel zooms and ordinary perspective wheel dollies",
            "persistence":"Only Save current as defaults and Reset saved defaults write navigation preferences. Fly speed edits and right-button wheel changes never save automatically. Only fly_speed is stored; cameras, positions, sensitivity and active tools are unchanged."}).as_object().unwrap().clone());
        state
    }

    pub fn change_navigation_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        let saved = self.navigation_preferences.authority().snapshot();
        if snapshot != self.navigation_stamp(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .navigation_preferences
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                let value = self
                    .navigation_preferences
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.set_fly_speed(value.fly_speed)?;
                self.log(actor, "applied saved navigation defaults");
            }
            "save" | "reset" => {
                let value = if action == "reset" {
                    Navigation::default()
                } else {
                    self.navigation_current
                };
                self.navigation_preferences.authority().save(
                    saved.serial,
                    STALE,
                    value,
                    action == "reset",
                )?;
                write_committed = true;
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
