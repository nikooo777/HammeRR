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
        description = "Inspect the default-off 3D sky preview and explicit authored sky_camera choices. Reports selected camera, authored/received origin, scale, separate authored/initial/scaled sky fog profiles, defaults/inert keys, preparation status, readiness, current snapshot, diagnostics, geometry statistics, real brush/terrain/model provenance, source-guard count and resource limits without graphics initialization. No automatic nearest/first camera selection. A ready prepared region does not prove that the current view displays sky pixels; perspective Textured is required. Document/material/visibility changes and changed consumed source files or winning namespaces retire preparation. No source/history/selection/camera mutation or game/native-parity claim.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_3d_sky_preview(&self) -> ToolResult {
        ok(self.session().sky3d_preview_state())
    }

    #[tool(
        description = "Share the 3D sky preview… controls. action=settings requires enabled and optional explicit camera entity:<id>; enabling requires a supported sky_camera, disabled settings may retain an explicit choice or clear it by omitting camera. Settings validate atomically and never prepare geometry. action=prepare accepts only optional background (default false waits outside the Session lock; true starts an owned worker) and resolves the currently chosen sealed world/func_detail region, certified whole authored fine terrain patches and fixed unit-scale single-bone prop_static LOD0 meshes with selected skins. action=cancel accepts only action and requests cancellation. Pending, stale, cancelled and unavailable inputs never report ready. No whole-map/nearest-camera fallback, source edits, Undo, camera movement, compiler output or game launch. SKY2D retains the independent 2D path. The selected camera's static LDR fog applies automatically with a ready enabled miniature in Textured perspective, independently of world Fog preview/controller; the shared initial radial context applies even while world Fog preview is off. Flat world/detail surfaces also support strict plain single-base cutout or translucent LightmappedGeneric/UnlitGeneric materials; terrain and models remain opaque. Cutouts use checked native byte references. Glass draws after opaque geometry with approximate back-to-front batch-center sorting; intersecting or cyclic glass can blend incorrectly. Instances, water, portal boundaries, animation and runtime effects are unsupported. Consumed topology/material/texture/model sources remain guarded; changed original files or winning namespaces retire readiness and requested stale rendering fails. Changed VPK indexes or mounts require reload_content before preparation. Catalogue refresh remains explicit. Inspect get_3d_sky_preview.limits for exact material, geometry, source and memory profiles. Sky fog has no controller inputs or farz; unsupported combined 2D sky materials refuse explicitly. render_view.sky3d_preview temporarily overrides display of a current prepared region without changing the shared settings."
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
