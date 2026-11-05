use std::collections::BTreeSet;

use map_core::fog::{
    ControllerFrame, Event, Input, MAX_EVENTS, PLAYER_UPDATE_HZ, PlayerEventKind, PlayerFrame,
    PlayerSwitch, PlayerTimeline, RawParams, Timeline,
};
use render::fog::ViewFog;
use serde_json::{Value, json};

use super::state::ControllerInput;
use super::{SelectionMode, profile_report};
use crate::session::Session;

fn raw_report(raw: RawParams) -> Value {
    json!({"profile":profile_report(raw.profile),"color_to":raw.color_to,
        "color2_to":raw.color2_to,"start_to":raw.start_to,"end_to":raw.end_to,
        "lerp_end_time_seconds":raw.lerp_end})
}

fn controller_report(controller: u32, frame: ControllerFrame) -> Value {
    json!({"controller":format!("entity:{controller}"),"server":raw_report(frame.server),
        "received":raw_report(frame.received),"changed":frame.changed,
        "pending_think_time_seconds":frame.pending_think})
}

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
        let (count, last_time) = if let Some(player) = &self.fog_preview.player {
            (
                player.history().len(),
                player.history().last().map(|event| event.time),
            )
        } else {
            (
                self.fog_preview.input_events.len(),
                self.fog_preview
                    .input_events
                    .last()
                    .map(|input| input.event.time),
            )
        };
        if count >= MAX_EVENTS {
            return Err("Fog preview reached 128 events; Reset before adding more".into());
        }
        if last_time.is_some_and(|time| self.fog_preview.time < time) {
            return Err(
                "Fog events must be appended in time order; Reset before editing an earlier time"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn controller_input_target_admission(&self, controller: u32) -> Result<(), String> {
        self.controller_input_admission()?;
        let profile = self.initial_fog_profile(controller)?;
        if let Some(player) = &self.fog_preview.player {
            let mut candidate = player.clone();
            candidate.register_controller(controller, profile)?;
            candidate.can_input(self.fog_preview.time, controller)?;
        } else {
            Timeline::new(profile)?;
        }
        Ok(())
    }

    pub(super) fn append_controller_input(
        &mut self,
        controller: u32,
        input: Input,
    ) -> Result<(), String> {
        self.controller_input_admission()?;
        let profile = self.initial_fog_profile(controller)?;
        if let Some(player) = &self.fog_preview.player {
            let mut candidate = player.clone();
            candidate.register_controller(controller, profile)?;
            candidate.push_input(self.fog_preview.time, controller, input)?;
            ViewFog::controller(candidate.evaluate(self.fog_preview.time)?.profile)?;
            self.fog_preview.player = Some(candidate);
        } else {
            let mut timelines = self.fog_preview.timelines.clone();
            if let std::collections::btree_map::Entry::Vacant(entry) = timelines.entry(controller) {
                entry.insert(Timeline::new(profile)?);
            }
            let event = Event {
                time: self.fog_preview.time,
                input,
            };
            timelines.get_mut(&controller).unwrap().push(event)?;
            Self::validate_fog_timelines(&timelines, self.fog_preview.time)?;
            self.fog_preview
                .input_events
                .try_reserve(1)
                .map_err(|_| "Fog input history allocation failed")?;
            self.fog_preview
                .input_events
                .push(ControllerInput { controller, event });
            self.fog_preview.timelines = timelines;
        }
        Ok(())
    }

    pub(super) fn player_timeline_for_switch(
        &self,
    ) -> Result<(PlayerTimeline, PlayerFrame), String> {
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
        if let Some(player) = &self.fog_preview.player {
            let frame = player.prepare_switch(self.fog_preview.time)?;
            return Ok((player.clone(), frame));
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
        let mut player = PlayerTimeline::new(initial, profile)?;
        for input in &self.fog_preview.input_events {
            player.register_controller(
                input.controller,
                self.initial_fog_profile(input.controller)?,
            )?;
            player.push_input(input.event.time, input.controller, input.event.input)?;
        }
        let frame = player.prepare_switch(self.fog_preview.time)?;
        Ok((player, frame))
    }

    pub(super) fn append_player_switch(&mut self, controller: u32) -> Result<(), String> {
        let (mut player, _) = self.player_timeline_for_switch()?;
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
        self.fog_preview.timelines.clear();
        self.fog_preview.input_events.clear();
        Ok(())
    }

    pub(super) fn validate_player_fog(&self, time: f64) -> Result<(), String> {
        if let Some(player) = &self.fog_preview.player {
            ViewFog::controller(player.evaluate(time)?.profile)?;
        }
        Ok(())
    }

    pub(super) fn fog_events_report(&self) -> Vec<Value> {
        if let Some(player) = &self.fog_preview.player {
            return player
                .history()
                .iter()
                .map(|event| match event.kind {
                    PlayerEventKind::Input(input) => json!({"kind":"input",
                        "controller":format!("entity:{}",event.controller),"time_seconds":event.time,
                        "input":input.name(),"value":input.value()}),
                    PlayerEventKind::Switch => json!({"kind":"switch_player_controller",
                        "controller":format!("entity:{}",event.controller),"time_seconds":event.time}),
                })
                .collect();
        }
        self.fog_preview
            .input_events
            .iter()
            .map(|input| {
                json!({"kind":"input",
            "controller":format!("entity:{}",input.controller),"time_seconds":input.event.time,
            "input":input.event.input.name(),"value":input.event.input.value()})
            })
            .collect()
    }

    pub(super) fn controller_states_report(&self) -> Vec<Value> {
        if let Some(player) = &self.fog_preview.player {
            return match player.controller_frames(self.fog_preview.time) {
                Ok(frames) => frames
                    .into_iter()
                    .map(|(controller, frame)| controller_report(controller, frame))
                    .collect(),
                Err(error) => vec![json!({"issue":error})],
            };
        }
        let mut ids = BTreeSet::new();
        ids.extend(self.fog_preview.timelines.keys().copied());
        match self.fog_preview.settings.selection {
            SelectionMode::Explicit => ids.extend(self.fog_preview.settings.controller),
            SelectionMode::Environment => ids.extend(
                self.fog_preview
                    .environment
                    .as_ref()
                    .and_then(|environment| environment.snapshot.initial_controller()),
            ),
        }
        ids.into_iter()
            .map(|controller| {
                let frame = if let Some(timeline) = self.fog_preview.timelines.get(&controller) {
                    timeline.frame(self.fog_preview.time)
                } else {
                    self.initial_fog_profile(controller)
                        .and_then(Timeline::new)
                        .and_then(|timeline| timeline.frame(self.fog_preview.time))
                };
                match frame {
                    Ok(frame) => controller_report(controller, frame),
                    Err(error) => {
                        json!({"controller":format!("entity:{controller}"),"issue":error})
                    }
                }
            })
            .collect()
    }

    pub(super) fn player_fog_report(&self) -> Result<Value, String> {
        let Some(player) = &self.fog_preview.player else {
            return Ok(Value::Null);
        };
        let frame = player.evaluate(self.fog_preview.time)?;
        Ok(
            json!({"initial_controller":format!("entity:{}",player.initial_controller()),
            "current_controller":format!("entity:{}",frame.controller),
            "old_controller":format!("entity:{}",frame.old_controller),
            "initial_profile":profile_report(player.initial_profile()),"profile":profile_report(frame.profile),
            "current_raw":raw_report(frame.raw),"switch_start_time_seconds":frame.switch_start,
            "old_cache":{"color":frame.old_cache.color,"start":frame.old_cache.start,"end":frame.old_cache.end},
            "new_cache":{"color":frame.new_cache.color,"start":frame.new_cache.start,"end":frame.new_cache.end},
            "sample_time_seconds":frame.sample_time_seconds,"render_time_seconds":self.fog_preview.time,"update_hz":PLAYER_UPDATE_HZ,
            "transitioning":frame.transitioning,"progress":frame.progress,"phase":frame.phase.name()}),
        )
    }
}
