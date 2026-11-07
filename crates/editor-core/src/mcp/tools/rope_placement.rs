use super::super::*;

#[tool_router(router = rope_placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect shared rope placement, snapshots, owned draft token, endpoint picks, readiness, errors and pending rope span. Preview does not edit the map or history. UI: Place rope….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_rope_placement(&self) -> ToolResult {
        ok(self.session().get_rope_placement())
    }

    #[tool(
        description = "Begin shared rope placement with request {material, slack, width} and the latest placement snapshot. Slack is signed integer Source units: 100 adds no nominal length; larger values add slack. Material errors reject without substitution. Then pick both endpoints, inspect/render, and Apply or Cancel."
    )]
    pub(in crate::mcp) fn begin_rope_placement(
        &self,
        Parameters(args): Parameters<RopePlacementArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .begin_rope_placement(Actor::Agent, args.request, &args.snapshot)
        })
    }

    #[tool(
        description = "Replace the owned rope draft's complete request {material, slack, width} through shared validation. Slack is a signed Source integer (-2048–2047); width is 1–64 units. Invalid or unavailable assets report errors without creating entities."
    )]
    pub(in crate::mcp) fn update_rope_placement(
        &self,
        Parameters(args): Parameters<RopePlacementUpdateArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .update_rope_placement(Actor::Agent, args.token, args.request)
        })
    }

    #[tool(
        description = "Arm start or end surface picking for the owned rope token using the latest pick_snapshot and captured camera/viewport. Omitted camera uses the shared camera. Subsequent ray or pixel picks must use this current capture. UI: Pick start / Pick end."
    )]
    pub(in crate::mcp) fn begin_rope_placement_pick(
        &self,
        Parameters(args): Parameters<RopePlacementBeginPickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, args.width as f32 / args.height as f32)?;
            session.begin_rope_placement_pick(
                Actor::Agent,
                args.token,
                args.endpoint,
                camera,
                (args.width, args.height),
                &args.pick_snapshot,
            )
        })
    }

    #[tool(
        description = "Pick the armed rope endpoint at the first visible authored world/func_detail brush or terrain surface hit by a finite nonzero ray. Requires the current captured pick context and owned token; arbitrary endpoint coordinates and runtime/model attachments are unavailable."
    )]
    pub(in crate::mcp) fn pick_rope_placement_ray(
        &self,
        Parameters(args): Parameters<RopePlacementRayArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().pick_rope_placement_ray(
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
        description = "Pick the armed rope endpoint from a pixel inside its captured viewport. Pointer dimensions must match the capture; changed camera, scene or view rejects. Uses the same visible brush/terrain hit validation as the desktop."
    )]
    pub(in crate::mcp) fn pick_rope_placement_pixel(
        &self,
        Parameters(args): Parameters<RopePlacementPixelArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().pick_rope_placement_pixel(
                Actor::Agent,
                args.token,
                args.pointer.resolve()?,
            )
        })
    }

    #[tool(
        description = "Stop surface picking for the matching owned rope token while retaining the draft and its settings. Does not edit the map. UI: Cancel picking."
    )]
    pub(in crate::mcp) fn cancel_rope_placement_pick(
        &self,
        Parameters(args): Parameters<RopePlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .cancel_rope_placement_pick(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Review and take ownership of the current rope draft, returning a new token. The old owner's token becomes invalid. Stale drafts remain unavailable; Cancel and Begin again for fresh endpoint picks. UI: Review shared placement."
    )]
    pub(in crate::mcp) fn review_rope_placement(
        &self,
        Parameters(args): Parameters<RopePlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .review_rope_placement(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Apply the owned ready rope preview as two uniquely named endpoint entities with their NextKey link in one atomic Undo step. Rechecks captured map, picks and asset sources; stale, foreign or replaced drafts reject without partial creation. UI: Apply rope."
    )]
    pub(in crate::mcp) fn apply_rope_placement(
        &self,
        Parameters(args): Parameters<RopePlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_rope_placement(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel the matching owned rope draft, including stale cleanup. Source, selection and history stay unchanged; foreign or replaced tokens cannot clear another draft. UI: Cancel / Escape."
    )]
    pub(in crate::mcp) fn cancel_rope_placement(
        &self,
        Parameters(args): Parameters<RopePlacementTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_rope_placement(Actor::Agent, args.token)?;
            Ok(session.get_rope_placement())
        })
    }
}
