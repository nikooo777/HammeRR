use map_core::fog::{CSS_OFFICIAL_MAP_CATALOG, INITIAL_RADIAL_CONTEXTS, Profile, Timeline};
use render::fog::{ViewFog, WaterFog};
use serde_json::{Value, json};

use super::Session;

mod catalog;
mod controls;
pub(super) mod environment;
mod preparation;
mod state;

pub(crate) use controls::Update;
pub use map_core::fog::InitialRadialContext;
pub(crate) use preparation::RenderGuard;
pub use preparation::{Completed, Request};
pub(crate) use state::ClockLease;
pub(super) use state::State;
pub use state::{Channel, Clock, Command, FogView, Overrides, SelectionMode, Settings};

fn profile_report(profile: Profile) -> Value {
    json!({"color":profile.color,"color2":profile.color2,"direction":profile.direction,
        "start":profile.start,"end":profile.end,"max_density":profile.max_density,
        "enabled":profile.enabled,"radial":profile.radial,"blend":profile.blend,
        "far_z":profile.far_z,"lerp_time":profile.lerp_time})
}

fn actor_name(actor: super::Actor) -> &'static str {
    match actor {
        super::Actor::User => "user",
        super::Actor::Agent => "agent",
        super::Actor::System => "system",
    }
}

impl Session {
    pub fn fog_preview_snapshot(&self) -> String {
        state::Key::capture(self).snapshot(&self.fog_preview.scope, self.fog_preview.control_serial)
    }

    pub fn fog_preview_settings(&self) -> Settings {
        self.fog_preview.settings
    }

    fn fog_settings_for(&self, overrides: Overrides) -> Result<Settings, String> {
        if overrides.controller.is_some() && overrides.selection == Some(SelectionMode::Environment)
        {
            return Err(
                "A temporary fog controller cannot be combined with environment selection".into(),
            );
        }
        Ok(Settings {
            enabled: overrides
                .enabled
                .unwrap_or(self.fog_preview.settings.enabled),
            controller: overrides
                .controller
                .or(self.fog_preview.settings.controller),
            selection: overrides
                .selection
                .unwrap_or(if overrides.controller.is_some() {
                    SelectionMode::Explicit
                } else {
                    self.fog_preview.settings.selection
                }),
            channel: overrides
                .channel
                .unwrap_or(self.fog_preview.settings.channel),
            initial_radial_context: self.fog_preview.settings.initial_radial_context,
        })
    }

    fn initial_fog_profile(&self, id: u32) -> Result<Profile, String> {
        if self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
            && let Some(timeline) = self.fog_preview.timelines.get(&id)
        {
            return Ok(timeline.initial());
        }
        let parsed = if self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
            && let Some(catalog) = &self.fog_preview.catalog
        {
            catalog
                .controllers
                .get(&id)
                .ok_or_else(|| format!("entity:{id} is not in the current fog catalog"))?
                .parsed
                .as_ref()
                .map(|parsed| parsed.profile)
                .map_err(Clone::clone)?
        } else {
            let entity = self
                .render_document()
                .entity(id)
                .ok_or_else(|| format!("entity:{id} no longer exists; choose a fog controller"))?;
            catalog::parse(self.render_document().vmf(), entity)?.profile
        };
        Ok(self
            .fog_preview
            .settings
            .initial_radial_context
            .initial_profile(parsed))
    }

    fn fog_profile(&self, id: u32, time: f64) -> Result<Profile, String> {
        if self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
            && let Some(timeline) = self.fog_preview.timelines.get(&id)
        {
            return timeline.evaluate(time);
        }
        self.initial_fog_profile(id)
    }

