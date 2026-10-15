use super::super::*;

#[tool_router(router = overlay_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Search existing info_overlay entities by name/material/ID without changing selection. Returns current overlay snapshot, exact total and paged rows with entity/name/material, existing bound face, settings, eligibility and rejection reason. A supported rebind target has one visible world/detail quad binding. UI: Overlay… → Existing overlay.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_overlay_targets(
        &self,
        Parameters(args): Parameters<OverlayTargetsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().browse_overlay_targets(
                args.query.as_deref().unwrap_or(""),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(20),
            )
        })
    }

    #[tool(
        description = "Preview explicitly rebinding an existing info_overlay to a DIFFERENT visible world/func_detail quad source {solid,side}. Pass the entity ID from browse_overlay_targets and current overlay snapshot. Existing binding must itself contain one visible eligible quad; multiside/hidden/missing/terrain/model/nonquad sources reject. Preserves raw settings, connections and metadata. Returns operation=rebind, entity and previous_source; owned review_overlay/apply_overlay/cancel_overlay work as usual. Apply updates the same entity in one Undo step. UI: select destination face → Overlay… → choose Existing overlay → Preview rebind → Apply rebind."
    )]
    pub(in crate::mcp) fn preview_overlay_rebind(
        &self,
        Parameters(args): Parameters<OverlayRebindArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, Some(&args.id))?;
            session.preview_overlay_rebind(
                Actor::Agent,
                id,
                map_core::surface::FaceId {
                    solid: args.source.solid,
                    side: args.source.side,
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Inspect shared overlay creation, existing-binding refit and explicit new-face rebind: current snapshot, selected source/overlay eligibility and rejection reasons, draft operation/target/owner/token/validity, settings, corners and bounds. Preview changes no map, selection or history. Create adds an entity; Refit and Rebind update the same entity projection without changing its authored settings."
    )]
    pub(in crate::mcp) fn get_overlay_state(&self) -> ToolResult {
        ok(self.session().get_overlay_state())
    }

    #[tool(
        description = "Preview refitting one visible existing info_overlay to its EXISTING single bound world/func_detail quad. id defaults to the single selected overlay; pass latest get_overlay_state.snapshot. No new-face rebinding. Rejects multiside, hidden, missing, terrain, brush-model and nonquad bindings without replacing a valid draft. Returns draft.operation=refit and its entity. Apply updates the same entity's projection/origin/sides only, preserving option strings, connections and opaque metadata. Use review_overlay, apply_overlay or cancel_overlay with the owned token. UI: Overlay… → Refit selected overlay → Apply refit."
    )]
    pub(in crate::mcp) fn preview_overlay_refit(
        &self,
        Parameters(args): Parameters<OverlayRefitArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.preview_overlay_refit(Actor::Agent, id, &args.snapshot)
        })
    }

    #[tool(
        description = "Fit a new overlay to one explicit visible planar world/func_detail quad face, using source {solid,side} and the latest get_overlay_state.snapshot. material is a material path without materials/ or extension; targetname is optional. render_order defaults 0, u/v to [0,1], fade_min/fade_max to -1/0. Uses the shared checked core definition and derived basis. Valid preview replaces the current draft and owns its token; invalid or stale input preserves the last valid draft. Source, selection and Undo remain unchanged. UI: Surface → Overlay… → Fit / Preview."
    )]
    pub(in crate::mcp) fn preview_overlay(
        &self,
        Parameters(args): Parameters<OverlayPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().preview_overlay(
                Actor::Agent,
                crate::session::overlay::Request {
                    source: map_core::surface::FaceId {
                        solid: args.source.solid,
                        side: args.source.side,
                    },
                    options: map_core::overlay::Options {
                        material: args.material,
                        targetname: args.targetname,
                        render_order: args.render_order.unwrap_or(0),
                        u: args.u.unwrap_or([0.0, 1.0]),
                        v: args.v.unwrap_or([0.0, 1.0]),
                        fade_min: args.fade_min.unwrap_or(-1.0),
                        fade_max: args.fade_max.unwrap_or(0.0),
                    },
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Explicitly review and take ownership of the current valid overlay draft. Returns a new token, invalidating the previous owner's token. Human Review and this tool use the same Session operation; foreign drafts cannot be applied or cancelled until reviewed."
    )]
    pub(in crate::mcp) fn review_overlay(
        &self,
        Parameters(args): Parameters<OverlayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().review_overlay(Actor::Agent, args.token))
    }

    #[tool(
        description = "Apply the current owned overlay draft in one atomic Undo step. draft.operation=create returns created and selects a new info_overlay; operation=refit/rebind returns updated and selects the same existing entity, preserving authored option strings, connections and metadata. Rejects stale/replaced/foreign tokens, changed map or selection, and active geometry previews. Source brush geometry stays unchanged; derived basis and side binding are generated by the same core operation used for Preview."
    )]
    pub(in crate::mcp) fn apply_overlay(
        &self,
        Parameters(args): Parameters<OverlayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_overlay(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel only the matching owned overlay draft, including stale cleanup. Foreign or replaced tokens reject without discarding another draft. Leaves source geometry, selection, history and other geometry previews unchanged. UI: Cancel / Escape."
    )]
    pub(in crate::mcp) fn cancel_overlay(
        &self,
        Parameters(args): Parameters<OverlayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().cancel_overlay(Actor::Agent, args.token))
    }
}
