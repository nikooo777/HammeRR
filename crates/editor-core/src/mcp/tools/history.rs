use super::super::*;

#[tool_router(router = history_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the current document's retained transactions, newest first. Shares History in the editor. Reports author, label, applied/redo/current positions, real added/modified/removed object counts, up to 16 object IDs and changed map sections. Object-level summaries exclude property diffs and full snapshots. Default 50 entries, maximum 100; follow next_offset. Initial state is target '0'; each transaction state means after that transaction. History is session-local and clears when a document is opened/replaced. Use the returned snapshot and state with navigate_history. Unknown authors are explicit. Navigation is bounded to 256 transactions, blocks during build/launch/shutdown and active previews.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_history(
        &self,
        Parameters(args): Parameters<HistoryArgs>,
    ) -> ToolResult {
        run(|| {
            let report = self.session().inspect_history(
                args.offset,
                args.limit.unwrap_or(crate::session::history::PAGE_SIZE),
            )?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Go to the exact retained transaction state returned by get_history, or '0' for the initial state. Requires the current history snapshot; stale, replaced and discarded branch targets reject atomically. Replays existing undo/redo operations, preserving the redo chain and original transaction authors. Does not save or mark the map clean. Returns exact direction, transaction count, source/target states and resulting dirty state. Maximum 256 transactions per call; use intermediate states for longer histories. An already-current target is a no-op."
    )]
    pub(in crate::mcp) fn navigate_history(
        &self,
        Parameters(args): Parameters<HistoryNavigationArgs>,
    ) -> ToolResult {
        run(|| {
            let result =
                self.session()
                    .navigate_history(Actor::Agent, &args.snapshot, &args.target)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
    }
}
