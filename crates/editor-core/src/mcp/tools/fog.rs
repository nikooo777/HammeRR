use super::super::*;
use crate::session::fog::{Command, Settings};

impl FogPreviewControlArgs {
    fn command(self) -> Result<(String, Command, bool), String> {
        let settings = self.settings.is_some();
        let controller = self.controller.is_some();
        let input = self.input.is_some() || self.value.is_some();
        let time = self.time_seconds.is_some();
        let advance = self.seconds.is_some();
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
            FogPreviewAction::PrepareEnvironment => {
                if settings || controller || input || time || advance {
                    return Err("Prepare environment accepts only optional background".into());
                }
                Command::PrepareEnvironment
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
        description = "Inspect the shared Fog preview, authored controller choices, effective settings, readiness and diagnostics. Returns the current opaque snapshot for reviewed preview operations. Preview controls do not edit VMF properties or Undo history.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_fog_preview(&self) -> ToolResult {
        run(|| self.document_access().inspect_fog_preview())
    }

    #[tool(
        description = "Apply the compatibility Fog preview operation through the shared Session. Pass enabled, an explicit env_fog_controller entity:<id> when enabling, and the latest get_fog_preview.snapshot. Invalid controllers and stale snapshots refuse without changing settings. Disabling remains available with a current snapshot. Preview controls preserve the map and Undo history."
    )]
    pub(in crate::mcp) fn set_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let controller = fog_controller_id(args.controller.as_deref())?;
            let channel = self.session().fog_preview_settings().channel;
            self.document_access().fog_preview(
                Actor::Agent,
                &args.snapshot,
                Command::Settings(Settings {
                    enabled: args.enabled,
                    controller,
                    selection: crate::session::fog::SelectionMode::Explicit,
                    channel,
                }),
                false,
            )
        })
    }

    #[tool(
        description = "Control the shared Fog preview through the same Settings, Prepare environment, native controller inputs and transport operations as the desktop. Inspect get_fog_preview first and pass its current expected_snapshot. Settings requires a complete settings object with enabled, controller, selection and channel; null controller clears the explicit choice. LDR is qualified; HDR requests refuse explicitly. Input requires an env_fog_controller entity:<id>, its exact native input name and raw parameter value when needed; the shared input parser owns value semantics and unsupported-input errors. Play and Pause control the shared preview clock; headless clients can Advance explicitly. Seek selects preview time while retaining the input sequence. Reset clears inputs and returns to authored initial values at time zero, retaining settings and a current prepared environment. Cancel stops pending work and playback while retaining a current published state; Clear disables the preview and clears its controller, environment, input sequence and time. Background requests return owned pending status when work is needed; synchronous requests wait outside the Session lock. Invalid fields, stale snapshots and unsupported sources refuse explicitly. Preview controls preserve VMF properties and Undo history."
    )]
    pub(in crate::mcp) fn control_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewControlArgs>,
    ) -> ToolResult {
        run(|| {
            let (snapshot, command, background) = args.command()?;
            self.document_access()
                .fog_preview(Actor::Agent, &snapshot, command, background)
        })
    }
}
