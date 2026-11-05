use super::profile::{angles_direction, validate_initial_profile};
use super::{Input, Profile};

pub const MAX_EVENTS: usize = 128;
pub const MAX_TIMELINE_SECONDS: f64 = 3600.0;
pub const TIMELINE_POLICY: &str = "Preview time zero is a settled post-initialization baseline, not global engine curtime zero. No transition deadline is active until StartFogTransition. Nominal scheduled time: due controller Think commits run before an input at the same nominal time; input append order is stable. Controller arithmetic uses float32 time and the installed unclamped client interpolation: color interpolates while lerp_end >= now; range interpolates only while lerp_end > now. This does not reproduce live server ticks, network delivery or game I/O scheduling.";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub time: f64,
    pub input: Input,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    initial: Profile,
    events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawParams {
    pub profile: Profile,
    pub color_to: [f32; 3],
    pub color2_to: [f32; 3],
    pub start_to: f32,
    pub end_to: f32,
    pub lerp_end: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControllerFrame {
    pub server: RawParams,
    pub received: RawParams,
    pub changed: u8,
    pub pending_think: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct State {
    pub(super) raw: RawParams,
    pub(super) changed: u8,
    pub(super) think: Option<f32>,
}

pub(super) fn checked_time(time: f64) -> Result<f32, String> {
    if !time.is_finite() || !(0.0..=MAX_TIMELINE_SECONDS).contains(&time) {
        return Err(format!(
            "Fog time must be between 0 and {MAX_TIMELINE_SECONDS} seconds"
        ));
    }
    Ok(time as f32)
}

impl RawParams {
    fn new(profile: Profile) -> Self {
        Self {
            profile,
            color_to: profile.color,
            color2_to: profile.color2,
            start_to: 0.0,
            end_to: 0.0,
            lerp_end: None,
        }
    }

    pub(super) fn received(mut self) -> Self {
        self.profile.direction = self.profile.received_direction();
        self
    }

    pub fn evaluate(self, time: f64) -> Result<Profile, String> {
        self.evaluated(checked_time(time)?)
    }

    pub(super) fn evaluated(self, now: f32) -> Result<Profile, String> {
        let mut profile = self.profile;
        if let Some(lerp_end) = self.lerp_end
            && lerp_end >= now
        {
            if profile.lerp_time <= 0.0 {
                return Err("Fog transition has an unsafe zero duration".into());
            }
            let alpha = 1.0 + (now - lerp_end) / profile.lerp_time;
            for axis in 0..3 {
                profile.color[axis] += (self.color_to[axis] - profile.color[axis]) * alpha;
                profile.color2[axis] += (self.color2_to[axis] - profile.color2[axis]) * alpha;
            }
            if lerp_end > now && profile.start != self.start_to {
                profile.start += (self.start_to - profile.start) * alpha;
            }
            if lerp_end > now && profile.end != self.end_to {
                profile.end += (self.end_to - profile.end) * alpha;
            }
        }
        profile.validate()?;
        Ok(profile)
    }
}

impl State {
    pub(super) fn new(profile: Profile) -> Self {
        Self {
            raw: RawParams::new(profile),
            changed: 0,
            think: None,
        }
    }

    pub(super) fn frame(self) -> ControllerFrame {
        ControllerFrame {
            server: self.raw,
            received: self.raw.received(),
            changed: self.changed,
            pending_think: self.think,
        }
    }

    pub(super) fn commit_due(&mut self, now: f32) {
        if let Some(think) = self.think
            && think <= now
        {
            if self.changed & 1 != 0 {
                self.raw.profile.color = self.raw.color_to;
            }
            if self.changed & 2 != 0 {
                self.raw.profile.color2 = self.raw.color2_to;
            }
            if self.changed & 4 != 0 {
                self.raw.profile.start = self.raw.start_to;
            }
            if self.changed & 8 != 0 {
                self.raw.profile.end = self.raw.end_to;
            }
            self.changed = 0;
            self.raw.lerp_end = Some(think);
            self.think = None;
        }
    }

    pub(super) fn input(&mut self, input: Input, now: f32) -> Result<(), String> {
        input.validate()?;
        let raw = &mut self.raw;
        match input {
            Input::SetStartDist(value) => raw.profile.start = value,
            Input::SetEndDist(value) => raw.profile.end = value,
            Input::SetMaxDensity(value) => raw.profile.max_density = value,
            Input::TurnOn => raw.profile.enabled = true,
            Input::TurnOff => raw.profile.enabled = false,
            Input::SetColor(value) => raw.profile.color = value.map(f32::from),
            Input::SetColorSecondary(value) => raw.profile.color2 = value.map(f32::from),
            Input::SetFarZ(value) => raw.profile.far_z = value as f32,
            Input::SetAngles(value) => raw.profile.direction = angles_direction(value),
            Input::SetRadial(value) => raw.profile.radial = value,
            Input::SetColorLerpTo(value) => {
                raw.color_to = value.map(f32::from);
                self.changed |= 1;
            }
            Input::SetColorSecondaryLerpTo(value) => {
                raw.color2_to = value.map(f32::from);
                self.changed |= 2;
            }
            Input::SetStartDistLerpTo(value) => {
                raw.start_to = value;
                self.changed |= 4;
            }
            Input::SetEndDistLerpTo(value) => {
                raw.end_to = value;
                self.changed |= 8;
            }
            Input::StartFogTransition => {
                if raw.profile.lerp_time <= 0.0 {
                    return Err("StartFogTransition with zero foglerptime has an unsafe native interpolation interval; use direct setters or author a positive duration".into());
                }
                let commit = now + raw.profile.lerp_time;
                let deadline = (f64::from(commit) + 0.1) as f32;
                if !commit.is_finite() || !deadline.is_finite() {
                    return Err("Fog transition time is not finite".into());
                }
                raw.lerp_end = Some(deadline);
                self.think = Some(commit);
            }
        }
        Ok(())
    }
}

impl Timeline {
    pub fn new(initial: Profile) -> Result<Self, String> {
        validate_initial_profile(initial)?;
        Ok(Self {
            initial,
            events: Vec::new(),
        })
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }
    pub fn initial(&self) -> Profile {
        self.initial
    }

    pub fn push(&mut self, event: Event) -> Result<(), String> {
        checked_time(event.time)?;
        event.input.validate()?;
        if self.events.len() >= MAX_EVENTS {
            return Err(format!("Fog timeline exceeds {MAX_EVENTS} inputs"));
        }
        if self
            .events
            .last()
            .is_some_and(|previous| event.time < previous.time)
        {
            return Err(
                "Fog inputs must be appended in time order; Reset before editing an earlier time"
                    .into(),
            );
        }
        let mut candidate = self.clone();
        candidate
            .events
            .try_reserve(1)
            .map_err(|_| "Fog timeline allocation failed")?;
        candidate.events.push(event);
        candidate.evaluate(event.time)?;
        *self = candidate;
        Ok(())
    }

    pub fn evaluate(&self, time: f64) -> Result<Profile, String> {
        self.frame(time)?.server.evaluate(time)
    }

    pub fn frame(&self, time: f64) -> Result<ControllerFrame, String> {
        let now = checked_time(time)?;
        let mut state = State::new(self.initial);
        for event in &self.events {
            if event.time > time {
                break;
            }
            let event_time = checked_time(event.time)?;
            state.commit_due(event_time);
            state.input(event.input, event_time)?;
        }
        state.commit_due(now);
        Ok(state.frame())
    }
}
