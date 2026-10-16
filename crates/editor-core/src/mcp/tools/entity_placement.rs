use super::super::*;

#[tool_router(router = entity_placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect shared contextual point-entity placement: current snapshot/grid, owned draft token, validity, picking state, class/options and rendered plan with Source properties, origin, actual mesh/helper bounds, target point/normal and contact offset. Preview leaves map, selection and history unchanged. UI: Entity… → Pick surface….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_placement(&self) -> ToolResult {
        ok(self.session().get_entity_placement())
    }

    #[tool(
        description = "Arm a shared point-entity surface pick using classname, optional FGD properties, offset, align and snap, and latest get_entity_placement.snapshot. Preserves FGD defaults and Source angles/legacy angle/light pitch through existing creation/transform operations. Origins are derived from the picked surface. info_overlay uses Overlay…; contextual infodecal placement is unavailable. Finish competing geometry drafts first. UI: Entity… → Pick surface… → Pick surface. Then pick_entity_placement, inspect/render, and apply_entity_placement or cancel_entity_placement. Foreign drafts require explicit review."
    )]
    pub(in crate::mcp) fn begin_entity_placement(
        &self,
        Parameters(args): Parameters<EntityPlacementArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let snap = args.snap.unwrap_or(session.spatial().snap);
            session.begin_entity_placement(
                Actor::Agent,
                crate::session::entity_placement::Request {
                    classname: args.classname,
                    properties: args.properties.unwrap_or_default(),
                    offset: args.offset.unwrap_or(0.0),
                    align: args.align.unwrap_or(false),
                    snap,
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Pick the first visible brush, terrain triangle or supported model surface for the owned entity-placement token. Finite nonzero ray required. Uses the same support/contact planner as selected-object placement. Optional snapping rounds the two world axes tangent to the normal's dominant axis, then rechecks contact on the same visible face; leaving the surface rejects. Result bounds must stay within ±65536. Misses or invalid input preserve the draft. After a valid pick, begin again to choose another target. UI: click visible surface while picking."
    )]
    pub(in crate::mcp) fn pick_entity_placement(
        &self,
        Parameters(args): Parameters<EntityPlacementPickArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().pick_entity_placement(
                Actor::Agent,
                args.token,
                Ray {
                    origin: vec3(args.ray_origin),
                    direction: vec3(args.ray_direction),
                },
            )
        })
    }

    #[tool(
        description = "Review and take ownership of the current contextual entity-placement draft, including stale cleanup. Returns a new token and invalidates the previous owner's token. Stale geometry still requires another Pick surface before Apply. UI: Review shared placement."
    )]
    pub(in crate::mcp) fn review_entity_placement(
        &self,
        Parameters(args): Parameters<EntityPlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .review_entity_placement(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Apply the owned valid entity-placement preview in one atomic Undo step and select the created entity. Replays existing creation plus Source orientation transform and verifies exact preview identity/properties before commit. Changed maps, selection, scene/grid, missing picks, foreign or replaced tokens reject. UI: Apply entity."
    )]
    pub(in crate::mcp) fn apply_entity_placement(
        &self,
        Parameters(args): Parameters<EntityPlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_entity_placement(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel only the matching owned entity-placement draft, including stale cleanup. Source, selection and history stay unchanged; foreign/replaced tokens cannot clear replacement geometry. UI: Cancel / Escape / Cancel picking."
    )]
    pub(in crate::mcp) fn cancel_entity_placement(
        &self,
        Parameters(args): Parameters<EntityPlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_entity_placement(Actor::Agent, args.token)?;
            Ok(session.get_entity_placement())
        })
    }
}
