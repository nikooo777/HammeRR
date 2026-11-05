use map_core::fog::{MAX_EVENTS, PlayerEventKind, RenderPass, RenderPassEvent};
use serde_json::{Value, json};

use super::state::HistoryEvent;
use super::{RenderPassKind, WaterRadialContext, profile_report};
use crate::session::Session;

fn pass_report(order: usize, event: RenderPassEvent, source: &Value) -> Value {
    let (controller, camera) = match event.pass {
        RenderPass::Reset => (None, None),
        RenderPass::World { controller, .. } => (controller, None),
        RenderPass::Sky { camera, .. } => (None, Some(camera)),
    };
    json!({"kind":"record_render_pass","order":order,"time_seconds":event.time_seconds,
        "pass":event.pass.name(),"controller":controller.map(|id|format!("entity:{id}")),
        "camera":camera.map(|id|format!("entity:{id}")),"radial_write":event.pass.radial_write(),"source":source})
}

fn pass_detail(pass: RenderPass, source: &Value) -> String {
    let write = match pass.radial_write() {
        Some(true) => "writes radial distance",
        Some(false) => "writes axial distance",
        None => "preserves the previous radial state",
    };
    match pass {
        RenderPass::Reset => format!("Declared shader default state · {write}"),
        RenderPass::World { controller, .. } => format!(
            "{} · {write}",
            controller.map_or_else(
                || "No world controller".to_owned(),
                |id| format!("entity:{id}")
            )
        ),
        RenderPass::Sky { camera, .. } => format!(
            "entity:{camera} · {} · {write}",
            source["initial_radial_context"]
                .as_str()
                .unwrap_or("unknown context")
        ),
    }
}

impl Session {
    pub(super) fn fog_event_admission(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        if !self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
        {
            return Err("Apply Fog settings or Reset before appending preview events".into());
        }
        if self.fog_preview.worker.busy() {
            return Err(
                "Wait for fog environment preparation before appending preview events".into(),
            );
        }
        if self.fog_preview.events.len() >= MAX_EVENTS {
            return Err("Fog preview reached 128 combined events; Reset before adding more".into());
        }
        if self
            .fog_preview
            .events
            .last()
            .is_some_and(|event| self.fog_preview.time < event.time())
        {
            return Err(
                "Fog events must be appended in time order; Reset before editing an earlier time"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn push_fog_history(&mut self, event: HistoryEvent) -> Result<(), String> {
        self.fog_event_admission()?;
        self.fog_preview
            .events
            .try_reserve(1)
            .map_err(|_| "Fog event history allocation failed")?;
        self.fog_preview.events.push(event);
        Ok(())
    }

    fn capture_fog_render_pass(&self, pass: RenderPassKind) -> Result<(RenderPass, Value), String> {
        self.fog_event_admission()?;
        if self.fog_preview.settings.water_radial_context != WaterRadialContext::RecordedPasses {
            return Err("Choose Recorded pass history before recording a fog setup".into());
        }
        self.fog_preview
            .passes
            .prepare_push(self.fog_preview.time)?;
        match pass {
            RenderPassKind::Reset => Ok((
                RenderPass::Reset,
                json!({"kind":"shader_default_state","radial":false}),
            )),
            RenderPassKind::World => {
                if !self.fog_preview.settings.enabled {
                    return Err("Enable Fog preview before recording World fog".into());
                }
                let selected =
                    self.selected_fog_state(&self.camera(), self.fog_preview.settings)?;
                let radial = selected
                    .profile
                    .filter(|profile| profile.enabled)
                    .map(|profile| profile.radial);
                Ok((
                    RenderPass::World {
                        controller: selected.controller,
                        radial,
                    },
                    json!({
                    "kind":"selected_world_fog","selection":self.fog_preview.settings.selection,
                    "selection_reason":selected.reason,"profile":selected.profile.map(profile_report),
                    "player_sample_time_seconds":selected.player_frame.map(|frame|frame.sample_time_seconds),
                    "source_snapshot":self.fog_preview_snapshot()}),
                ))
            }
            RenderPassKind::Sky => {
                let (camera, profile, source) = self.sky3d_fog_pass_source()?;
                Ok((
                    RenderPass::Sky {
                        camera,
                        radial: profile.enabled.then_some(profile.radial),
                    },
                    source,
                ))
            }
        }
    }

    pub(super) fn append_fog_render_pass(&mut self, pass: RenderPassKind) -> Result<(), String> {
        let (pass, source) = self.capture_fog_render_pass(pass)?;
        let event = RenderPassEvent {
            time_seconds: self.fog_preview.time,
            pass,
        };
        let mut candidate = self.fog_preview.passes.clone();
        candidate.push(event)?;
        self.push_fog_history(HistoryEvent::Pass { event, source })?;
        self.fog_preview.passes = candidate;
        Ok(())
    }

    pub(super) fn fog_events_report(&self) -> Vec<Value> {
        self.fog_preview
            .events
            .iter()
            .enumerate()
            .map(|(order, event)| match event {
                HistoryEvent::Controller(event) => match event.kind {
                    PlayerEventKind::Input(input) => json!({"kind":"input","order":order,
                    "controller":format!("entity:{}",event.controller),"time_seconds":event.time,
                    "input":input.name(),"value":input.value()}),
                    PlayerEventKind::Switch => {
                        json!({"kind":"switch_player_controller","order":order,
                    "controller":format!("entity:{}",event.controller),"time_seconds":event.time})
                    }
                },
                HistoryEvent::Pass { event, source } => pass_report(order, *event, source),
            })
            .collect()
    }

    pub(super) fn fog_pass_events_report(&self) -> Vec<Value> {
        self.fog_preview
            .events
            .iter()
            .enumerate()
            .filter_map(|(order, event)| match event {
                HistoryEvent::Pass { event, source } => Some(pass_report(order, *event, source)),
                HistoryEvent::Controller(_) => None,
            })
            .collect()
    }

    pub(super) fn fog_pass_history_report(&self) -> Value {
        match self.fog_preview.passes.evaluate(self.fog_preview.time) {
            Ok(frame) => {
                let last_writer =
                    self.fog_preview
                        .events
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(order, event)| match event {
                            HistoryEvent::Pass { event, source }
                                if event.time_seconds <= self.fog_preview.time
                                    && event.pass.radial_write().is_some() =>
                            {
                                Some(pass_report(order, *event, source))
                            }
                            _ => None,
                        });
                json!({"radial":frame.radial,"known":frame.radial.is_some(),"last_writer":last_writer,
                    "active_event_count":frame.event_count,"total_event_count":self.fog_preview.passes.events().len(),
                    "origin":"explicit_preview_history"})
            }
            Err(error) => json!({"known":false,"issue":error,"origin":"explicit_preview_history"}),
        }
    }

    pub(super) fn fog_pass_choices(&self) -> Vec<Value> {
        [(RenderPassKind::Reset,"Reset shader state"),(RenderPassKind::World,"World fog"),(RenderPassKind::Sky,"Sky fog")]
            .into_iter().map(|(pass,label)| {
                let admission = self.capture_fog_render_pass(pass);
                let detail = admission.as_ref().ok().map(|(pass,source)|pass_detail(*pass,source));
                json!({"id":pass.name(),"label":label,"ready":admission.is_ok(),"detail":detail,"issue":admission.err()})
            }).collect()
    }
}
