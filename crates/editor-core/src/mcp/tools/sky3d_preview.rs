use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Settings,
    Prepare,
    Cancel,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sky3dPreviewArgs {
    action: Action,
    enabled: Option<bool>,
    camera: Option<String>,
    background: Option<bool>,
}

#[tool_router(router = sky3d_preview_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the default-off 3D sky preview and explicit authored sky_camera choices. Reports selected camera, origin/scale/errors, preparation status, readiness, current snapshot, diagnostics, geometry statistics and resource limits without graphics initialization. No automatic nearest/first camera selection. A ready prepared region does not prove that the current view displays sky pixels; perspective Textured is required. Source/material/visibility changes stale preparation. No source/history/selection/camera mutation or game/native-parity claim.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_3d_sky_preview(&self) -> ToolResult {
        ok(self.session().sky3d_preview_state())
    }

    #[tool(
        description = "Share the 3D sky preview… controls. action=settings requires enabled and optional explicit camera entity:<id>; enabling requires a supported sky_camera, disabled settings may retain an explicit choice or clear it by omitting camera. Settings validate atomically and never prepare geometry. action=prepare accepts only optional background (default false waits outside the Session lock; true starts an owned worker) and resolves the currently chosen sealed flat world/func_detail region. action=cancel accepts only action and requests cancellation. Pending, stale, cancelled and unavailable inputs never report ready. No whole-map/nearest-camera fallback, source edits, Undo, camera movement, compiler output or game launch. SKY2D retains the independent 2D path. Models/terrain/instances/water/portal boundaries, sky fog, translucency and runtime effects are unsupported. render_view.sky3d_preview temporarily overrides display of a current prepared region without changing the shared settings."
    )]
    pub(in crate::mcp) fn set_3d_sky_preview(
        &self,
        Parameters(args): Parameters<Sky3dPreviewArgs>,
    ) -> ToolResult {
        run(|| match args.action {
            Action::Settings => {
                if args.background.is_some() {
                    return Err("Settings accepts only action, enabled and camera".into());
                }
                let enabled = args.enabled.ok_or("Settings requires enabled")?;
                let camera = args
                    .camera
                    .as_deref()
                    .map(|id| match parse_id(id)? {
                        ObjectId::Entity(id) => Ok(id),
                        _ => Err("3D sky camera must be an entity:<id>".to_owned()),
                    })
                    .transpose()?;
                self.session().set_sky3d_preview(enabled, camera)
            }
            Action::Prepare => {
                if args.enabled.is_some() || args.camera.is_some() {
                    return Err("Prepare accepts only action and optional background".into());
                }
                let access = self.document_access();
                if args.background.unwrap_or(false) {
                    access.start_sky3d_preview(Actor::Agent)
                } else {
                    access.with_shared(|shared| {
                        let request = lock(shared).begin_sky3d_preview(Actor::Agent)?;
                        let completed = request.run();
                        lock(shared).finish_sky3d_preview(Actor::Agent, completed)
                    })
                }
            }
            Action::Cancel => {
                if args.enabled.is_some() || args.camera.is_some() || args.background.is_some() {
                    return Err("Cancel accepts only action".into());
                }
                self.session().cancel_sky3d_preview()
            }
        })
    }
}

#[cfg(test)]
mod tests;