    pub fn fog_render_state_for_view(
        &self,
        camera: &render::Camera,
        surface: render::SurfaceView,
        overrides: Overrides,
    ) -> Result<FogView, String> {
        let settings = self.fog_settings_for(overrides)?;
        if !settings.enabled {
            return Ok(FogView {
                fog: None,
                report: json!({"enabled":false,"applied":false,"channel":settings.channel,"selection":settings.selection,"initial_radial_context":settings.initial_radial_context.name()}),
            });
        }
        settings.channel.validate()?;
        if self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| !key.current(self))
        {
            return Err(
                "Fog source changed; apply preview settings or Reset before rendering".into(),
            );
        }
        if let Some(error) = &self.fog_preview.error {
            return Err(error.clone());
        }
        let (id, reason, water) =
            match settings.selection {
                SelectionMode::Explicit => (
                    Some(settings.controller.ok_or(
                        "Choose an existing env_fog_controller; no controller is selected",
                    )?),
                    "explicit",
                    None,
                ),
                SelectionMode::Environment => {
                    let prepared = self.fog_preview.environment.as_ref().ok_or(
                        "Prepare the fog environment before rendering automatic selection",
                    )?;
                    let selected = prepared.snapshot.select(camera.position.as_dvec3())?;
                    (selected.controller, selected.reason, selected.underwater)
                }
            };
        let profile = id
            .map(|id| self.fog_profile(id, self.fog_preview.time))
            .transpose()?;
        let initial_radial = id
            .map(|id| self.initial_fog_profile(id).map(|profile| profile.radial))
            .transpose()?;
        let water_report=water.as_ref().map(|water|json!({"solid":format!("solid:{}",water.solid),"contents":water.contents,
            "source_solids":water.source_solids,"material":water.material,"surface_material":water.surface_material,"surface_z":water.surface_z,"color_normalized":water.color_normalized,"start":water.start,"end":water.end,"enabled":water.enabled}));
        let fog = if let Some(water) = water {
            let water = WaterFog::new(
                water.color_normalized,
                water.start,
                water.end,
                water.enabled,
            )?;
            Some(ViewFog::water(profile, water)?)
        } else {
            profile.map(ViewFog::controller).transpose()?
        };
        let far_z = render::fog::far_z(fog, camera);
        let color = render::fog::for_color(fog, camera, surface);
        Ok(FogView {
            fog,
            report: json!({"enabled":true,"ready":true,"applied":color.is_some_and(|fog|fog.enabled() && fog.max_density()>0.0),
            "controller":id.map(|id|format!("entity:{id}")),"selection":settings.selection,"selection_reason":reason,"channel":settings.channel,
            "initial_radial_context":settings.initial_radial_context.name(),"initial_radial":initial_radial,
            "time_seconds":self.fog_preview.time,"profile":profile.map(profile_report),"water":water_report,"far_z":far_z,
            "color_linear":color.map(|fog|fog.color_for_camera(camera)),"preview_only":true}),
        })
    }

    pub fn fog_preview_for_render(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<Option<ViewFog>, String> {
        self.fog_render_state_for_view(
            &self.camera(),
            self.view.surface,
            Overrides {
                enabled,
                controller,
                ..Overrides::default()
            },
        )
        .map(|view| view.fog)
    }

    pub fn fog_render_state(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<(Option<ViewFog>, Value), String> {
        self.fog_render_state_for_view(
            &self.camera(),
            self.view.surface,
            Overrides {
                enabled,
                controller,
                ..Overrides::default()
            },
        )
        .map(|view| (view.fog, view.report))
    }

    pub fn fog_preview_clock(&self) -> Clock {
        let stale = self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| !key.current(self));
        let busy = self.fog_preview.worker.busy();
        let status = if stale {
            "stale"
        } else if busy {
            "preparing_environment"
        } else if self.fog_preview.error.is_some() {
            "failed"
        } else if self.fog_preview.playing {
            "playing"
        } else if !self.fog_preview.settings.enabled {
            "disabled"
        } else {
            "paused"
        };
        Clock {
            snapshot: self.fog_preview_snapshot(),
            serial: self.fog_preview.serial,
            frame_serial: self.fog_preview.frame_serial,
            status,
            playing: self.fog_preview.playing && !stale,
            time_seconds: self.fog_preview.time,
            requested_time_seconds: self.fog_preview.time,
            busy,
            presentation_active: self.fog_preview.presentation_active,
        }
    }

    pub fn fog_preview_summary(&self) -> String {
        let clock = self.fog_preview_clock();
        if clock.status == "stale" {
            return "Fog source changed; apply settings or Reset".into();
        }
        if let Some(error) = &self.fog_preview.error {
            return error.clone();
        }
        if !self.fog_preview.settings.enabled {
            return "Fog preview is off".into();
        }
        if self.fog_preview.settings.selection == SelectionMode::Environment
            && self.fog_preview.environment.is_none()
        {
            return "Prepare the fog environment".into();
        }
        let status = match clock.status {
            "preparing_environment" => "preparing environment",
            other => other,
        };
        format!(
            "Fog {} · {} · {:.3} s",
            match self.fog_preview.settings.channel {
                Channel::Ldr => "LDR",
                Channel::Hdr => "HDR",
            },
            status,
            clock.time_seconds
        )
    }

    pub fn fog_preview_state(&self) -> Value {
        let clock = self.fog_preview_clock();
        let query =
            self.fog_render_state_for_view(&self.camera(), self.view.surface, Overrides::default());
        let current = query.as_ref().ok().map(|view| &view.report);
        let issue = query
            .as_ref()
            .err()
            .cloned()
            .or_else(|| self.fog_preview.error.clone());
        let captured;
        let catalog = if self
            .fog_preview
            .key
            .as_ref()
            .is_some_and(|key| key.current(self))
        {
            self.fog_preview
                .catalog
                .as_ref()
                .ok_or_else(|| "Fog catalog has not been captured".to_owned())
        } else {
            captured = catalog::Catalog::capture(self.render_document().vmf());
            captured.as_ref().map_err(Clone::clone)
        };
        let controllers=catalog.as_ref().map(|catalog|catalog.controllers.iter().map(|(&id,controller)| {
            json!({"id":format!("entity:{id}"),"label":controller.targetname.as_deref().filter(|name|!name.is_empty()).map_or_else(||format!("entity:{id}"),|name|format!("{name} · entity:{id}")),
                "targetname":controller.targetname,"ready":controller.parsed.is_ok(),"settings":controller.parsed.as_ref().ok().map(|parsed|profile_report(parsed.profile)),
                "initial_settings":controller.parsed.as_ref().ok().map(|parsed|profile_report(self.fog_preview.settings.initial_radial_context.initial_profile(parsed.profile))),
                "issue":controller.parsed.as_ref().err(),"detail":controller.parsed.as_ref().err(),
                "defaults":controller.parsed.as_ref().ok().map(|parsed|&parsed.defaults),"inert_keys":controller.parsed.as_ref().ok().map(|parsed|&parsed.inert_keys),
                "parent_chain":controller.parsed.as_ref().ok().map(|parsed|parsed.parent_chain.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>())})
        }).collect::<Vec<_>>()).unwrap_or_default();
        let inputs=map_core::fog::input_choices().iter().map(|choice|json!({"id":choice.name,"name":choice.name,"label":choice.name,"kind":choice.kind,"help":choice.help,"has_value":choice.has_value})).collect::<Vec<_>>();
        let events=self.fog_preview.timelines.iter().flat_map(|(&id,timeline)|timeline.events().iter().map(move |event|json!({"controller":format!("entity:{id}"),"time_seconds":event.time,"input":event.input.name(),"value":event.input.value()}))).collect::<Vec<_>>();
        let initial_radial_contexts=INITIAL_RADIAL_CONTEXTS.map(|context|json!({"id":context.name(),"label":context.label(),"ready":true,"detail":context.detail()}));
        let detail = if let Some(error) = &issue {
            error.clone()
        } else if let Some(current) = current {
            format!(
                "Controller {} · selection {} · channel {} · time {:.3} s · {}",
                current["controller"].as_str().unwrap_or("none"),
                self.fog_preview.settings.selection.name(),
                self.fog_preview.settings.channel.name(),
                self.fog_preview.time,
                current["profile"]
            )
        } else {
            "Fog unavailable".into()
        };
        json!({"snapshot":clock.snapshot,"serial":clock.serial,"frame_serial":clock.frame_serial,"status":clock.status,"playback_status":clock.status,
            "enabled":self.fog_preview.settings.enabled,"configured_controller":self.fog_preview.settings.controller.map(|id|format!("entity:{id}")),
            "controller":current.and_then(|current|current.get("controller")),"selection":self.fog_preview.settings.selection,"channel":self.fog_preview.settings.channel,
            "initial_radial_context":self.fog_preview.settings.initial_radial_context.name(),"initial_radial_contexts":initial_radial_contexts,
            "initial_radial_context_source":"explicit_preview_setting","css_official_map_catalog":CSS_OFFICIAL_MAP_CATALOG,
            "settings":self.fog_preview.settings,"ready":query.is_ok()&&!clock.busy,"active":current.is_some_and(|current|current["applied"]==true),
            "summary":self.fog_preview_summary(),"detail":detail,"issue":issue,"error":issue,"catalog_issue":catalog.err(),
            "controllers":controllers,"channels":[{"id":"ldr","label":"LDR","ready":true,"detail":"World fog · LDR"},{"id":"hdr","label":"HDR","ready":false,"detail":"HDR fog display is unavailable"}],
            "selections":[{"id":"explicit","label":"Explicit controller","ready":true,"detail":"Use the selected controller's authored or replayed values"},{"id":"environment","label":"Environment","ready":true,"detail":"Use the map's initial controller and water fog. Runtime controller switches are unavailable."}],
            "inputs":inputs,"events":events,"current":current,"requested":current,"time_seconds":clock.time_seconds,"current_time_seconds":clock.time_seconds,
            "requested_time_seconds":clock.requested_time_seconds,"max_time_seconds":map_core::fog::MAX_TIMELINE_SECONDS,"playing":clock.playing,"busy":clock.busy,
            "presentation_active":clock.presentation_active,"last_actor":self.fog_preview.last_actor.map(actor_name),"worker_actor":self.fog_preview.pending.as_ref().map(|pending|actor_name(pending.actor)),
            "can_play":query.is_ok()&&self.fog_preview.settings.enabled&&!clock.busy&&clock.status!="stale"&&clock.time_seconds<map_core::fog::MAX_TIMELINE_SECONDS,
            "can_seek":self.fog_preview.key.as_ref().is_some_and(|key|key.current(self))&&!clock.busy,"can_prepare_environment":!clock.busy,
            "environment_ready":self.fog_preview.environment.is_some()&&clock.status!="stale","last_source_check_age_seconds":self.fog_preview.verified.map(|time|time.elapsed().as_secs_f64()),
            "environment":self.fog_preview.environment.as_ref().filter(|_|clock.status!="stale").map(|environment|json!({"controllers":environment.snapshot.controllers(),"liquids":environment.snapshot.liquid_count(),"solids":environment.snapshot.solid_count(),"tree_nodes":environment.snapshot.tree_nodes()})),
            "transitioning":self.fog_preview.timelines.values().any(|timeline|timeline.events().iter().any(|event|matches!(event.input,map_core::fog::Input::StartFogTransition))),
            "limits":{"controllers":catalog::MAX_CONTROLLERS,"entities":catalog::MAX_ENTITIES,"controller_property_bytes":catalog::MAX_CONTROLLER_BYTES,"events":map_core::fog::MAX_EVENTS,"time_seconds":map_core::fog::MAX_TIMELINE_SECONDS,"advance_seconds":state::MAX_ADVANCE_SECONDS,"source_snapshot_bytes":state::MAX_SOURCE_BYTES,"workers":1},
            "timeline_policy":map_core::fog::TIMELINE_POLICY,
            "initial_radial_context_policy":"The selected rule applies to every controller's initial profile before preview inputs. Changing it clears inputs, resets time and pauses, while retaining a current prepared environment. Later SetRadial inputs override the initial value. No running map name or file path is inferred.",
            "playback_policy":"One shared runtime clock drives Play in desktop and headless sessions. Hidden or inactive presentation suspends wall time without catch-up. Seek pauses; explicit Advance remains available. Reset clears preview inputs and restores the current-context initial profile at time zero. Cancel stops environment preparation and playback; Clear also disables and clears preview state.",
            "selection_policy":"Manual controller selection snaps. Automatic mode uses initial first-controller/last-Master selection and current-camera prepared water context. Trigger history, player controller-switch blends, fog volumes, runtime I/O and native scheduler/network timing are not simulated.",
            "persistence":"Session view state only; no VMF edits, Undo steps or saved controller mutation. HDRColorScale and ZoomFogScale remain inert foreign metadata in this installed CSS path."})
    }

    pub fn fog_preview_report(&self, enabled: bool, controller: Option<u32>) -> Value {
        match self.fog_render_state_for_view(
            &self.camera(),
            self.view.surface,
            Overrides {
                enabled: Some(enabled),
                controller,
                ..Overrides::default()
            },
        ) {
            Ok(view) => view.report,
            Err(error) => json!({"enabled":enabled,"ready":false,"issue":error,"summary":error}),
        }
    }

    fn validate_fog_timelines(
        timelines: &std::collections::BTreeMap<u32, Timeline>,
        time: f64,
    ) -> Result<(), String> {
        for (&id, timeline) in timelines {
            let profile = timeline
                .evaluate(time)
                .map_err(|error| format!("entity:{id}: {error}"))?;
            ViewFog::controller(profile)?;
        }
        Ok(())
    }
}
