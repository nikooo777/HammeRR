use super::super::*;
use crate::session::fog::{Command, InitialRadialContext, Settings};

impl FogPreviewControlArgs {
    fn command(self, current: Settings) -> Result<(String, Command, bool), String> {
        let settings = self.settings.is_some();
        let controller = self.controller.is_some();
        let input = self.input.is_some() || self.value.is_some();
        let time = self.time_seconds.is_some();
        let advance = self.seconds.is_some();
        if self.pass.is_some() && !matches!(self.action, FogPreviewAction::RecordRenderPass) {
            return Err("pass is only accepted by Record render pass".into());
        }
        let command = match self.action {
            FogPreviewAction::Settings => {
                if controller || input || time || advance || self.background.is_some() {
                    return Err("Settings accepts only a complete settings object".into());
                }
                let settings = self.settings.ok_or("Settings requires a settings object")?;
                Command::Settings(Settings {
                    enabled: settings.enabled,
                    controller: fog_controller_id(settings.controller.as_deref())?,
                    selection: settings.selection,
                    channel: settings.channel,
                    initial_radial_context: settings
                        .initial_radial_context
                        .as_deref()
                        .map(InitialRadialContext::parse)
                        .transpose()?
                        .unwrap_or(current.initial_radial_context),
                    water_radial_context: settings
                        .water_radial_context
                        .unwrap_or(current.water_radial_context),
                })
            }
            FogPreviewAction::Input => {
                if settings || time || advance {
                    return Err(
                        "Input accepts controller, input, value and optional background".into(),
                    );
                }
                let controller = fog_controller_id(self.controller.as_deref())?
                    .ok_or("Input requires an env_fog_controller entity:<id>")?;
                let name = self
                    .input
                    .as_deref()
                    .ok_or("Input requires a native input name")?;
                Command::Input {
                    controller,
                    input: map_core::fog::Input::parse(name, self.value.as_deref())?,
                }
            }
            FogPreviewAction::SwitchPlayerController => {
                if settings || input || time || advance {
                    return Err(
                        "Switch player controller accepts only controller and optional background"
                            .into(),
                    );
                }
                Command::SwitchPlayerController {
                    controller: fog_controller_id(self.controller.as_deref())?
                        .ok_or("Switch player controller requires a destination env_fog_controller entity:<id>")?,
                }
            }
            FogPreviewAction::PrepareEnvironment => {
                if settings || controller || input || time || advance {
                    return Err("Prepare environment accepts only optional background".into());
                }
                Command::PrepareEnvironment
            }
            FogPreviewAction::RecordRenderPass => {
                if settings || controller || input || time || advance || self.background.is_some() {
                    return Err("Record render pass accepts only pass".into());
                }
                Command::RecordRenderPass {
                    pass: self.pass.ok_or("Record render pass requires pass")?,
                }
            }
            FogPreviewAction::Seek => {
                if settings || controller || input || advance {
                    return Err("Seek accepts only time_seconds and optional background".into());
                }
                Command::Seek {
                    time_seconds: self.time_seconds.ok_or("Seek requires time_seconds")?,
                }
            }
            FogPreviewAction::Advance => {
                if settings || controller || input || time {
                    return Err("Advance accepts only seconds and optional background".into());
                }
                Command::Advance {
                    seconds: self.seconds.ok_or("Advance requires seconds")?,
                }
            }
            FogPreviewAction::Reset => {
                if settings || controller || input || time || advance {
                    return Err("Reset accepts only optional background".into());
                }
                Command::Reset
            }
            FogPreviewAction::Play
            | FogPreviewAction::Pause
            | FogPreviewAction::Cancel
            | FogPreviewAction::Clear => {
                if settings || controller || input || time || advance || self.background.is_some() {
                    return Err("Play, Pause, Cancel and Clear accept no additional fields".into());
                }
                match self.action {
                    FogPreviewAction::Play => Command::Play,
                    FogPreviewAction::Pause => Command::Pause,
                    FogPreviewAction::Cancel => Command::Cancel,
                    _ => Command::Clear,
                }
            }
        };
        Ok((
            self.expected_snapshot,
            command,
            self.background.unwrap_or(false),
        ))
    }
}

