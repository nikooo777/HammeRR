use std::collections::{BTreeMap, BTreeSet};

use map_core::fog::{
    Event, PlayerEvent, PlayerEventKind, PlayerTimeline, Timeline, TouchEvent, TriggerAction,
    TriggerDefinition, TriggerExpansion, TriggerHistoryEvent, admit_trigger_source_catalog,
    expand_trigger_history, parse_trigger_source,
};
use render::fog::ViewFog;
use serde_json::{Value, json};
use source_formats::vmf::Vmf;

use super::TriggerTouchKind;
use super::catalog::Controller;
use super::state::HistoryEvent;
use crate::session::Session;

pub(super) const MAX_GENERATED_ACTIONS: usize = map_core::fog::MAX_TRIGGER_ACTIONS;

pub(super) struct Trigger {
    label: String,
    definition: Result<TriggerDefinition, String>,
}

struct Compiled {
    player: PlayerTimeline,
    expansion: TriggerExpansion,
    timelines: BTreeMap<u32, Timeline>,
}

pub(super) fn capture_catalog(
    vmf: &Vmf,
    controllers: &BTreeMap<u32, Controller>,
) -> Result<BTreeMap<u32, Trigger>, String> {
    admit_trigger_source_catalog(vmf)?;
    let mut result = BTreeMap::new();
    for entity in vmf.entities.iter().filter(|entity| {
        entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("trigger_multiple"))
    }) {
        let definition = parse_trigger_source(entity).and_then(|source| {
            let mut actions = Vec::new();
            for action in source.actions {
                let (&controller,target) = controllers.iter().find(|(_,controller)|controller.switch_target.as_ref().is_ok_and(|name|name == &action.controller_name))
                    .ok_or_else(||format!("Connection {}: Use exact authored targetname of a uniquely qualified fog controller; aliases and unresolved targets are unavailable",action.connection_index))?;
                target.parsed.as_ref().map_err(Clone::clone)?;
                actions.push(TriggerAction {connection_index:action.connection_index,output:action.output,controller,delay:action.delay,times_to_fire:action.times_to_fire});
            }
            TriggerDefinition::new(source.id,actions)
        });
        let name = entity
            .properties
            .get("targetname")
            .filter(|name| !name.is_empty());
        let label = name.map_or_else(
            || format!("entity:{}", entity.id),
            |name| format!("{name} · entity:{}", entity.id),
        );
        if result
            .insert(entity.id, Trigger { label, definition })
            .is_some()
        {
            return Err("Fog trigger catalog has duplicate entity IDs".into());
        }
    }
    Ok(result)
}

impl Session {
    pub(super) fn first_fog_player_operation(&self) -> Option<Value> {
        self.fog_preview.events.iter().enumerate().find_map(|(order,event)| match event {
            HistoryEvent::Controller(event) if matches!(event.kind,PlayerEventKind::Switch) => Some(json!({"order":order,"time_seconds":event.time,"kind":"switch_player_controller","controller":format!("entity:{}",event.controller)})),
            HistoryEvent::Touch(event) => Some(json!({"order":order,"time_seconds":event.time_seconds,"kind":"trigger_touch","trigger":format!("entity:{}",event.trigger)})),
            _ => None,
        })
    }

    pub(super) fn fog_player_active(&self, time: f64) -> bool {
        self.fog_preview
            .events
            .iter()
            .find_map(|event| match event {
                HistoryEvent::Controller(event)
                    if matches!(event.kind, PlayerEventKind::Switch) =>
                {
                    Some(event.time)
                }
                HistoryEvent::Touch(event) => Some(event.time_seconds),
                _ => None,
            })
            .is_none_or(|first| time >= first)
    }

    fn compile_trigger_history(&self, events: &[HistoryEvent]) -> Result<Compiled, String> {
        let initial = self.initial_player_controller()?;
        let catalog = self
            .fog_preview
            .catalog
            .as_ref()
            .ok_or("Capture the fog source catalog before declaring trigger touches")?;
        let triggers = catalog.triggers.as_ref().map_err(Clone::clone)?;
        let mut definitions = BTreeMap::new();
        let mut authored = Vec::new();
        let mut controllers = BTreeSet::from([initial]);
        let mut timelines = BTreeMap::new();
        for event in events {
            match event {
                HistoryEvent::Controller(event) => {
                    controllers.insert(event.controller);
                    if let PlayerEventKind::Input(input) = event.kind {
                        if let std::collections::btree_map::Entry::Vacant(entry) =
                            timelines.entry(event.controller)
                        {
                            entry.insert(Timeline::new(
                                self.initial_fog_profile(event.controller)?,
                            )?);
                        }
                        timelines.get_mut(&event.controller).unwrap().push(Event {
                            time: event.time,
                            input,
                        })?;
                    }
                    authored.push(TriggerHistoryEvent::Controller(*event));
                }
                HistoryEvent::Pass { event, .. } => {
                    authored.push(TriggerHistoryEvent::CapturedPass {
                        time_seconds: event.time_seconds,
                    })
                }
                HistoryEvent::Touch(event) => {
                    let definition = triggers
                        .get(&event.trigger)
                        .ok_or("Trigger source is not in the current authored catalog")?
                        .definition
                        .as_ref()
                        .map_err(Clone::clone)?;
                    definitions.insert(event.trigger, definition.clone());
                    authored.push(TriggerHistoryEvent::Touch(*event));
                }
            }
        }
        for definition in definitions.values() {
            controllers.extend(definition.actions.iter().map(|action| action.controller));
        }
        let definitions = definitions.into_values().collect::<Vec<_>>();
        let expansion = expand_trigger_history(initial, &definitions, &authored)?;
        let profiles = controllers
            .into_iter()
            .map(|id| self.initial_fog_profile(id).map(|profile| (id, profile)))
            .collect::<Result<Vec<_>, _>>()?;
        let player = PlayerTimeline::from_expanded_events(
            initial,
            self.initial_fog_profile(initial)?,
            &profiles,
            &expansion.controller_events,
        )?;
        ViewFog::controller(player.evaluate(self.fog_preview.time)?.profile)?;
        Ok(Compiled {
            player,
            expansion,
            timelines,
        })
    }

