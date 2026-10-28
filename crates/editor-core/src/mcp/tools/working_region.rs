use super::super::*;

#[tool_router(router = working_region_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the session-local working-region viewport filter: snapshot, enabled bounds, exact total/excluded/selected_excluded counts and paginated object/child-brush eligibility, reasons and bounds source. offset defaults 0; limit defaults 50 (1–200). Reasons distinguish authored_hidden, owner_hidden, editor_hidden, owner_editor_hidden, tool_filter, outside_working_region and unresolved_bounds. Whole brush owners stay together; models use asset bounds with explicit helper fallback. This display filter never edits saved source, selection, history or compile scope.",
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
        description = "Set the shared editor-only working region using latest get_working_region.snapshot. action=set requires finite ordered mins/maxs within editor bounds; enabled defaults true. action=selection_bounds captures the current object/face selection (empty rejects) and accepts enabled only. action=clear accepts no bounds/enabled. Contradictory/stale/invalid requests preserve current settings. Touching/intersecting whole world brushes and whole brush owners remain; point entities use model/helper bounds. Changes refresh desktop/MCP scenes and spatial picking while preserving source, selection, Undo and owned geometry/overlay drafts; active spatial gestures are cancelled. Compilation, saved hide/show and temporary editor-hidden exclusions are independent. UI: Working region…; Clear working region remains reachable when everything is excluded."
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
    #[tool(
        description = "Inspect temporary editor-hidden committed root IDs and a current snapshot. offset defaults 0; limit defaults 50 (1–200). This filter affects viewport rendering and spatial hits independently of saved Hide, object-selection type flags, tool textures and working region. Explicit-ID reports and edits remain available. New/Open resets it; removed source IDs are pruned after edits/Undo/Redo.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_editor_visibility(
        &self,
        Parameters(args): Parameters<WorkingRegionInspectArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .get_editor_visibility(args.offset, args.limit.unwrap_or(50))
        })
    }

    #[tool(
        description = "Change temporary editor visibility using the latest get_editor_visibility snapshot. action=hide accepts optional root ids (defaults current selection), validates the entire batch, and adds exclusions without further group expansion or changing selection/source/history/builds. Apply or cancel displayed geometry previews before Hide. action=show_all accepts snapshot only, clears only temporary exclusions, and remains available during previews, Faces mode, empty selection, disabled pick types and empty working regions. Real changes cancel active spatial gestures while preserving inactive owned reviews; stale/invalid/no-op requests leave drafts unchanged. UI: Selection → Hide selected in editor / Show all editor-hidden."
    )]
    pub(in crate::mcp) fn set_editor_visibility(
        &self,
        Parameters(args): Parameters<EditorVisibilitySetArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::editor_visibility::Action;
            let mut session = self.session();
            let (action, snapshot) = match args {
                EditorVisibilitySetArgs::Hide { snapshot, ids } => (
                    Action::Hide(ids_or_selection(&session, ids.as_deref())?),
                    snapshot,
                ),
                EditorVisibilitySetArgs::ShowAll { snapshot } => (Action::ShowAll, snapshot),
            };
            session.set_editor_visibility(Actor::Agent, action, &snapshot)
        })
    }
}
