use super::player::can_switch_controller;
use super::timeline::checked_time;
use super::{MAX_EVENTS, MAX_TIMELINE_SECONDS, PlayerEvent, PlayerEventKind};

mod source;

pub use source::{
    MAX_TRIGGER_SOURCE_ACTIONS, MAX_TRIGGER_SOURCE_BYTES, MAX_TRIGGER_SOURCES, SourceAction,
    TriggerOutput, TriggerSource, admit_trigger_source_catalog, parse_trigger_source,
    parse_trigger_sources,
};

pub const MAX_TRIGGER_ACTIONS: usize = MAX_EVENTS;
pub const MAX_ENROLLED_TRIGGERS: usize = MAX_EVENTS;
pub const TRIGGER_HISTORY_POLICY: &str = "Declared Enter/Leave history for one alive virtual player, initially outside every admitted static trigger_multiple. Enter fires OnStartTouch then OnStartTouchAll; Leave fires OnEndTouch then OnEndTouchAll. Each output walks its authored connection rows in reverse order. Actions consume their individual firing count when enqueued, even if a later delivery is a same-controller no-op. Count zero and -1 are unlimited. Native deadlines use float32(current float32 time + delay). A deadline already due at the producer's native time delivers at its logical time, including zero and positive float32-collapsed delays. Later deadlines project to max(producer logical time, converted native deadline), preventing backward delivery. Pending actions deliver before equal-time authored events, FIFO for equal deadlines; newly due actions follow their own producer before the next authored event. Leaving does not cancel queued actions. Changed destinations use the existing raw player timeline; same-controller deliveries preserve player blend/cache timing. Queries and Seek read immutable expansion records without enqueuing, consuming counts or changing membership. Captured render passes keep their append-time values. At most 128 authored events, 128 generated actions, 128 enrolled triggers and 128 enrolled action rows, through 3600 seconds. This declared schedule does not infer physics contacts, viewport-player identity, native server ticks, packets, prediction or arbitrary I/O.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Enter,
    Leave,
}

