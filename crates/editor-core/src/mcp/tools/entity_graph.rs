use super::super::*;

#[tool_router(router = entity_graph_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect a bounded committed entity I/O graph from explicit authored roots, direction outgoing/incoming/both and depth1–16. scope authored(default) or expanded uses checked source preparation; expanded instance helpers seed contained entities. Native event matching follows every name match, then class matches only when no names match, with installed CSS byte/wildcard semantics. context.activator/caller supplies explicit source-scoped scenario IDs; caller omission defaults to each output owner, caller_unbound models null. !self uses caller. Player/picker and unknown runtime identities remain unresolved. Virtual IDs retain source provenance and select authored helper owners. Returns snapshot-bound separate pages(max64), full traversal counts and limits; search/pages never restrict connected selection/framing. Expanded source/dependency failures do not fall back to root scope. Cap exhaustion refuses rather than truncating. UI: Entity graph… → Inspect graph.",
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
        description = "Select the complete bounded graph traversal or optional exact diagram node IDs using the matching traversal/scope/context snapshot. Source-scoped virtual nodes map to authored helper owners, deduplicated before ordinary replace/add/remove/toggle selection. Search/pages never restrict the complete action. Stale source/query/dependency, cap exhaustion and active geometry work refuse atomically; map/history/camera remain unchanged. UI: Entity graph… → Select connected / Select owner."
    )]
    pub(in crate::mcp) fn select_entity_graph(
        &self,
        Parameters(args): Parameters<SelectEntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            let mode = parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?;
            let mut session = self.session();
            let report = match args.nodes {
                Some(nodes) => session.select_entity_graph_nodes(
                    Actor::Agent,
                    &traversal,
                    &args.snapshot,
                    &nodes,
                    mode,
                )?,
                None => {
                    session.select_entity_graph(Actor::Agent, &traversal, &args.snapshot, mode)?
                }
            };
            Ok(json!(report))
        })
    }

    #[tool(
        description = "Frame the complete graph traversal or optional exact diagram node IDs using the matching traversal/scope/context snapshot. Uses every requested authored/prepared node position; missing or deferred bounds refuse the whole action explicitly. Optional aspect0.01–100 defaults1. Preserves selection/map/history; uses ordinary camera publication. Search/pages do not restrict the complete action. UI: Entity graph… → Frame connected / Frame node."
    )]
    pub(in crate::mcp) fn frame_entity_graph(
        &self,
        Parameters(args): Parameters<FrameEntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            let mut session = self.session();
            let frame = match args.nodes {
                Some(nodes) => session.frame_entity_graph_nodes(
                    Actor::Agent,
                    &traversal,
                    &args.snapshot,
                    &nodes,
                    args.aspect.unwrap_or(1.0),
                )?,
                None => session.frame_entity_graph(
                    Actor::Agent,
                    &traversal,
                    &args.snapshot,
                    args.aspect.unwrap_or(1.0),
                )?,
            };
            Ok(
                json!({"snapshot":frame.snapshot,"matched":frame.matched,"bounds":frame.bounds,"camera":camera_json(&frame.camera)}),
            )
        })
    }
    #[tool(
        description = "Read the complete bounded entity graph diagram for the current traversal snapshot. Returns source-scoped nodes, full edge details, deterministic positions and shared session-local pan/zoom. Search and pages do not restrict it. Report limit1MiB; no partial canvas is returned. UI: Entity graph… → Canvas.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_entity_graph_diagram(
        &self,
        Parameters(args): Parameters<EntityGraphDiagramArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            Ok(json!(self.session().inspect_entity_graph_diagram(
                &traversal,
                &args.snapshot
            )?))
        })
    }

    #[tool(
        description = "Arrange the complete graph diagram using layered BFS-depth columns or a deterministic grid. Cycles remain edges. Clears custom node positions and preserves current pan/zoom. Session-local presentation only; no map or Undo changes. UI: Entity graph… → Layered / Grid."
    )]
    pub(in crate::mcp) fn layout_entity_graph(
        &self,
        Parameters(args): Parameters<LayoutEntityGraphArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            Ok(json!(self.session().layout_entity_graph(
                &traversal,
                &args.snapshot,
                args.layout
            )?))
        })
    }

    #[tool(
        description = "Commit diagram node positions and/or view for a matching graph snapshot. Positions/pan are finite within ±1000000, zoom0.1–4. IDs must belong to the complete traversal. One atomic presentation update; map, history, selection and camera remain unchanged. UI commits node/pan gestures on release."
    )]
    pub(in crate::mcp) fn update_entity_graph_diagram(
        &self,
        Parameters(args): Parameters<UpdateEntityGraphDiagramArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            Ok(json!(self.session().update_entity_graph_diagram(
                &traversal,
                &args.snapshot,
                &args.positions,
                args.view
            )?))
        })
    }

    #[tool(
        description = "Reset graph presentation to the deterministic layered layout, no custom positions, pan[24,24] and zoom1. Uses the current traversal snapshot. Session-local presentation only; no source/history changes. UI: Entity graph… → Reset diagram."
    )]
    pub(in crate::mcp) fn reset_entity_graph_diagram(
        &self,
        Parameters(args): Parameters<EntityGraphDiagramArgs>,
    ) -> ToolResult {
        run(|| {
            let traversal = entity_graph_traversal(
                args.roots,
                args.direction,
                args.depth,
                args.scope,
                args.context,
            )?;
            Ok(json!(
                self.session()
                    .reset_entity_graph_diagram(&traversal, &args.snapshot)?
            ))
        })
    }
}
