use super::super::*;

#[tool_router(router = workspace_preferences_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect current workspace layout, saved startup defaults, factory values, storage readiness/errors, layout_serial, layout_snapshot for current layout edits and snapshot for saved-default actions. Layout fields: dock_visible, dock_width (240–640 logical pixels), agent_visible, agent_height (96–480 logical pixels), tool_rail_visible. Factory values: true, 344, true, 250, true. Sizes are preferred and fit available window space; the Agent feed is inside the dock, so hiding the dock hides both. Ordinary changes never save automatically. Shared with View → Workspace layout…. Headless sessions retain the same layout state for an attached native UI.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_workspace_preferences(
        &self,
        Parameters(_args): Parameters<WorkspacePreferencesInspectArgs>,
    ) -> ToolResult {
        ok(self.session().workspace_preferences_state())
    }

    #[tool(
        description = "Apply a complete current workspace layout using get_workspace_preferences.layout_snapshot and layout {dock_visible,dock_width,agent_visible,agent_height,tool_rail_visible}. All fields are required; unknown fields reject. Dock width is 240–640 and Agent height 96–480 logical pixels, fitted to the actual window. Hiding the dock also hides its Agent feed. Current layout changes remain available when saved-default storage is unavailable. Preserves map, selection, history, camera and inactive owned drafts/reviews; real changes cancel captured viewport gestures. Changes current Session layout only and never write saved defaults. UI: Apply layout."
    )]
    pub(in crate::mcp) fn set_workspace_layout(
        &self,
        Parameters(args): Parameters<WorkspaceLayoutSetArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .set_workspace_layout(Actor::Agent, args.layout, &args.snapshot)
        })
    }

    #[tool(
        description = "Manage saved workspace defaults using get_workspace_preferences.snapshot. action=save explicitly saves the applied current layout for future Sessions; apply restores saved defaults into the current Session without writing; reload observes the file without applying; reset explicitly writes factory defaults without applying them. Only save/reset write. Malformed, unavailable, externally changed or stale storage reports explicit failures through the shared defaults authority; inspect readiness/error and write_committed. Current layout edits use the separate layout_snapshot and remain independent of storage errors. Layout changes preserve map/history/camera and cancel captured viewport gestures. UI: Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults."
    )]
    pub(in crate::mcp) fn set_workspace_preferences(
        &self,
        Parameters(args): Parameters<WorkspacePreferencesSetArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_workspace_preferences(
                Actor::Agent,
                args.action.name(),
                &args.snapshot,
            )
        })
    }
}
