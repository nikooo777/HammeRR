use super::profile::angles_direction;
use super::{Input, Profile};

pub const MAX_EVENTS: usize = 128;
pub const MAX_TIMELINE_SECONDS: f64 = 3600.0;
pub const TIMELINE_POLICY: &str = "Preview time zero is a settled post-spawn authored baseline, not global engine curtime zero. No transition deadline is active until StartFogTransition. Nominal scheduled time: due controller Think commits run before an input at the same nominal time; input append order is stable. Controller arithmetic uses float32 time and the installed unclamped client interpolation: color interpolates while lerp_end >= now; range interpolates only while lerp_end > now. This does not reproduce live server ticks, network delivery or game I/O scheduling.";

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

struct State {
    profile: Profile,
    color_to: [f32; 3],
    color2_to: [f32; 3],
    start_to: f32,
    end_to: f32,
    changed: u8,
    lerp_end: Option<f32>,
    think: Option<f32>,
}

fn checked_time(time: f64) -> Result<f32, String> {
    if !time.is_finite() || !(0.0..=MAX_TIMELINE_SECONDS).contains(&time) {
        return Err(format!(
            "Fog time must be between 0 and {MAX_TIMELINE_SECONDS} seconds"
        ));
    }
    Ok(time as f32)
}

impl State {
    fn new(profile: Profile) -> Self {
        Self {
            profile,
            color_to: profile.color,
            color2_to: profile.color2,
            start_to: 0.0,
            end_to: 0.0,
            changed: 0,
            lerp_end: None,
            think: None,
        }
    }

    fn commit_due(&mut self, now: f32) {
        if let Some(think) = self.think
            && think <= now
        {
            if self.changed & 1 != 0 {
                self.profile.color = self.color_to;
            }
            if self.changed & 2 != 0 {
                self.profile.color2 = self.color2_to;
            }
            if self.changed & 4 != 0 {
                self.profile.start = self.start_to;
            }
            if self.changed & 8 != 0 {
                self.profile.end = self.end_to;
            }
            self.changed = 0;
            self.lerp_end = Some(think);
            self.think = None;
        }
    }

    fn input(&mut self, input: Input, now: f32) -> Result<(), String> {
        input.validate()?;
        match input {
            Input::SetStartDist(value) => self.profile.start = value,
            Input::SetEndDist(value) => self.profile.end = value,
            Input::SetMaxDensity(value) => self.profile.max_density = value,
            Input::TurnOn => self.profile.enabled = true,
            Input::TurnOff => self.profile.enabled = false,
            Input::SetColor(value) => self.profile.color = value.map(f32::from),
            Input::SetColorSecondary(value) => self.profile.color2 = value.map(f32::from),
            Input::SetFarZ(value) => self.profile.far_z = value as f32,
            Input::SetAngles(value) => self.profile.direction = angles_direction(value),
            Input::SetRadial(value) => self.profile.radial = value,
            Input::SetColorLerpTo(value) => {
                self.color_to = value.map(f32::from);
                self.changed |= 1;
            }
            Input::SetColorSecondaryLerpTo(value) => {
                self.color2_to = value.map(f32::from);
                self.changed |= 2;
            }
            Input::SetStartDistLerpTo(value) => {
                self.start_to = value;
                self.changed |= 4;
            }
            Input::SetEndDistLerpTo(value) => {
                self.end_to = value;
                self.changed |= 8;
            }
            Input::StartFogTransition => {
                if self.profile.lerp_time <= 0.0 {
                    return Err("StartFogTransition with zero foglerptime has an unsafe native interpolation interval; use direct setters or author a positive duration".into());
                }
                let commit = now + self.profile.lerp_time;
                let deadline = (f64::from(commit) + 0.1) as f32;
                if !commit.is_finite() || !deadline.is_finite() {
                    return Err("Fog transition time is not finite".into());
                }
                self.lerp_end = Some(deadline);
                self.think = Some(commit);
            }
        }
        Ok(())
    }

    fn evaluated(&self, now: f32) -> Result<Profile, String> {
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

impl Timeline {
    pub fn new(initial: Profile) -> Result<Self, String> {
        initial.validate()?;
        if initial
            .color
            .into_iter()
            .chain(initial.color2)
            .any(|value| !(0.0..=255.0).contains(&value) || value.fract() != 0.0)
        {
            return Err("Fog timeline needs authored RGB-byte initial colors".into());
        }
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
        state.evaluated(now)
    }
}
