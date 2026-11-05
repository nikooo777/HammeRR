use super::profile::validate_initial_profile;
use super::timeline::checked_time;
use super::{MAX_EVENTS, Profile};

pub const PLAYER_TIMELINE_POLICY: &str = "Switch-only preview over immutable settled static controller profiles. The known initial controller is snapped at the settled logical epoch. Before a nonoverlapping switch, the previous player blend is settled at that operation time. Overlapping switches and controller-input histories are unavailable because native interruptions cache the last written player state. Events have stable append order, including equal logical times; blend timing uses float32 elapsed and the destination's copied duration. Only primary RGB and start/end blend; all other fields copy immediately. Seeking is analytic and query-independent. This does not reproduce packet delivery, prediction cadence, native update/render history or arbitrary controller-input interleaving.";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerSwitch {
    pub time: f64,
    pub controller: u32,
    pub profile: Profile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerSwitchPhase {
    Initial,
    Blending,
    Settled,
}

impl PlayerSwitchPhase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Blending => "blending",
            Self::Settled => "settled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerFrame {
    pub controller: u32,
    pub profile: Profile,
    pub transitioning: bool,
    pub progress: f32,
    pub phase: PlayerSwitchPhase,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerTimeline {
    initial_controller: u32,
    initial_profile: Profile,
    events: Vec<PlayerSwitch>,
}

fn blended_profile(old: Profile, new: Profile, alpha: f32) -> Result<Profile, String> {
    let complement = 1.0 - alpha;
    let mut profile = new;
    for axis in 0..3 {
        let new_term = new.color[axis] * alpha;
        let old_term = old.color[axis] * complement;
        let sum = old_term + new_term;
        profile.color[axis] = f32::from(sum as i32 as u8);
    }
    profile.start = new.start * alpha + old.start * complement;
    profile.end = alpha * new.end + complement * old.end;
    profile.validate()?;
    Ok(profile)
}

impl PlayerTimeline {
    pub fn new(initial_controller: u32, initial_profile: Profile) -> Result<Self, String> {
        validate_initial_profile(initial_profile)?;
        Ok(Self {
            initial_controller,
            initial_profile,
            events: Vec::new(),
        })
    }

    pub fn initial_controller(&self) -> u32 {
        self.initial_controller
    }

    pub fn initial_profile(&self) -> Profile {
        self.initial_profile
    }

    pub fn events(&self) -> &[PlayerSwitch] {
        &self.events
    }

    pub fn prepare_switch(&self, time: f64) -> Result<PlayerFrame, String> {
        checked_time(time)?;
        if self.events.len() >= MAX_EVENTS {
            return Err(format!(
                "Player fog timeline exceeds {MAX_EVENTS} switches; Reset before adding more"
            ));
        }
        if self
            .events
            .last()
            .is_some_and(|previous| time < previous.time)
        {
            return Err("Player fog switches must be appended in time order; Reset before editing an earlier time".into());
        }
        let current = self.evaluate(time)?;
        if current.transitioning {
            return Err("Wait for the active player fog switch to settle before switching again; interrupted switches are unavailable".into());
        }
        Ok(current)
    }

    pub fn can_switch(&self, time: f64, controller: u32) -> Result<(), String> {
        let current = self.prepare_switch(time)?;
        if controller == current.controller {
            return Err("The player already uses this fog controller; a same-controller request cannot restart a switch".into());
        }
        Ok(())
    }

    pub fn push(&mut self, event: PlayerSwitch) -> Result<(), String> {
        validate_initial_profile(event.profile)?;
        if (event.controller == self.initial_controller && event.profile != self.initial_profile)
            || self.events.iter().any(|previous| {
                previous.controller == event.controller && previous.profile != event.profile
            })
        {
            return Err("Player fog switching requires immutable controller profiles; Reset after a controller or initial-context change".into());
        }
        self.can_switch(event.time, event.controller)?;
        let mut candidate = self.clone();
        candidate
            .events
            .try_reserve(1)
            .map_err(|_| "Player fog timeline allocation failed")?;
        candidate.events.push(event);
        candidate.evaluate(event.time)?;
        *self = candidate;
        Ok(())
    }

    pub fn evaluate(&self, time: f64) -> Result<PlayerFrame, String> {
        let now = checked_time(time)?;
        let mut old = self.initial_profile;
        let mut selected: Option<&PlayerSwitch> = None;
        for event in &self.events {
            if event.time > time {
                break;
            }
            if let Some(previous) = selected {
                old = previous.profile;
            }
            selected = Some(event);
        }
        let Some(event) = selected else {
            self.initial_profile.validate()?;
            return Ok(PlayerFrame {
                controller: self.initial_controller,
                profile: self.initial_profile,
                transitioning: false,
                progress: 1.0,
                phase: PlayerSwitchPhase::Initial,
            });
        };
        let elapsed = now - checked_time(event.time)?;
        if elapsed < 0.0 {
            return Err("Player fog evaluation precedes its selected switch".into());
        }
        let duration = event.profile.lerp_time;
        if duration <= elapsed {
            event.profile.validate()?;
            return Ok(PlayerFrame {
                controller: event.controller,
                profile: event.profile,
                transitioning: false,
                progress: 1.0,
                phase: PlayerSwitchPhase::Settled,
            });
        }
        let progress = elapsed / duration;
        let profile = blended_profile(old, event.profile, progress)?;
        Ok(PlayerFrame {
            controller: event.controller,
            profile,
            transitioning: true,
            progress,
            phase: PlayerSwitchPhase::Blending,
        })
    }
}
