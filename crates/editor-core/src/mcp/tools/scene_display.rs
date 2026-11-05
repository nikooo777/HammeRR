use super::super::*;

#[tool_router(router = scene_display_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect shared Scene display and manual Scene exposure EV, resolved separately for the active pane and every visible pane. HDR activates only for Perspective Textured views; other panes retain legacy display and component exposures. Reports requested/effective mode, effective owner/EV, per-channel Reinhard display curve, readiness and explicit errors without initializing graphics. This editor display policy does not claim native exposure adaptation or framebuffer parity.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_scene_display(&self) -> ToolResult {
        ok(self.session().scene_display_state())
    }

    #[tool(
        description = "Set the shared Scene display to legacy or hdr. Switching Legacy to HDR requires explicit finite exposure_ev -8..8; omitted fields preserve current HDR context. Perspective Textured HDR views compose scene radiance, translucency and world/independent sky fog before shared exposure and one per-channel Reinhard display pass. Component EV values remain stored for HDR-inactive panes. Changing display does not enable previews, change content channels, camera, map revision, Undo or fog history. Legacy display refuses while enabled fog explicitly requires HDR; choose LDR fog or disable it first. GPU/source/radiance errors remain explicit frame failures."
    )]
    pub(in crate::mcp) fn set_scene_display(
        &self,
        Parameters(args): Parameters<SetSceneDisplayArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .set_scene_display(args.mode.as_deref(), args.exposure_ev)
        })
    }
}
