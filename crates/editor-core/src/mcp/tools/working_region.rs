use super::super::*;

#[tool_router(router = working_region_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the session-local working-region viewport filter: snapshot, enabled bounds, exact total/excluded/selected_excluded counts and paginated object/child-brush eligibility, reasons and bounds source. offset defaults 0; limit defaults 50 (1–200). Reasons distinguish authored_hidden, owner_hidden, tool_filter, outside_working_region and unresolved_bounds. Whole brush owners stay together; models use asset bounds with explicit helper fallback. This display filter never edits saved source, selection, history or compile scope.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_working_region(
        &self,
        Parameters(args): Parameters<WorkingRegionInspectArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .get_working_region(args.offset, args.limit.unwrap_or(50))
        })
    }

    #[tool(
        description = "Set the shared editor-only working region using latest get_working_region.snapshot. action=set requires finite ordered mins/maxs within editor bounds; enabled defaults true. action=selection_bounds captures the current object/face selection (empty rejects) and accepts enabled only. action=clear accepts no bounds/enabled. Contradictory/stale/invalid requests preserve current settings. Touching/intersecting whole world brushes and whole brush owners remain; point entities use model/helper bounds. Changes refresh desktop/MCP scenes and spatial picking while preserving source, selection, Undo and owned geometry/overlay drafts; active spatial gestures are cancelled. Compilation and saved hide/show are independent. UI: Working region…; Clear working region remains reachable when everything is excluded."
    )]
    pub(in crate::mcp) fn set_working_region(
        &self,
        Parameters(args): Parameters<WorkingRegionSetArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::working_region::Action;
            let action = match args.action.as_str() {
                "set" => Action::Set { bounds: [DVec3::from_array(args.mins.ok_or("set requires mins")?), DVec3::from_array(args.maxs.ok_or("set requires maxs")?)], enabled: args.enabled.unwrap_or(true) },
                "selection_bounds" if args.mins.is_none() && args.maxs.is_none() => Action::SelectionBounds { enabled: args.enabled.unwrap_or(true) },
                "clear" if args.mins.is_none() && args.maxs.is_none() && args.enabled.is_none() => Action::Clear,
                _ => return Err("Use set with mins/maxs, selection_bounds without bounds, or clear without bounds/enabled".into()),
            };
            self.session()
                .set_working_region(Actor::Agent, action, &args.snapshot)
        })
    }
}
