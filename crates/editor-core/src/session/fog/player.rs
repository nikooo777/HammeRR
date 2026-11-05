use map_core::fog::{MAX_EVENTS, PlayerSwitch, PlayerTimeline};
use render::fog::ViewFog;
use serde_json::{Value, json};

use super::{SelectionMode, profile_report};
use crate::session::Session;

impl Session {
    pub(super) fn controller_input_admission(&self) -> Result<(), String> {
        if !self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
        {
            return Err("Apply Fog settings or Reset before sending controller inputs".into());
        }
        if self.fog_preview.worker.busy() {
            return Err(
                "Wait for fog environment preparation before sending controller inputs".into(),
            );
        }
        if self.fog_preview.player.is_some() {
            return Err("Controller inputs cannot coexist with player fog switches; Reset the preview first".into());
        }
        if self
            .fog_preview
            .timelines
            .values()
            .map(|timeline| timeline.events().len())
            .sum::<usize>()
            >= MAX_EVENTS
        {
            return Err("Fog preview reached 128 input events; Reset before adding more".into());
        }
        Ok(())
    }

    pub(super) fn player_timeline_for_switch(&self) -> Result<PlayerTimeline, String> {
        self.ensure_not_closing()?;
        if !self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
        {
            return Err(
                "Apply Fog settings or Reset before switching the player controller".into(),
            );
        }
        if self.fog_preview.worker.busy() {
            return Err(
                "Wait for fog environment preparation before switching the player controller"
                    .into(),
            );
        }
        if !self.fog_preview.settings.enabled {
            return Err("Enable Fog preview before switching the player controller".into());
        }
        if !self.fog_preview.timelines.is_empty() {
            return Err("Player fog switches cannot coexist with controller inputs; Reset the preview first".into());
        }
        if let Some(player) = &self.fog_preview.player {
            player.prepare_switch(self.fog_preview.time)?;
            return Ok(player.clone());
        }
        let initial = match self.fog_preview.settings.selection {
            SelectionMode::Explicit => self
                .fog_preview
                .settings
                .controller
                .ok_or("Choose an initial fog controller before switching the player")?,
            SelectionMode::Environment => self
                .fog_preview
                .environment
                .as_ref()
                .ok_or("Prepare the fog environment before switching the player controller")?
                .snapshot
                .initial_controller()
                .ok_or("The prepared environment has no initial fog controller")?,
        };
        let profile = self.initial_fog_profile(initial)?;
        ViewFog::controller(profile)?;
        PlayerTimeline::new(initial, profile)
    }

    pub(super) fn append_player_switch(&mut self, controller: u32) -> Result<(), String> {
        let mut player = self.player_timeline_for_switch()?;
        let target = self
            .fog_preview
            .catalog
            .as_ref()
            .ok_or("Capture the fog controller catalog before switching")?
            .controllers
            .get(&controller)
            .ok_or("Player fog target is not an authored fog controller")?;
        target.switch_target.as_ref().map_err(Clone::clone)?;
        target.parsed.as_ref().map_err(Clone::clone)?;
        let profile = self.initial_fog_profile(controller)?;
        player.push(PlayerSwitch {
            time: self.fog_preview.time,
            controller,
            profile,
        })?;
        ViewFog::controller(player.evaluate(self.fog_preview.time)?.profile)?;
        self.fog_preview.player = Some(player);
        Ok(())
    }

    pub(super) fn validate_player_fog(&self, time: f64) -> Result<(), String> {
        if let Some(player) = &self.fog_preview.player {
            ViewFog::controller(player.evaluate(time)?.profile)?;
        }
        Ok(())
    }

    pub(super) fn player_fog_report(&self) -> Result<Value, String> {
        let Some(player) = &self.fog_preview.player else {
            return Ok(Value::Null);
        };
        let frame = player.evaluate(self.fog_preview.time)?;
        Ok(
            json!({"initial_controller":format!("entity:{}",player.initial_controller()),
            "current_controller":format!("entity:{}",frame.controller),
            "initial_profile":profile_report(player.initial_profile()),"profile":profile_report(frame.profile),
            "transitioning":frame.transitioning,"progress":frame.progress,"phase":frame.phase.name()}),
        )
    }
}
