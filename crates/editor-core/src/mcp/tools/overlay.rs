use super::super::*;

#[tool_router(router = overlay_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Move an existing single-face overlay's local corner draft through the same viewport handles as the UI. action=inspect returns handle_snapshot and ordinary overlay state. First preview_overlay_corners or review_overlay to own a valid corners draft. action=begin requires its token, latest handle_snapshot as snapshot, width/height (1–8192) and optional temporary camera matching render_view. Captures scene/view/camera/viewport; returns a new token and four projected draft.handles.pixels with an 11-pixel pick radius. action=pick and move require current token, finite x/y pixel coordinates inside the captured width/height; pick chooses the nearest visible projected corner, move uses relative unsnapped motion on the fixed projection plane. Every successful gesture action returns a new token; use that token next. Misses, stale camera/view/map/selection/viewport, nonconvex or native-invalid moves preserve the last valid draft. Finish ends a picked drag and returns to ordinary apply_overlay/cancel_overlay review; no map/history changes before Apply, which remains one Undo. Cancel restores the exact pre-handle draft even after stale context changes. Foreign/replaced tokens reject. Review closes handles and takes ownership of the last valid corners; Apply rejects active handles. Single world/func_detail quad only; no terrain/multiface authoring, snapping or projected material preview. UI: Overlay… → Edit selected corners → Preview corners → Viewport handles → drag a cyan corner → Apply corners. Escape / Cancel handles restores the pre-handle draft."
    )]
    pub(in crate::mcp) fn overlay_corner_handles(
        &self,
        Parameters(args): Parameters<OverlayCornerHandleArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                OverlayCornerHandleArgs::Inspect => Ok(session.overlay_corner_handle_state()),
                OverlayCornerHandleArgs::Begin {
                    token,
                    width,
                    height,
                    camera,
                    snapshot,
                } => {
                    let camera = camera
                        .unwrap_or_default()
                        .resolve(&session, width as f32 / height as f32)?;
                    session.begin_overlay_corner_handles(
                        Actor::Agent,
                        token,
                        camera,
                        (width, height),
                        &snapshot,
                    )
                }
                OverlayCornerHandleArgs::Pick {
                    token,
                    x,
                    y,
                    width,
                    height,
                } => session.pick_overlay_corner_handle(
                    Actor::Agent,
                    token,
                    crate::session::Pointer {
                        position: glam::DVec2::new(x, y),
                        size: (width, height),
                    },
                ),
                OverlayCornerHandleArgs::Move {
                    token,
                    x,
                    y,
                    width,
                    height,
                } => session.move_overlay_corner_handle(
                    Actor::Agent,
                    token,
                    crate::session::Pointer {
                        position: glam::DVec2::new(x, y),
                        size: (width, height),
                    },
                ),
                OverlayCornerHandleArgs::Finish { token } => {
                    session.finish_overlay_corner_handles(Actor::Agent, token)
                }
                OverlayCornerHandleArgs::Cancel { token } => {
                    session.cancel_overlay_corner_handles(Actor::Agent, token)
                }
            }
        })
    }

    #[tool(
        description = "Preview four perimeter-ordered local U/V corner positions on one existing visible info_overlay. id defaults to the selected overlay. Inspect get_overlay_state.selected_corners for current local_corners and basis; pass its snapshot. corners is exactly four [u,v] world-unit pairs in the EXISTING BasisU/BasisV frame, separate from texture StartU/EndU ranges. Existing binding must be one eligible world/func_detail quad and its projection must lie on that plane; moved sources require explicit refit first. Rejects nonfinite, nonconvex, collapsed, out-of-world/native-float-invalid corners without replacing a valid draft. Does not refit or rebind. Preserves ID, raw basis/origin/sides/settings, connections and metadata; Apply changes uv0–uv3 XY in one Undo step. Corners may extend beyond the source perimeter and are clipped by native rendering. UI: select overlay → Overlay… → Edit selected corners → Preview corners → Apply corners."
    )]
    pub(in crate::mcp) fn preview_overlay_corners(
        &self,
        Parameters(args): Parameters<OverlayCornersArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.preview_overlay_corners(Actor::Agent, id, args.corners, &args.snapshot)
        })
    }

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
        description = "Inspect shared overlay creation, existing-binding refit, explicit new-face rebind, numeric local corner editing and viewport corner handles: current snapshot, selected source/overlay eligibility and rejection reasons, draft operation/target/owner/token/validity, settings, corners and bounds. Preview changes no map, selection or history. Create adds an entity; Refit, Rebind and Corners update the same entity projection without changing its authored settings."
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
        description = "Fit one new overlay to source {solid,side}, an explicit visible planar world/func_detail quad, using latest get_overlay_state.snapshot. Optional additional_sources is a list of distinct coplanar flat world/detail faces facing the same direction (64 total source faces maximum). The first source supplies the existing projection axes; the combined U/V rectangle covers all real face loops, including gaps, and native rendering clips to the exact selected side IDs. Omitting the list preserves the original single-quad fit exactly. Rejects hidden/unsupported/missing/noncoplanar/terrain/model faces atomically. material is a material path without materials/ or extension; targetname is optional. render_order defaults 0, u/v to [0,1], fade_min/fade_max to -1/0. Valid preview replaces the shared draft and owns its token; invalid or stale input preserves the last valid draft. Multiple-source drafts report sources and source_count. Map, selection and Undo stay unchanged until Apply. Refit/rebind/corners still require one existing bound quad. UI: select coplanar faces → Overlay… → Fit / Preview."
    )]
    pub(in crate::mcp) fn preview_overlay(
        &self,
        Parameters(args): Parameters<OverlayPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let additional_sources = args
                .additional_sources
                .unwrap_or_default()
                .into_iter()
                .map(|source| map_core::surface::FaceId {
                    solid: source.solid,
                    side: source.side,
                })
                .collect::<Vec<_>>();
            self.session().preview_overlay_faces(
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
                &additional_sources,
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Explicitly review and take ownership of the current valid overlay draft. Returns a new token, invalidating the previous owner's token. Human Review and this tool use the same Session operation. Reviewing active corner handles closes the gesture and adopts its last valid corners; foreign drafts cannot be applied or cancelled until reviewed."
    )]
    pub(in crate::mcp) fn review_overlay(
        &self,
        Parameters(args): Parameters<OverlayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().review_overlay(Actor::Agent, args.token))
    }

    #[tool(
        description = "Apply the current owned overlay draft in one atomic Undo step. draft.operation=create returns created and selects a new info_overlay; operation=refit/rebind/corners returns updated and selects the same existing entity, preserving authored option strings, connections and metadata. Rejects stale/replaced/foreign tokens, changed map or selection, and active geometry previews or corner handles. Source brush geometry stays unchanged. Corners preserves the existing basis and binding; fitted operations generate the same projection used for Preview."
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