impl TouchPhase {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "enter" => Ok(Self::Enter),
            "leave" => Ok(Self::Leave),
            _ => Err("Fog trigger phase must be enter or leave".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Leave => "leave",
        }
    }

    fn outputs(self) -> [TriggerOutput; 2] {
        match self {
            Self::Enter => [TriggerOutput::StartTouch, TriggerOutput::StartTouchAll],
            Self::Leave => [TriggerOutput::EndTouch, TriggerOutput::EndTouchAll],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriggerAction {
    pub connection_index: usize,
    pub output: TriggerOutput,
    pub controller: u32,
    pub delay: f32,
    pub times_to_fire: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerDefinition {
    pub id: u32,
    pub actions: Vec<TriggerAction>,
}

impl TriggerDefinition {
    pub fn new(id: u32, mut actions: Vec<TriggerAction>) -> Result<Self, String> {
        for action in &mut actions {
            if action.times_to_fire == 0 {
                action.times_to_fire = -1;
            }
        }
        let definition = Self { id, actions };
        definition.validate()?;
        Ok(definition)
    }

    fn validate(&self) -> Result<(), String> {
        if self.actions.len() > MAX_TRIGGER_ACTIONS {
            return Err(format!(
                "Fog trigger entity:{} exceeds {MAX_TRIGGER_ACTIONS} enrolled action rows",
                self.id
            ));
        }
        if self
            .actions
            .windows(2)
            .any(|rows| rows[0].connection_index >= rows[1].connection_index)
        {
            return Err("Fog trigger actions must retain unique authored connection order".into());
        }
        for action in &self.actions {
            if !action.delay.is_finite()
                || !(0.0..=MAX_TIMELINE_SECONDS as f32).contains(&action.delay)
            {
                return Err(format!(
                    "Fog trigger delay must be finite between 0 and {MAX_TIMELINE_SECONDS} seconds"
                ));
            }
            if action.times_to_fire < -1 {
                return Err("Fog trigger firing count must be -1, zero or positive".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchEvent {
    pub time_seconds: f64,
    pub trigger: u32,
    pub phase: TouchPhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriggerHistoryEvent {
    Controller(PlayerEvent),
    Touch(TouchEvent),
    CapturedPass { time_seconds: f64 },
}

impl TriggerHistoryEvent {
    pub fn time(self) -> f64 {
        match self {
            Self::Controller(event) => event.time,
            Self::Touch(event) => event.time_seconds,
            Self::CapturedPass { time_seconds } => time_seconds,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriggeredAction {
    pub trigger: u32,
    pub connection_index: usize,
    pub output: TriggerOutput,
    pub source_order: usize,
    pub enqueue_order: usize,
    pub enqueue_time_seconds: f64,
    pub native_due_time_seconds: f32,
    pub due_time_seconds: f64,
    pub controller: u32,
    pub switched: bool,
    pub remaining_after_enqueue: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerTouchState {
    pub trigger: u32,
    pub inside: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerActionCount {
    pub trigger: u32,
    pub connection_index: usize,
    pub remaining: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerFrame {
    pub touches: Vec<TriggerTouchState>,
    pub action_counts: Vec<TriggerActionCount>,
    pub generated_action_count: usize,
    pub delivered_action_count: usize,
    pub pending_action_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerExpansion {
    pub controller_events: Vec<PlayerEvent>,
    pub generated_actions: Vec<TriggeredAction>,
    pub touches: Vec<TouchEvent>,
    pub first_player_operation: Option<f64>,
    initial_touches: Vec<TriggerTouchState>,
    initial_counts: Vec<TriggerActionCount>,
}

impl TriggerExpansion {
    pub fn frame(&self, time_seconds: f64) -> Result<TriggerFrame, String> {
        checked_time(time_seconds)?;
        let mut touches = self.initial_touches.clone();
        for event in &self.touches {
            if event.time_seconds > time_seconds {
                break;
            }
            let state = touches
                .iter_mut()
                .find(|state| state.trigger == event.trigger)
                .ok_or("Fog trigger membership journal refers to an unenrolled trigger")?;
            state.inside = event.phase == TouchPhase::Enter;
        }
        let mut action_counts = self.initial_counts.clone();
        for count in &mut action_counts {
            if let Some(action) = self
                .generated_actions
                .iter()
                .filter(|action| {
                    action.trigger == count.trigger
                        && action.connection_index == count.connection_index
                        && action.enqueue_time_seconds <= time_seconds
                })
                .max_by_key(|action| action.enqueue_order)
            {
                count.remaining = action.remaining_after_enqueue;
            }
        }
        let generated_action_count = self
            .generated_actions
            .iter()
            .filter(|action| action.enqueue_time_seconds <= time_seconds)
            .count();
        let delivered_action_count = self
            .generated_actions
            .iter()
            .filter(|action| action.due_time_seconds <= time_seconds)
            .count();
        Ok(TriggerFrame {
            touches,
            action_counts,
            generated_action_count,
            delivered_action_count,
            pending_action_count: generated_action_count - delivered_action_count,
        })
    }
}

struct ExpansionState {
    expansion: TriggerExpansion,
    memberships: Vec<TriggerTouchState>,
    counts: Vec<TriggerActionCount>,
    pending: Vec<TriggeredAction>,
    current_controller: u32,
}

impl ExpansionState {
    fn new(initial_controller: u32, triggers: &[TriggerDefinition]) -> Result<Self, String> {
        if triggers.len() > MAX_ENROLLED_TRIGGERS {
            return Err(format!(
                "Fog trigger history exceeds {MAX_ENROLLED_TRIGGERS} enrolled triggers"
            ));
        }
        let mut memberships = Vec::new();
        memberships
            .try_reserve(triggers.len())
            .map_err(|_| "Fog trigger membership allocation failed")?;
        let mut counts = Vec::new();
        counts
            .try_reserve(MAX_TRIGGER_ACTIONS)
            .map_err(|_| "Fog trigger action-count allocation failed")?;
        for trigger in triggers {
            trigger.validate()?;
            if memberships
                .iter()
                .any(|state: &TriggerTouchState| state.trigger == trigger.id)
            {
                return Err("Fog trigger history has duplicate enrolled trigger IDs".into());
            }
            if counts.len() + trigger.actions.len() > MAX_TRIGGER_ACTIONS {
                return Err(format!(
                    "Fog trigger history exceeds {MAX_TRIGGER_ACTIONS} enrolled action rows"
                ));
            }
            memberships.push(TriggerTouchState {
                trigger: trigger.id,
                inside: false,
            });
            for action in &trigger.actions {
                counts.push(TriggerActionCount {
                    trigger: trigger.id,
                    connection_index: action.connection_index,
                    remaining: if action.times_to_fire <= 0 {
                        None
                    } else {
                        Some(action.times_to_fire as u32)
                    },
                });
            }
        }
        let mut controller_events = Vec::new();
        controller_events
            .try_reserve(MAX_EVENTS + MAX_TRIGGER_ACTIONS)
            .map_err(|_| "Expanded fog event allocation failed")?;
        let mut generated_actions = Vec::new();
        generated_actions
            .try_reserve(MAX_TRIGGER_ACTIONS)
            .map_err(|_| "Generated fog action allocation failed")?;
        let mut pending = Vec::new();
        pending
            .try_reserve(MAX_TRIGGER_ACTIONS)
            .map_err(|_| "Pending fog action allocation failed")?;
        let mut touches = Vec::new();
        touches
            .try_reserve(MAX_EVENTS)
            .map_err(|_| "Fog trigger touch allocation failed")?;
        Ok(Self {
            expansion: TriggerExpansion {
                controller_events,
                generated_actions,
                touches,
                first_player_operation: None,
                initial_touches: memberships.clone(),
                initial_counts: counts.clone(),
            },
            memberships,
            counts,
            pending,
            current_controller: initial_controller,
        })
    }

    fn drain(&mut self, time_seconds: f64) {
        while self
            .pending
            .first()
            .is_some_and(|action| action.due_time_seconds <= time_seconds)
        {
            let mut action = self.pending.remove(0);
            action.switched = action.controller != self.current_controller;
            if action.switched {
                self.expansion.controller_events.push(PlayerEvent {
                    time: action.due_time_seconds,
                    controller: action.controller,
                    kind: PlayerEventKind::Switch,
                });
                self.current_controller = action.controller;
            }
            self.expansion.generated_actions.push(action);
        }
    }

    fn touch(
        &mut self,
        event: TouchEvent,
        source_order: usize,
        triggers: &[TriggerDefinition],
    ) -> Result<(), String> {
        let trigger = triggers
            .iter()
            .find(|trigger| trigger.id == event.trigger)
            .ok_or_else(|| format!("Fog trigger entity:{} is not enrolled", event.trigger))?;
        let membership = self
            .memberships
            .iter_mut()
            .find(|state| state.trigger == event.trigger)
            .ok_or("Enrolled fog trigger has no membership state")?;
        let entering = event.phase == TouchPhase::Enter;
        if membership.inside == entering {
            return Err(if entering {
                "The virtual player is already inside this fog trigger".into()
            } else {
                "The virtual player is outside this fog trigger; Enter before Leave".into()
            });
        }
        membership.inside = entering;
        self.expansion.touches.push(event);
        self.expansion
            .first_player_operation
            .get_or_insert(event.time_seconds);
        for output in event.phase.outputs() {
            for action in trigger
                .actions
                .iter()
                .rev()
                .filter(|action| action.output == output)
            {
                self.enqueue(event, source_order, *action)?;
            }
        }
        self.drain(event.time_seconds);
        Ok(())
    }

    fn enqueue(
        &mut self,
        event: TouchEvent,
        source_order: usize,
        action: TriggerAction,
    ) -> Result<(), String> {
        let count = self
            .counts
            .iter_mut()
            .find(|count| {
                count.trigger == event.trigger && count.connection_index == action.connection_index
            })
            .ok_or("Enrolled fog trigger action has no firing-count state")?;
        if count.remaining == Some(0) {
            return Ok(());
        }
        let enqueue_order = self.expansion.generated_actions.len() + self.pending.len();
        if enqueue_order >= MAX_TRIGGER_ACTIONS {
            return Err(format!(
                "Fog trigger history exceeds {MAX_TRIGGER_ACTIONS} generated actions"
            ));
        }
        let native_now = checked_time(event.time_seconds)?;
        let native_due_time_seconds = native_now + action.delay;
        if !native_due_time_seconds.is_finite() {
            return Err("Fog trigger action deadline is not finite".into());
        }
        let due_time_seconds = if native_due_time_seconds <= native_now {
            event.time_seconds
        } else {
            event.time_seconds.max(f64::from(native_due_time_seconds))
        };
        checked_time(due_time_seconds)?;
        if let Some(remaining) = &mut count.remaining {
            *remaining -= 1;
        }
        let generated = TriggeredAction {
            trigger: event.trigger,
            connection_index: action.connection_index,
            output: action.output,
            source_order,
            enqueue_order,
            enqueue_time_seconds: event.time_seconds,
            native_due_time_seconds,
            due_time_seconds,
            controller: action.controller,
            switched: false,
            remaining_after_enqueue: count.remaining,
        };
        let position = self
            .pending
            .iter()
            .position(|queued| queued.due_time_seconds > due_time_seconds)
            .unwrap_or(self.pending.len());
        self.pending.insert(position, generated);
        Ok(())
    }
}

pub fn expand_trigger_history(
    initial_controller: u32,
    triggers: &[TriggerDefinition],
    authored: &[TriggerHistoryEvent],
) -> Result<TriggerExpansion, String> {
    if authored.len() > MAX_EVENTS {
        return Err(format!(
            "Fog trigger history exceeds {MAX_EVENTS} authored events"
        ));
    }
    let mut state = ExpansionState::new(initial_controller, triggers)?;
    let mut previous_time = None;
    for (source_order, &event) in authored.iter().enumerate() {
        let time = event.time();
        checked_time(time)?;
        if previous_time.is_some_and(|previous| time < previous) {
            return Err(
                "Fog trigger authored events must retain chronological append order".into(),
            );
        }
        previous_time = Some(time);
        state.drain(time);
        match event {
            TriggerHistoryEvent::Controller(event) => {
                match event.kind {
                    PlayerEventKind::Input(input) => input.validate()?,
                    PlayerEventKind::Switch => {
                        can_switch_controller(state.current_controller, event.controller)?;
                        state.current_controller = event.controller;
                        state
                            .expansion
                            .first_player_operation
                            .get_or_insert(event.time);
                    }
                }
                state.expansion.controller_events.push(event);
            }
            TriggerHistoryEvent::Touch(event) => state.touch(event, source_order, triggers)?,
            TriggerHistoryEvent::CapturedPass { .. } => {}
        }
    }
    state.drain(MAX_TIMELINE_SECONDS);
    if !state.pending.is_empty() {
        return Err("Fog trigger expansion retained an action outside its time bound".into());
    }
    Ok(state.expansion)
}
