use super::super::*;

#[tool_router(router = color_correction_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared default-off editor color-correction preview, explicit physical LUT path, admitted cache identity/SHA256, requested enable/weight, source readiness, errors and bounds. Rechecks the original bounded source and invalidates changed/missing data. Source readiness does not prove a GPU frame was graded. One 32x32x32 RGB8 Source .raw LUT; whole-viewport sRGB display domain, including helpers/background. No native HDR, map entities/volumes, multiple LUTs or game composition parity.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_color_correction_preview(&self) -> ToolResult {
        ok(self.session().color_correction_state())
    }

    #[tool(
        description = "Load one explicit physical Source .raw LUT through the same operation as Load LUT. Requires exactly 98304 packed RGB8 bytes, 32 cubed entries, red fastest, then green, then blue; .cube/image/headered files are unsupported. Load turns preview off. A failed load clears earlier LUT data and retains an explicit error; it never substitutes identity or stale cached data. Bounded opened-target/source identity checks reject missing, changing and oversized input. Does not edit VMF, undo, compiler output or user preferences."
    )]
    pub(in crate::mcp) fn load_color_correction_lut(
        &self,
        Parameters(args): Parameters<LoadColorCorrectionArgs>,
    ) -> ToolResult {
        run(|| self.session().load_color_correction(Path::new(&args.path)))
    }

    #[tool(
        description = "Share Enable color correction, Weight and Apply preview. Requires finite weight 0..1. Enabling rechecks the original loaded LUT; source errors invalidate data and require explicit Load LUT. Display-domain trilinear lookup applies once after all viewport rendering, including helpers and background, and is shared by render_view. Requested grading with unavailable source/GPU/target fails render_view; the desktop displays an ungraded frame with an explicit warning. Disable is available after failure. No source/history/camera change or native HDR parity."
    )]
    pub(in crate::mcp) fn set_color_correction_preview(
        &self,
        Parameters(args): Parameters<SetColorCorrectionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .set_color_correction(args.enabled, args.weight, args.expected_identity)
        })
    }

    #[tool(
        description = "Share Clear LUT: disable color correction, release admitted LUT data and clear its source/error. Does not edit the map or saved defaults."
    )]
    pub(in crate::mcp) fn clear_color_correction_lut(&self) -> ToolResult {
        ok(self.session().clear_color_correction())
    }
}
