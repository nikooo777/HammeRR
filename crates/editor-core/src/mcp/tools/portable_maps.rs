use super::super::*;

#[tool_router(router=portable_map_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Read the current map's portable asset attachment, file/byte counts, payload digest and explicitly selected local source paths. Shared with File → Portable map…. No resource loading or file changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_portable_map(&self) -> ToolResult {
        ok(self.session().portable_map_state())
    }

    #[tool(
        description = "Attach or export portable VMF content through the shared Session operation. action=attach imports explicitly supplied companion_bsp and/or ordered content_roots into the current map; normal Save then refreshes and atomically retains its versioned compressed asset package. action=share writes an explicit destination .vmf copy; optional source_vmf reads a source without opening or repairing its geometry. No same-named BSP is inferred. action=export_plain writes a VMF without the package; action=export_assets writes its assets as a standard ZIP for external tools. Exports preserve source VMFs and the open document. Packaged content wins over configured mounts on Open. Missing/unsupported dependencies, damaged packages, path/case conflicts and resource limits reject explicitly; no partial package is presented as complete. Hammer/external compilers do not mount this extension themselves."
    )]
    pub(in crate::mcp) async fn portable_map(
        &self,
        Parameters(options): Parameters<crate::session::portable_maps::Options>,
    ) -> ToolResult {
        let server = self.clone();
        match tokio::task::spawn_blocking(
            self.worker(move || server.session().portable_map(Actor::Agent, options)),
        )
        .await
        {
            Ok(result) => run(|| result),
            Err(error) => fail(format!("Portable map operation failed: {error}")),
        }
    }
}