#[tool_router(router = fog_preview_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared Fog preview, per-destination input readiness, ordered mixed events, separate server/received/player raw fog fields and caches, sampled player time, render query time, declared delivery policy, initial context, named switch targets, effective settings, recorded setup choices/history and diagnostics. Returns the current opaque snapshot for reviewed preview operations. Preview controls do not edit VMF properties or Undo history.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_fog_preview(&self) -> ToolResult {
        run(|| self.document_access().inspect_fog_preview())
    }

    #[tool(
        description = "Apply the compatibility Fog preview operation through the shared Session. Pass enabled, an explicit env_fog_controller entity:<id> when enabling, and the latest get_fog_preview.snapshot. Preserves the current initial radial and water radial contexts. Invalid controllers and stale snapshots refuse without changing settings. Disabling remains available with a current snapshot. Preview controls preserve the map and Undo history."
    )]
    pub(in crate::mcp) fn set_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let controller = fog_controller_id(args.controller.as_deref())?;
            let current = self.session().fog_preview_settings();
            self.document_access().fog_preview(
                Actor::Agent,
                &args.snapshot,
                Command::Settings(Settings {
                    enabled: args.enabled,
                    controller,
                    selection: crate::session::fog::SelectionMode::Explicit,
                    channel: current.channel,
                    initial_radial_context: current.initial_radial_context,
                    water_radial_context: current.water_radial_context,
                }),
                false,
            )
        })
    }

    #[tool(
        description = "Control the shared Fog preview through the same Settings, Prepare environment, native controller inputs, player-controller switches and transport operations as the desktop. Inspect get_fog_preview first and pass its current expected_snapshot. Settings requires a complete settings object with enabled, controller, selection and channel; optional water_radial_context accepts controller_preview or recorded_passes and omission preserves the current mode. Changing that mode pauses, clears all controller/player/pass events and resets time while retaining a current prepared environment; null controller clears the explicit choice. Optional initial_radial_context accepts authored or css_official_map; omission preserves the current context. It initializes controllers before event replay; later SetRadial inputs override it. Changing controller, selection or initial context clears preview events, rewinds time and pauses, retaining a current prepared environment; visibility-only settings retain the sequence. LDR is qualified; HDR requests refuse explicitly. The switch_player_controller action targets the player, using a named destination env_fog_controller entity:<id> from switch_targets in controller and optional background. It accepts no input/value/settings/time fields. Controller inputs, player switches and recorded render passes share one append-ordered history capped at 128 total events. The record_render_pass action accepts only pass reset/world/sky and expected_snapshot, without background. It captures immutable values at the current logical time in recorded_passes mode. Reset writes the declared shader default false; World samples the selected current controller/player before water composition; Sky samples the explicitly selected qualified sky_camera under its current initial radial context. Enabled setups write radial even at zero density; absent or disabled setups preserve it. No render query records a setup. In recorded mode the initial radial state is Unknown, and water refuses until an actual writer establishes it; a known state admits water with no controller without inventing a profile. Inspect pass_choices, pass_events and pass_history. Seeking behind any future event prevents appending inputs, switches or passes until Reset or returning to the latest time. Input requires its explicit env_fog_controller entity:<id> from input_targets, its exact native input name and raw parameter value when needed; it never retargets to the configured or current player controller. Direct controller inputs do not require targetnames. Input-only preview retains continuous controller evaluation. The first player switch replays all original inputs from the settled initial epoch under the mixed raw-state policy. Due server Think commits precede equal-time events. Each controller input or Think immediately delivers full controller raw fields without a player data update. Each switch is a separate player data delivery in append order, caches the last written player primary RGB/start/end without advancing the old blend, copies destination received raw fields, and immediately updates the new blend. After equal-time events each k/50-second tick performs an ordinary player data delivery and one Simulate update; prediction is absent. Active player blending freezes the copied raw fields and duration. Only primary RGB/start/end blend, including RGB byte truncation; other fields copy immediately. Blend completion permits changed controller fields to copy on a subsequent tick after the saved handle is refreshed. Queries and Seek add no updates: player.sample_time_seconds identifies the latest player update, while player.render_time_seconds also controls interpolation from the copied controller targets/deadline. Inspect controller_states and player.current_raw/old_cache/new_cache. The declared schedule does not reproduce native packet, prediction or render cadence. Destination foglerptime controls duration; zero switches instantly. Same-controller requests refuse without restarting. The shared input parser owns value semantics and unsupported-input errors. Play and Pause control the shared preview clock; headless clients can Advance explicitly. Seek selects preview time while retaining the event sequence. Reset clears preview events and returns to initial values under the current context at time zero, retaining settings and a current prepared environment. Cancel stops pending work and playback while retaining a current published state; Clear disables the preview and clears its controller, environment, event sequence and time. Background requests return owned pending status when work is needed; synchronous requests wait outside the Session lock. Invalid fields, stale snapshots and unsupported sources refuse explicitly. Preview controls preserve VMF properties and Undo history."
    )]
    pub(in crate::mcp) fn control_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewControlArgs>,
    ) -> ToolResult {
        run(|| {
            let current = self.session().fog_preview_settings();
            let (snapshot, command, background) = args.command(current)?;
            self.document_access()
                .fog_preview(Actor::Agent, &snapshot, command, background)
        })
    }
}
