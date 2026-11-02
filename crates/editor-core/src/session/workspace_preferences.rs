use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Actor, Session};
use crate::saved_defaults::{Binding, Snapshot};
use crate::workspace_preferences::{Layout, Store};

#[derive(Default)]
pub(super) struct State {
    pub(super) binding: Binding<Store>,
    layout: Layout,
    serial: u64,
    started: bool,
}

const STALE: &str = "Workspace layout or defaults changed; inspect Workspace defaults again";
const LAYOUT_STALE: &str = "Workspace layout changed; inspect the current layout again";

impl Session {
    pub(super) fn configure_workspace_preferences(&mut self, directory: Result<PathBuf, String>) {
        if self.workspace_preferences.started {
            return;
        }
        let _ = self.workspace_preferences.binding.configure(directory);
        self.apply_startup_workspace_preferences();
    }

    pub(super) fn apply_startup_workspace_preferences(&mut self) {
        if self.workspace_preferences.started {
            return;
        }
        self.workspace_preferences.started = true;
        let saved = self.workspace_preferences.binding.authority().snapshot();
        if saved.loaded {
            if let Err(error) = self.apply_workspace_layout(saved.saved.unwrap_or_default()) {
                self.log(
                    Actor::System,
                    format!("Workspace defaults could not apply: {error}"),
                );
            }
        } else if let Some(error) = saved.error.or(saved.storage_error) {
            self.log(
                Actor::System,
                format!("Workspace defaults could not load: {error}"),
            );
        }
    }

    fn workspace_layout_snapshot(&self) -> String {
        format!(
            "workspace-layout:{}:{}",
            self.buffer_review_scope, self.workspace_preferences.serial
        )
    }

    fn workspace_preferences_stamp(&self, saved: &Snapshot<Layout>) -> String {
        format!(
            "workspace-defaults:{}:{}:{}",
            self.buffer_review_scope, self.workspace_preferences.serial, saved.serial
        )
    }

    pub fn workspace_layout(&self) -> Layout {
        self.workspace_preferences.layout
    }

    pub fn workspace_layout_serial(&self) -> u64 {
        self.workspace_preferences.serial
    }

    pub fn workspace_preferences_state(&self) -> Value {
        let saved = self.workspace_preferences.binding.authority().snapshot();
        let mut state = saved.report(
            self.workspace_layout(),
            self.workspace_preferences_stamp(&saved),
        );
        state["layout_snapshot"] = json!(self.workspace_layout_snapshot());
        state["layout_serial"] = json!(self.workspace_layout_serial());
        state["limits"] = json!({
            "file_bytes":crate::workspace_preferences::MAX_FILE_BYTES,
            "min_dock_width":crate::workspace_preferences::MIN_DOCK_WIDTH,
            "max_dock_width":crate::workspace_preferences::MAX_DOCK_WIDTH,
            "min_agent_height":crate::workspace_preferences::MIN_AGENT_HEIGHT,
            "max_agent_height":crate::workspace_preferences::MAX_AGENT_HEIGHT
        });
        state["persistence"] = json!(
            "Only Save current as defaults and Reset saved defaults write workspace preferences. Save stores the applied layout. Startup applies loaded or absent defaults once per Session. Apply changes this Session's layout; Reload and Reset do not apply it. New/Open retain the current layout. Ordinary layout changes never save automatically and remain available when saved defaults are unavailable. Layout changes cancel captured viewport gestures while preserving inactive reviews, map source, history and camera."
        );
        state
    }

    fn apply_workspace_layout(&mut self, layout: Layout) -> Result<bool, String> {
        layout.validate()?;
        if self.workspace_preferences.layout == layout {
            return Ok(false);
        }
        let serial = self
            .workspace_preferences
            .serial
            .checked_add(1)
            .ok_or("Workspace layout revision exhausted")?;
        self.invalidate_display_visibility_gestures();
        self.workspace_preferences.layout = layout;
        self.workspace_preferences.serial = serial;
        self.view_serial += 1;
        Ok(true)
    }

    pub fn set_workspace_layout(
        &mut self,
        actor: Actor,
        layout: Layout,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if snapshot != self.workspace_layout_snapshot() {
            return Err(LAYOUT_STALE.into());
        }
        if self.apply_workspace_layout(layout)? {
            self.log(actor, "Changed workspace layout");
        }
        Ok(self.workspace_preferences_state())
    }

    pub fn change_workspace_preferences(
        &mut self,
        actor: Actor,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let saved = self.workspace_preferences.binding.authority().snapshot();
        if snapshot != self.workspace_preferences_stamp(&saved) {
            return Err(STALE.into());
        }
        let mut write_committed = false;
        match action {
            "reload" => self
                .workspace_preferences
                .binding
                .authority()
                .reload(saved.serial, STALE)?,
            "apply" => {
                let layout = self
                    .workspace_preferences
                    .binding
                    .authority()
                    .checked_value(saved.serial, STALE)?;
                self.apply_workspace_layout(layout)?;
                self.log(actor, "Applied saved workspace defaults");
            }
            "save" | "reset" => {
                let layout = if action == "reset" {
                    Layout::default()
                } else {
                    self.workspace_layout()
                };
                self.workspace_preferences.binding.authority().save(
                    saved.serial,
                    STALE,
                    layout,
                    action == "reset",
                )?;
                write_committed = true;
                self.log(
                    actor,
                    if action == "reset" {
                        "Reset saved workspace defaults"
                    } else {
                        "Saved current workspace defaults"
                    },
                );
            }
            _ => return Err("Choose save, apply, reload or reset for workspace defaults".into()),
        }
        let mut state = self.workspace_preferences_state();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}
