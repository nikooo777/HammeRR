use super::super::*;
use crate::session::ropes::{Command, Settings};

impl RopePreviewArgs {
    fn command(&self, current: Settings) -> Result<Command, String> {
        let environment = self.gravity.is_some() || self.wind.is_some();
        let time = self.time_seconds.is_some();
        let advance = self.seconds.is_some();
        match self.action {
            RopePreviewAction::Settings => {
                if !environment || time || advance || self.background.is_some() {
                    return Err(
                        "Settings accepts gravity and/or wind, with no time or background fields"
                            .into(),
                    );
                }
                Ok(Command::Settings(Settings {
                    gravity: self.gravity.unwrap_or(current.gravity),
                    wind: self.wind.unwrap_or(current.wind),
                }))
            }
            RopePreviewAction::Seek => {
                if environment || advance {
                    return Err("Seek accepts only time_seconds and optional background".into());
                }
                Ok(Command::Seek {
                    time_seconds: self.time_seconds.ok_or("Seek requires time_seconds")?,
                })
            }
            RopePreviewAction::Advance => {
                if environment || time {
                    return Err("Advance accepts only seconds and optional background".into());
                }
                Ok(Command::Advance {
                    seconds: self.seconds.ok_or("Advance requires seconds")?,
                })
            }
            RopePreviewAction::Prepare | RopePreviewAction::Reset => {
                if environment || time || advance {
                    return Err("Prepare and Reset accept only optional background".into());
                }
                Ok(if matches!(self.action, RopePreviewAction::Prepare) {
                    Command::Prepare
                } else {
                    Command::Reset
                })
            }
            RopePreviewAction::Play | RopePreviewAction::Pause | RopePreviewAction::Cancel => {
                if environment || time || advance || self.background.is_some() {
                    return Err("Play, Pause and Cancel accept no additional control fields".into());
                }
                Ok(match self.action {
                    RopePreviewAction::Play => Command::Play,
                    RopePreviewAction::Pause => Command::Pause,
                    _ => Command::Cancel,
                })
            }
        }
    }
}

#[tool_router(router = rope_preview_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Control the shared deterministic editor rope preview through the same Settings, Prepare preview, Play, Pause, Seek, Advance, Reset and Cancel operations as the desktop. Inspect get_rope_preview first and pass its current opaque snapshot. Settings accepts explicit gravity and/or direct world wind acceleration and invalidates the prepared pose; it does not edit entity fields or saved defaults. Prepare resolves the current authored endpoints, supported collision geometry and material sources outside the Session lock. Play advances through one shared runtime clock in both desktop and headless sessions. Pause freezes playback; optional Advance explicitly steps time while paused or playing. Seek and Reset pause and replay the same fixed-step interpolation. Background work owns one cancellable document operation; pending reports distinguish published time from the requested target. Source changes and failed preparation clear readiness instead of substituting a static or stale curve. Pause and Cancel require the current snapshot but may be requested by either actor. Visibility remains set_view.rope_preview. No VMF changes or Undo entries."
    )]
    pub(in crate::mcp) fn set_rope_preview(
        &self,
        Parameters(args): Parameters<RopePreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let settings = self.session().rope_preview_settings();
            let command = args.command(settings)?;
            self.document_access().rope_preview(
                Actor::Agent,
                &args.expected_snapshot,
                command,
                args.background.unwrap_or(false),
            )
        })
    }
}