    fn append_trigger_event(&mut self, event: HistoryEvent) -> Result<(), String> {
        self.fog_event_admission()?;
        let mut events = self.fog_preview.events.clone();
        events
            .try_reserve(1)
            .map_err(|_| "Fog trigger history allocation failed")?;
        events.push(event);
        let Compiled {
            player,
            expansion,
            timelines,
        } = self.compile_trigger_history(&events)?;
        self.fog_preview.events = events;
        self.fog_preview.player = Some(player);
        self.fog_preview.trigger_expansion = Some(expansion);
        self.fog_preview.timelines = timelines;
        Ok(())
    }

    pub(super) fn append_trigger_controller(&mut self, event: PlayerEvent) -> Result<(), String> {
        self.append_trigger_event(HistoryEvent::Controller(event))
    }

    fn trigger_touch_admission(&self, trigger: u32, touch: TriggerTouchKind) -> Result<(), String> {
        self.fog_event_admission()?;
        if !self.fog_preview.settings.enabled {
            return Err("Enable Fog preview before declaring a trigger touch".into());
        }
        self.initial_player_controller()?;
        let catalog = self
            .fog_preview
            .catalog
            .as_ref()
            .ok_or("Capture the fog source catalog before declaring trigger touches")?;
        let source = catalog
            .triggers
            .as_ref()
            .map_err(Clone::clone)?
            .get(&trigger)
            .ok_or("Choose an authored trigger_multiple from the fog trigger catalog")?;
        source.definition.as_ref().map_err(Clone::clone)?;
        let inside = self
            .fog_preview
            .trigger_expansion
            .as_ref()
            .map(|expansion| expansion.frame(self.fog_preview.time))
            .transpose()?
            .is_some_and(|frame| {
                frame
                    .touches
                    .iter()
                    .any(|state| state.trigger == trigger && state.inside)
            });
        match (touch, inside) {
            (TriggerTouchKind::Enter, true) => {
                Err("The declared virtual player is already inside this trigger".into())
            }
            (TriggerTouchKind::Leave, false) => {
                Err("The declared virtual player is outside this trigger".into())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn append_trigger_touch(
        &mut self,
        trigger: u32,
        touch: TriggerTouchKind,
    ) -> Result<(), String> {
        self.trigger_touch_admission(trigger, touch)?;
        self.append_trigger_event(HistoryEvent::Touch(TouchEvent {
            time_seconds: self.fog_preview.time,
            trigger,
            phase: touch.phase(),
        }))
    }

    pub(super) fn trigger_choices_report(&self) -> Vec<Value> {
        let Some(catalog) = &self.fog_preview.catalog else {
            return Vec::new();
        };
        let Ok(triggers) = &catalog.triggers else {
            return Vec::new();
        };
        let frame = self
            .fog_preview
            .trigger_expansion
            .as_ref()
            .map(|expansion| expansion.frame(self.fog_preview.time))
            .transpose();
        triggers.iter().map(|(&id,source)| {
            let inside = frame.as_ref().ok().and_then(Option::as_ref).is_some_and(|frame|frame.touches.iter().any(|state|state.trigger==id&&state.inside));
            let enter = self.trigger_touch_admission(id,TriggerTouchKind::Enter);
            let leave = self.trigger_touch_admission(id,TriggerTouchKind::Leave);
            let actions = source.definition.as_ref().ok().map(|definition|definition.actions.iter().map(|action|json!({"connection_index":action.connection_index,"output":action.output.name(),"controller":format!("entity:{}",action.controller),"delay_seconds":action.delay,"times_to_fire":action.times_to_fire})).collect::<Vec<_>>());
            let detail = source.definition.as_ref().map(|definition|format!("entity:{id} · {} · {} supported actions · {}",if inside {"Inside"}else{"Outside"},definition.actions.len(),if inside {"Leave records the boundary; queued actions remain scheduled"}else{"Enter records one alive virtual player; no camera or physics contact is inferred"})).unwrap_or_else(Clone::clone);
            let detail = if enter.is_err() && leave.is_err() {
                format!("{detail} · {}",enter.as_ref().err().map(String::as_str).unwrap_or("Touch unavailable"))
            } else {detail};
            json!({"id":format!("entity:{id}"),"label":source.label,"ready":source.definition.is_ok(),"issue":source.definition.as_ref().err(),
                "inside":inside,"can_enter":enter.is_ok(),"enter_issue":enter.err(),"can_leave":leave.is_ok(),"leave_issue":leave.err(),"detail":detail,"actions":actions})
        }).collect()
    }

    fn trigger_action_delay(&self, trigger: u32, connection: usize) -> Option<f32> {
        self.fog_preview
            .catalog
            .as_ref()?
            .triggers
            .as_ref()
            .ok()?
            .get(&trigger)?
            .definition
            .as_ref()
            .ok()?
            .actions
            .iter()
            .find(|action| action.connection_index == connection)
            .map(|action| action.delay)
    }

    pub(super) fn trigger_history_report(&self) -> Value {
        let Some(expansion) = &self.fog_preview.trigger_expansion else {
            return json!({"active":false,"actor":"one_alive_virtual_player","generated_event_count":0,"total_generated_event_count":0,"generated_event_limit":MAX_GENERATED_ACTIONS,"pending_count":0,"fired_count":0,"detail":"One alive virtual player starts outside every trigger; Enter and Leave are explicit declarations."});
        };
        let frame = match expansion.frame(self.fog_preview.time) {
            Ok(frame) => frame,
            Err(error) => return json!({"issue":error}),
        };
        let generated = expansion.generated_actions.iter().filter(|action|action.enqueue_time_seconds<=self.fog_preview.time).map(|action|json!({
            "trigger":format!("entity:{}",action.trigger),"connection_index":action.connection_index,"output":action.output.name(),
            "source_order":action.source_order,"delay_seconds":self.trigger_action_delay(action.trigger,action.connection_index),"enqueue_order":action.enqueue_order,"enqueue_time_seconds":action.enqueue_time_seconds,
            "native_due_time_seconds":action.native_due_time_seconds,"due_time_seconds":action.due_time_seconds,
            "controller":format!("entity:{}",action.controller),"switched":if action.due_time_seconds<=self.fog_preview.time {Some(action.switched)}else{None},
            "delivered":action.due_time_seconds<=self.fog_preview.time,"remaining_after_enqueue":action.remaining_after_enqueue})).collect::<Vec<_>>();
        let last = expansion
            .generated_actions
            .iter()
            .filter(|action| action.due_time_seconds <= self.fog_preview.time)
            .max_by(|a, b| {
                a.due_time_seconds
                    .total_cmp(&b.due_time_seconds)
                    .then(a.enqueue_order.cmp(&b.enqueue_order))
            });
        let detail = last
            .map(|action| {
                format!(
                    "entity:{} · {} at {:.3} s → entity:{} · {}",
                    action.trigger,
                    action.output.name(),
                    action.due_time_seconds,
                    action.controller,
                    if action.switched {
                        "controller switched"
                    } else {
                        "same controller; no change"
                    }
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "Declared trigger history at {:.3} s · {} pending actions",
                    self.fog_preview.time, frame.pending_action_count
                )
            });
        let next = expansion
            .generated_actions
            .iter()
            .filter(|action| {
                action.enqueue_time_seconds <= self.fog_preview.time
                    && action.due_time_seconds > self.fog_preview.time
            })
            .min_by(|a, b| {
                a.due_time_seconds
                    .total_cmp(&b.due_time_seconds)
                    .then(a.enqueue_order.cmp(&b.enqueue_order))
            });
        let detail = if let Some(action) = next {
            let remaining = action
                .remaining_after_enqueue
                .map_or_else(|| "unlimited".to_owned(), |count| count.to_string());
            let delay = self
                .trigger_action_delay(action.trigger, action.connection_index)
                .map(|delay| format!(" · delay {delay:.3} s"))
                .unwrap_or_default();
            format!(
                "{detail} · Next entity:{}/{} → entity:{} at {:.3} s{delay} · remaining {remaining}",
                action.trigger,
                action.output.name(),
                action.controller,
                action.due_time_seconds
            )
        } else {
            detail
        };
        json!({"active":true,"actor":"one_alive_virtual_player","player_operation_active":self.fog_player_active(self.fog_preview.time),"first_player_operation":self.first_fog_player_operation(),"generated_event_count":frame.generated_action_count,"total_generated_event_count":expansion.generated_actions.len(),"generated_event_limit":MAX_GENERATED_ACTIONS,
            "pending_count":frame.pending_action_count,"fired_count":frame.delivered_action_count,"detail":detail,"generated_actions":generated,
            "membership":frame.touches.iter().map(|state|json!({"trigger":format!("entity:{}",state.trigger),"inside":state.inside})).collect::<Vec<_>>(),
            "action_counts":frame.action_counts.iter().map(|count|json!({"trigger":format!("entity:{}",count.trigger),"connection_index":count.connection_index,"remaining":count.remaining})).collect::<Vec<_>>()})
    }
}
