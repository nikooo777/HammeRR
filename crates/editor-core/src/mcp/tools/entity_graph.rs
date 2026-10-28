use super::super::*;

#[tool_router(router = entity_graph_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect a bounded authored entity I/O graph from explicit roots and direction outgoing/incoming/both; depth defaults2, range1–16. Uses committed visible root-map entities and existing checked I/O parsing/name matching. Follows every exact named fan-out and !self edge, safely deduplicating cycles. Wildcard-prefix candidates, class/runtime-dependent, missing and malformed records are explicit unresolved diagnostics, never guessed concrete edges. Saved-hidden entities, expanded instance contents and unapplied drafts are excluded. Returns session-owned snapshot, structured nodes/edges, separate node/edge/unresolved pages (limit1–64), complete traversal counts and limits. Unresolved pages cover the admitted whole root map. Search filters rows only. Nonzero page offsets require the returned snapshot; stale pages reject. Hard-cap exhaustion refuses without an incomplete traversal. UI: Entity graph… → Inspect graph.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_entity_graph(
        &self,
        Parameters(args): Parameters<EntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = args.traversal()?;
            let query = crate::session::entity_graph::Query {
                snapshot: args.snapshot,
                traversal,
                search: args.search.unwrap_or_default(),
                node_offset: args.node_offset,
                edge_offset: args.edge_offset,
                unresolved_offset: args.unresolved_offset,
                limit: args.limit.unwrap_or(crate::session::entity_graph::MAX_PAGE),
            };
            Ok(json!(self.session().inspect_entity_graph(&query)?))
        })
    }

    #[tool(
        description = "Select the complete bounded exact-link entity traversal from roots/direction/depth and matching inspect_entity_graph.snapshot. Includes roots and every reached visible authored entity, independent of result pages and search. mode replace/add/remove/toggle uses ordinary shared explicit-ID selection. Revalidates current session/map/query ownership and recomputes all nodes before mutation; stale tokens, cap exhaustion, geometry drafts and active gestures refuse atomically. Map/history/camera stay unchanged. Runtime/wildcard/class uncertainty is not followed. UI: Entity graph… → Select connected."
    )]
    pub(in crate::mcp) fn select_entity_graph(
        &self,
        Parameters(args): Parameters<SelectEntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(args.roots, args.direction, args.depth)?;
            let mode = parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?;
            Ok(json!(self.session().select_entity_graph(
                Actor::Agent,
                &traversal,
                &args.snapshot,
                mode
            )?))
        })
    }

    #[tool(
        description = "Frame every node in the complete bounded exact-link entity graph, using matching roots/direction/depth and inspect_entity_graph.snapshot. Optional aspect defaults1, range0.01–100. Preserves selection, map and history; uses ordinary authored-object framing bounds and camera publication. Search/page filters do not restrict framing. Revalidates snapshot and refuses active geometry drafts/gestures, missing bounds or invalid camera/aspect before mutation. No instance expansion or source-file opening. UI: Entity graph… → Frame connected."
    )]
    pub(in crate::mcp) fn frame_entity_graph(
        &self,
        Parameters(args): Parameters<FrameEntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(args.roots, args.direction, args.depth)?;
            let frame = self.session().frame_entity_graph(
                Actor::Agent,
                &traversal,
                &args.snapshot,
                args.aspect.unwrap_or(1.0),
            )?;
            Ok(
                json!({"snapshot":frame.snapshot,"matched":frame.matched,"bounds":frame.bounds,"camera":camera_json(&frame.camera)}),
            )
        })
    }
}
