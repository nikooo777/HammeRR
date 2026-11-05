use super::profile::validate_initial_profile;
use super::timeline::{State, checked_time};
use super::{ControllerFrame, Input, MAX_EVENTS, Profile, RawParams};

pub const PLAYER_UPDATE_HZ: u32 = 50;
pub const PLAYER_UPDATE_INTERVAL_SECONDS: f64 = 1.0 / PLAYER_UPDATE_HZ as f64;
pub const MAX_PLAYER_CONTROLLERS: usize = 128;
const MAX_EXPANDED_PLAYER_EVENTS: usize = 2 * MAX_EVENTS;
pub const PLAYER_TIMELINE_POLICY: &str = "Mixed controller/player preview from a settled logical epoch, including initial CREATED delivery and a following ordinary player data update. Controller inputs and due nominal Think commits immediately deliver the full controller raw params without a player data callback. Due commits run before equal-time events. Inputs and switches retain append order. Each switch is a separate player delivery before a scheduled update at the same time: capture old handle, deliver the new handle, drain its callback, cache the last-written player primary RGB/start/end, copy all destination raw params and immediately update once. Ordinary updates run at k/50 seconds after equal-time events; each explicitly includes a player data delivery refreshing oldHandle to the current handle, then one Simulate update. No prediction updates are inserted. An active switch freezes all copied controller params. Completion writes cached destination primary RGB/start/end and clears the switch; recopy can occur only on a later invocation whose old and current handles agree. Blend timing uses float32 elapsed and copied duration; RGB writes truncate to bytes. The renderer applies the copied controller targets/deadline interpolation after the latest player write, at query time. Queries and Seek add no updates; replay is independent of query frequency and clock partitioning. Ordinary manual ledgers admit at most 128 combined events; bounded trigger expansion admits at most 256 raw controller events. At most 128 registered controllers, through 3600 seconds. This nominal schedule does not reproduce native packet delivery, prediction, physics or render history.";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerSwitch {
    pub time: f64,
    pub controller: u32,
    pub profile: Profile,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayerEventKind {
    Input(Input),
    Switch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerEvent {
    pub time: f64,
    pub controller: u32,
    pub kind: PlayerEventKind,
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
pub struct PlayerSwitchCache {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

impl PlayerSwitchCache {
    fn from_profile(profile: Profile) -> Self {
        Self {
            color: profile.color,
            start: profile.start,
            end: profile.end,
        }
    }

    fn write(self, profile: &mut Profile) {
        profile.color = self.color;
        profile.start = self.start;
        profile.end = self.end;
    }

    fn blend(self, new: Self, alpha: f32, profile: &mut Profile) {
        let complement = 1.0 - alpha;
        for axis in 0..3 {
            let new_term = new.color[axis] * alpha;
            let old_term = self.color[axis] * complement;
            let sum = old_term + new_term;
            profile.color[axis] = f32::from(sum as i32 as u8);
        }
        profile.start = new.start * alpha + self.start * complement;
        profile.end = alpha * new.end + complement * self.end;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerFrame {
    pub controller: u32,
    pub profile: Profile,
    pub raw: RawParams,
    pub old_controller: u32,
    pub switch_start: Option<f32>,
    pub old_cache: PlayerSwitchCache,
    pub new_cache: PlayerSwitchCache,
    pub sample_time_seconds: f64,
    pub transitioning: bool,
    pub progress: f32,
    pub phase: PlayerSwitchPhase,
}

pub(super) fn can_switch_controller(current: u32, controller: u32) -> Result<(), String> {
    if controller == current {
        return Err("The player already uses this fog controller; a same-controller request cannot restart a switch".into());
    }
    Ok(())
}

impl PlayerFrame {
    pub fn can_switch(&self, controller: u32) -> Result<(), String> {
        can_switch_controller(self.controller, controller)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RegisteredController {
    id: u32,
    profile: Profile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerTimeline {
    initial_controller: u32,
    initial_profile: Profile,
    controllers: Vec<RegisteredController>,
    switches: Vec<PlayerSwitch>,
    history: Vec<PlayerEvent>,
}

struct ControllerState {
    id: u32,
    server: State,
    received: RawParams,
}

impl ControllerState {
    fn frame(&self) -> ControllerFrame {
        ControllerFrame {
            server: self.server.raw,
            received: self.received,
            changed: self.server.changed,
            pending_think: self.server.think,
        }
    }
}

struct PlayerState {
    controller: u32,
    old_controller: u32,
    raw: RawParams,
    switch_start: Option<f32>,
    old_cache: PlayerSwitchCache,
    new_cache: PlayerSwitchCache,
    sample_time_seconds: f64,
    progress: f32,
    phase: PlayerSwitchPhase,
}

impl PlayerState {
    fn update(&mut self, time: f64, received: RawParams) -> Result<(), String> {
        let now = checked_time(time)?;
        if self.switch_start.is_none() && self.old_controller == self.controller {
            self.raw = received;
        }
        if let Some(start) = self.switch_start {
            let elapsed = now - start;
            if elapsed < 0.0 {
                return Err("Player fog update precedes its active switch".into());
            }
            if elapsed < self.raw.profile.lerp_time {
                self.progress = elapsed / self.raw.profile.lerp_time;
                self.old_cache
                    .blend(self.new_cache, self.progress, &mut self.raw.profile);
                self.phase = PlayerSwitchPhase::Blending;
            } else {
                self.new_cache.write(&mut self.raw.profile);
                self.switch_start = None;
                self.progress = 1.0;
                self.phase = PlayerSwitchPhase::Settled;
            }
        }
        self.raw.profile.validate()?;
        self.sample_time_seconds = time;
        Ok(())
    }

    fn ordinary_update(&mut self, tick: u32, received: RawParams) -> Result<(), String> {
        self.old_controller = self.controller;
        self.update(tick_time(tick), received)
    }

    fn switch(&mut self, event: PlayerEvent, received: RawParams) -> Result<(), String> {
        can_switch_controller(self.controller, event.controller)?;
        self.old_controller = self.controller;
        self.controller = event.controller;
        self.old_cache = PlayerSwitchCache::from_profile(self.raw.profile);
        self.new_cache = PlayerSwitchCache::from_profile(received.profile);
        self.raw = received;
        self.switch_start = Some(checked_time(event.time)?);
        self.update(event.time, received)
    }

    fn completion_at(&self, tick: u32) -> bool {
        self.switch_start
            .is_some_and(|start| tick_time(tick) as f32 - start >= self.raw.profile.lerp_time)
    }

    fn advance_ticks(&mut self, first: u32, last: u32, received: RawParams) -> Result<(), String> {
        if first > last {
            return Ok(());
        }
        if self.completion_at(last) {
            let mut low = first;
            let mut high = last;
            while low < high {
                let middle = low + (high - low) / 2;
                if self.completion_at(middle) {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            self.ordinary_update(low, received)?;
            if low == last {
                return Ok(());
            }
        }
        self.ordinary_update(last, received)
    }

    fn frame(&self, time: f64) -> Result<PlayerFrame, String> {
        Ok(PlayerFrame {
            controller: self.controller,
            profile: self.raw.evaluate(time)?,
            raw: self.raw,
            old_controller: self.old_controller,
            switch_start: self.switch_start,
            old_cache: self.old_cache,
            new_cache: self.new_cache,
            sample_time_seconds: self.sample_time_seconds,
            transitioning: self.switch_start.is_some(),
            progress: self.progress,
            phase: self.phase,
        })
    }
}

struct Replay {
    controllers: Vec<ControllerState>,
    player: PlayerState,
    next_tick: u32,
}

fn tick_time(tick: u32) -> f64 {
    f64::from(tick) / f64::from(PLAYER_UPDATE_HZ)
}

fn scheduled_update_tick(time: f64, inclusive: bool) -> Option<u32> {
    let mut tick = (time * f64::from(PLAYER_UPDATE_HZ)).floor() as u32;
    if tick_time(tick) > time {
        tick = tick.checked_sub(1)?;
    } else if tick_time(tick + 1) <= time {
        tick += 1;
    }
    if !inclusive && tick_time(tick) == time {
        tick = tick.checked_sub(1)?;
    }
    Some(tick)
}

impl Replay {
    fn new(timeline: &PlayerTimeline) -> Result<Self, String> {
        let mut controllers = Vec::new();
        controllers
            .try_reserve(timeline.controllers.len())
            .map_err(|_| "Fog controller replay allocation failed")?;
        for controller in &timeline.controllers {
            let server = State::new(controller.profile);
            controllers.push(ControllerState {
                id: controller.id,
                received: server.raw.received(),
                server,
            });
        }
        let raw = controllers
            .iter()
            .find(|controller| controller.id == timeline.initial_controller)
            .ok_or("Initial player fog controller is absent from the catalog")?
            .received;
        let cache = PlayerSwitchCache::from_profile(raw.profile);
        Ok(Self {
            controllers,
            player: PlayerState {
                controller: timeline.initial_controller,
                old_controller: timeline.initial_controller,
                raw,
                switch_start: None,
                old_cache: PlayerSwitchCache {
                    color: [0.0; 3],
                    start: 0.0,
                    end: 0.0,
                },
                new_cache: cache,
                sample_time_seconds: 0.0,
                progress: 1.0,
                phase: PlayerSwitchPhase::Initial,
            },
            next_tick: 0,
        })
    }

    fn controller(&self, id: u32) -> Result<&ControllerState, String> {
        self.controllers
            .iter()
            .find(|controller| controller.id == id)
            .ok_or_else(|| format!("Fog controller entity:{id} is absent from the replay catalog"))
    }

    fn commit_due(&mut self, now: f32) {
        for controller in &mut self.controllers {
            if controller.server.think.is_some_and(|think| think <= now) {
                controller.server.commit_due(now);
                controller.received = controller.server.raw.received();
            }
        }
    }

    fn advance_ticks(&mut self, time: f64, inclusive: bool) -> Result<(), String> {
        let Some(last) = scheduled_update_tick(time, inclusive) else {
            return Ok(());
        };
        while self.next_tick <= last {
            let due = self
                .controllers
                .iter()
                .filter_map(|controller| controller.server.think)
                .filter(|&think| think <= tick_time(last) as f32)
                .min_by(f32::total_cmp);
            let Some(think) = due else {
                let received = self.controller(self.player.controller)?.received;
                self.player.advance_ticks(self.next_tick, last, received)?;
                self.next_tick = last + 1;
                break;
            };
            let mut low = self.next_tick;
            let mut high = last;
            while low < high {
                let middle = low + (high - low) / 2;
                if tick_time(middle) as f32 >= think {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            if self.next_tick < low {
                let received = self.controller(self.player.controller)?.received;
                self.player
                    .advance_ticks(self.next_tick, low - 1, received)?;
            }
            self.commit_due(tick_time(low) as f32);
            let received = self.controller(self.player.controller)?.received;
            self.player.ordinary_update(low, received)?;
            self.next_tick = low + 1;
        }
        Ok(())
    }

    fn event(&mut self, event: PlayerEvent) -> Result<(), String> {
        self.advance_ticks(event.time, false)?;
        let now = checked_time(event.time)?;
        self.commit_due(now);
        match event.kind {
            PlayerEventKind::Input(input) => {
                let controller = self
                    .controllers
                    .iter_mut()
                    .find(|controller| controller.id == event.controller)
                    .ok_or_else(|| {
                        format!(
                            "Fog controller entity:{} is absent from the replay catalog",
                            event.controller
                        )
                    })?;
                controller.server.input(input, now)?;
                controller.received = controller.server.raw.received();
            }
            PlayerEventKind::Switch => {
                let received = self.controller(event.controller)?.received;
                self.player.switch(event, received)?;
            }
        }
        Ok(())
    }
}

impl PlayerTimeline {
    pub fn new(initial_controller: u32, initial_profile: Profile) -> Result<Self, String> {
        validate_initial_profile(initial_profile)?;
        let mut timeline = Self {
            initial_controller,
            initial_profile,
            controllers: Vec::new(),
            switches: Vec::new(),
            history: Vec::new(),
        };
        timeline.register_controller(initial_controller, initial_profile)?;
        Ok(timeline)
    }

    pub fn from_expanded_events(
        initial_controller: u32,
        initial_profile: Profile,
        controllers: &[(u32, Profile)],
        events: &[PlayerEvent],
    ) -> Result<Self, String> {
        if events.len() > MAX_EXPANDED_PLAYER_EVENTS {
            return Err(format!(
                "Expanded fog timeline exceeds {MAX_EXPANDED_PLAYER_EVENTS} controller events"
            ));
        }
        let mut timeline = Self::new(initial_controller, initial_profile)?;
        for &(controller, profile) in controllers {
            timeline.register_controller(controller, profile)?;
        }
        let mut current = initial_controller;
        for &event in events {
            timeline.can_append_with_limit(event.time, MAX_EXPANDED_PLAYER_EVENTS)?;
            let profile = timeline.controller_profile(event.controller)?;
            match event.kind {
                PlayerEventKind::Input(input) => input.validate()?,
                PlayerEventKind::Switch => {
                    can_switch_controller(current, event.controller)?;
                    timeline
                        .switches
                        .try_reserve(1)
                        .map_err(|_| "Player fog switch allocation failed")?;
                    timeline.switches.push(PlayerSwitch {
                        time: event.time,
                        controller: event.controller,
                        profile,
                    });
                    current = event.controller;
                }
            }
            timeline.append(event)?;
        }
        Ok(timeline)
    }

    pub fn initial_controller(&self) -> u32 {
        self.initial_controller
    }

    pub fn initial_profile(&self) -> Profile {
        self.initial_profile
    }

    pub fn events(&self) -> &[PlayerSwitch] {
        &self.switches
    }

    pub fn history(&self) -> &[PlayerEvent] {
        &self.history
    }

    pub fn register_controller(&mut self, id: u32, profile: Profile) -> Result<(), String> {
        validate_initial_profile(profile)?;
        if let Some(previous) = self
            .controllers
            .iter()
            .find(|controller| controller.id == id)
        {
            if previous.profile != profile {
                return Err("Fog replay requires immutable authored controller profiles; Reset after a controller or initial-context change".into());
            }
            return Ok(());
        }
        if self.controllers.len() >= MAX_PLAYER_CONTROLLERS {
            return Err(format!(
                "Player fog catalog exceeds {MAX_PLAYER_CONTROLLERS} controllers; Reset before adding more"
            ));
        }
        self.controllers
            .try_reserve(1)
            .map_err(|_| "Player fog catalog allocation failed")?;
        self.controllers.push(RegisteredController { id, profile });
        Ok(())
    }

    fn can_append(&self, time: f64) -> Result<(), String> {
        self.can_append_with_limit(time, MAX_EVENTS)
    }

    fn can_append_with_limit(&self, time: f64, limit: usize) -> Result<(), String> {
        checked_time(time)?;
        if self.history.len() >= limit {
            return Err(format!(
                "Player fog timeline exceeds {limit} combined inputs and switches; Reset before adding more"
            ));
        }
        if self
            .history
            .last()
            .is_some_and(|previous| time < previous.time)
        {
            return Err("Player fog events must be appended in time order; Reset before editing an earlier time".into());
        }
        Ok(())
    }

    pub fn prepare_switch(&self, time: f64) -> Result<PlayerFrame, String> {
        self.can_append(time)?;
        self.evaluate(time)
    }

    pub fn can_switch(&self, time: f64, controller: u32) -> Result<(), String> {
        self.prepare_switch(time)?.can_switch(controller)
    }

    pub fn can_input(&self, time: f64, controller: u32) -> Result<(), String> {
        self.can_append(time)?;
        self.controller_profile(controller)?;
        Ok(())
    }

    fn controller_profile(&self, controller: u32) -> Result<Profile, String> {
        self.controllers
            .iter()
            .find(|entry| entry.id == controller)
            .map(|entry| entry.profile)
            .ok_or_else(|| {
                format!("Fog controller entity:{controller} is absent from the replay catalog")
            })
    }

    pub fn push_input(&mut self, time: f64, controller: u32, input: Input) -> Result<(), String> {
        self.can_input(time, controller)?;
        input.validate()?;
        self.append(PlayerEvent {
            time,
            controller,
            kind: PlayerEventKind::Input(input),
        })
    }

    pub fn push_switch(&mut self, time: f64, controller: u32) -> Result<(), String> {
        self.can_switch(time, controller)?;
        let profile = self.controller_profile(controller)?;
        let mut candidate = self.clone();
        candidate
            .switches
            .try_reserve(1)
            .map_err(|_| "Player fog switch allocation failed")?;
        candidate.switches.push(PlayerSwitch {
            time,
            controller,
            profile,
        });
        candidate.append(PlayerEvent {
            time,
            controller,
            kind: PlayerEventKind::Switch,
        })?;
        *self = candidate;
        Ok(())
    }

    pub fn push(&mut self, event: PlayerSwitch) -> Result<(), String> {
        let mut candidate = self.clone();
        candidate.register_controller(event.controller, event.profile)?;
        candidate.push_switch(event.time, event.controller)?;
        *self = candidate;
        Ok(())
    }

    fn append(&mut self, event: PlayerEvent) -> Result<(), String> {
        let mut candidate = self.clone();
        candidate
            .history
            .try_reserve(1)
            .map_err(|_| "Player fog history allocation failed")?;
        candidate.history.push(event);
        candidate.evaluate(event.time)?;
        *self = candidate;
        Ok(())
    }

    fn replay(&self, time: f64) -> Result<Replay, String> {
        checked_time(time)?;
        let mut replay = Replay::new(self)?;
        for &event in &self.history {
            if event.time > time {
                break;
            }
            replay.event(event)?;
        }
        replay.advance_ticks(time, true)?;
        let now = checked_time(time)?;
        replay.commit_due(now);
        for controller in &replay.controllers {
            controller.server.raw.evaluated(now)?;
        }
        Ok(replay)
    }

    pub fn evaluate(&self, time: f64) -> Result<PlayerFrame, String> {
        self.replay(time)?.player.frame(time)
    }

    pub fn controller_frame(&self, controller: u32, time: f64) -> Result<ControllerFrame, String> {
        Ok(self.replay(time)?.controller(controller)?.frame())
    }

    pub fn controller_frames(&self, time: f64) -> Result<Vec<(u32, ControllerFrame)>, String> {
        let replay = self.replay(time)?;
        let mut frames = Vec::new();
        frames
            .try_reserve(replay.controllers.len())
            .map_err(|_| "Fog controller frame allocation failed")?;
        for controller in &replay.controllers {
            frames.push((controller.id, controller.frame()));
        }
        Ok(frames)
    }
}
