use super::super::*;

#[tool_router(router = partitions_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Stage two flat loops between any two distinct boundary-edge midpoints on a convex face with 3–8 corners, one selected face per brush across 1–16 ordinary brushes. pairs uses the complete get_brush_components.partition_choices and the latest snapshot. Quads retain two opposing and four adjacent choices with their original ordering; opposing quad cuts produce two quads and adjacent quad cuts produce a triangle and pentagon. Polygon cuts produce two checked convex loops with 3–9 vertices. Keeps independent draft vertex/loop IDs, exact authored planes, corner and edge incidence, ownership and source-side inheritance; never adds duplicate VMF sides or changes source history/allocators. Inspect preview.partition through get_brush_components, select loops with select_component_partitions and frame them with frame_component_partitions. Keep flat pieces with store_component_partition, or raise a shared ridge for two quad pieces with raise_component_partition and Apply through commit_component_edit. Generic flat commit is rejected while retaining the draft. Save/compile/play and map replacement without explicit discard are blocked until Keep, Apply or Cancel. Inspect/resume/remove kept pieces through the saved-partition tools. Cancel with cancel_component_edit."
    )]
    pub(in crate::mcp) fn preview_component_partition(
        &self,
        Parameters(args): Parameters<ComponentPartitionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_component_partition(&args.pairs, &args.snapshot)?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Update a staged flat partition's shared ridge height from its immutable source. Zero returns to the flat draft without allocating VMF IDs; positive 0.125–4096 raises both captured cut endpoints along the face normal through checked planar, convex crease geometry. Unequal widths are supported when both roofs remain planar; nonplanar cuts reject without resetting the parameters. Two quad pieces share the ridge regardless of highlight selection. Cuts whose pieces are not both quads reject positive ridge heights; keep or extrude them. preview.partition.ridge_supported reports this shape restriction. Source corners, side/solid IDs, owners and neighboring materials/world UVs remain. Planned top materials apply to corresponding roofs. Invalid updates retain the last valid draft; returns a replacement token. Apply with commit_component_edit (positive rise only), or cancel_component_edit. Rejects stale tokens, unsupported sources, nonplanar roofs, neighboring-plane crossings and bounds/face limits atomically."
    )]
    pub(in crate::mcp) fn raise_component_partition(
        &self,
        Parameters(args): Parameters<RaisePartitionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.raise_component_partition_with_texture(
                parse_token(&args.token)?,
                args.rise,
                args.roof_texture_lock,
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Adjust a flat partition's two cut endpoints along its captured source boundary edges. Requires the current token, zero ridge height and no extrusion draft; Return to flat first. edge_parameters are two finite values strictly between zero and one, applied to every source brush. Edges follow sorted neighboring side IDs; each parameter runs between the edge's sorted corner incidence lists, reported in preview.partition.vertices[].source.corners. Defaults are [0.5,0.5]. Produces unequal/diagonal convex pieces while retaining source planes, owners, saved piece identity, highlights and history. Invalid, collapsed or unsupported results retain the last valid draft. Returns a replacement token. preview.partition.edge_parameters contains shared values or null for mixed saved sources; vertex provenance reports each exact value. Keep with store_component_partition, extrude with preview_partition_extrusion, frame/pick with the existing partition tools or Cancel. Saved parameters survive affine edits, reflection and reopening. Shared ridge previews accept adjusted cuts that produce two planar convex quad roofs; other cuts reject without snapping or losing the draft."
    )]
    pub(in crate::mcp) fn update_component_partition_cut(
        &self,
        Parameters(args): Parameters<PartitionCutArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session
                .update_component_partition_cut(parse_token(&args.token)?, args.edge_parameters)?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Set the planned top material on 1–32 distinct pieces of the current flat partition draft. Supply current token and explicit partitions [{solid,partition}]; material is a Source material name, or null/omitted to restore inheritance from the source face. Return to flat before changing materials. Shares the human material controls. Returns a replacement token and explicit/effective material per loop. Invalid, stale or over-budget updates retain the previous draft. Keep persists bounded editor intent without changing source faces; Cancel stores nothing. Raising applies the plan to corresponding roof faces; extrusion applies it only to prism tops, retaining original bases and wall policy. Texture placement uses independent piece plans or source inheritance; other surface settings retain the source policy."
    )]
    pub(in crate::mcp) fn update_component_partition_material(
        &self,
        Parameters(args): Parameters<PartitionMaterialArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_component_partition_material(
                parse_token(&args.token)?,
                &args.partitions,
                args.material.as_deref(),
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Set independent top texture placement on 1–32 distinct flat partition pieces through the shared human controls. Supply the current token, partitions [{solid,partition}], and texture {shift:[u,v],scale:[u,v],rotation:degrees}; null or omitted restores source inheritance. Shifts are texels, scales are world units per texel, Rotation is an absolute angle; its difference from the source rotation turns the inherited projection axes. All values must be finite within ±65536; scale magnitude must be at least 1e-8, with negative scales allowing mirroring. Returns a replacement token, explicit texture_plan, effective_texture and texture_inherited for every loop. Invalid, duplicate, stale and over-budget changes retain the last valid preview. Return to flat before editing. Keep persists bounded intent and leaves source faces unchanged; Cancel stores nothing. Raised roofs and prism tops use each planned projection, with optional corner attachment; bases and walls retain their own policy. Material, lightmap and smoothing settings are independent."
    )]
    pub(in crate::mcp) fn update_component_partition_texture(
        &self,
        Parameters(args): Parameters<PartitionTextureArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_component_partition_texture(
                parse_token(&args.token)?,
                &args.partitions,
                args.texture
                    .map(|texture| map_core::subobject::PartitionTexture {
                        shift: texture.shift,
                        scale: texture.scale,
                        rotation: texture.rotation,
                    }),
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Fit or justify the existing effective texture projection independently on 1–32 distinct flat partition loops using each piece's actual corners and effective material dimensions. Supply current token, partitions [{solid,partition}], fit:true and/or justify_u/justify_v: start, center or end (horizontal left/center/right; vertical top/center/bottom). Fit maps one tile over each loop's projected bounds, preserving rotation, axes and signed mirroring; justification shifts a chosen bound or center to that material tile and preserves scale. Fit runs before justification when combined. Without Fit, omitted axes preserve their shifts. Supports triangles, pentagons and all current convex adjusted loops; source face bounds are never substituted. Uses the same Session operation as Fit each piece and Preview justification. Missing dimensions, empty/duplicate/stale refs, invalid projections, raised/extruded scopes and metadata-budget failures retain the previous draft and token. Returns replacement token and effective_texture/texture_plan per loop. Review the flat preview, then Keep or raise/extrude and Apply; Cancel discards changes. Other plans, materials, lighting, source geometry and history remain intact. Explicit projection-axis authoring is unavailable."
    )]
    pub(in crate::mcp) fn place_component_partition_texture(
        &self,
        Parameters(args): Parameters<PlacePartitionTextureArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.place_component_partition_texture(
                parse_token(&args.token)?,
                &args.partitions,
                args.fit.unwrap_or(false),
                [
                    args.justify_u.map(Into::into),
                    args.justify_v.map(Into::into),
                ],
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Preview outward extrusion of the highlighted flat partition pieces into separate convex brushes. Requires a current flat-partition token; select 1–32 pieces using select_component_partitions across 1–16 source brushes. height is 0.125–4096 world units along each original face normal. Source brushes and saved partition intents remain unchanged. World sources add world children; brush-entity sources add children to the same entity. Each prism has fresh solid/side IDs, a top using its planned material and texture placement or source inheritance, a base inheriting the original source face, three boundary walls inheriting neighboring faces, and a divider inheriting the source face. Walls receive new world UV axes; optional wall_material overrides all four walls. texture_lock defaults false; true attaches only top UVs to corresponding corners. Inspect preview.extrusion for allocated IDs, provenance, owners, bounds and all face planes/materials/UVs. Updates rebuild from the original source and replace the token; invalid updates retain the last valid preview. Frame and pick the visible prisms through the existing partition tools. Changing highlights requires another preview before Apply. commit_component_edit verifies exact replay, adds all volumes in one undo step and selects their faces. Cancel discards geometry and allocations. raise_component_partition with zero returns to flat. Save/build/play remain guarded until Apply or Cancel."
    )]
    pub(in crate::mcp) fn preview_partition_extrusion(
        &self,
        Parameters(args): Parameters<ExtrudePartitionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_partition_extrusion(
                parse_token(&args.token)?,
                map_core::subobject::ExtrusionSpec {
                    height: args.height,
                    wall_material: args.wall_material,
                    texture_lock: args.texture_lock.unwrap_or(false),
                },
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Inspect saved flat face partitions, current validity, source corner counts, explicit polygon original-corner membership and bounded metadata limits. Missing or changed sources remain listed with errors. Read-only; snapshot guards resume and remove operations.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_saved_component_partitions(&self) -> ToolResult {
        run(|| Ok(self.session().saved_partition_report()))
    }

    #[tool(
        description = "Keep a flat partition draft as bounded VMF editor metadata in one undoable edit. Requires its current token and zero rise. Closes the draft, permits save/compile/play, and leaves brush planes and compiler geometry unchanged. Resume later to inspect, highlight, frame, extrude or raise supported two-quad pieces. Polygon sources retain explicit original corner membership for both pieces; legacy quad kinds retain their format."
    )]
    pub(in crate::mcp) fn store_component_partition(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let count =
                session.store_component_partition(Actor::Agent, parse_token(&args.token)?)?;
            Ok(json!({"stored":count,"saved":session.saved_partition_report()}))
        })
    }

    #[tool(
        description = "Resume valid saved flat partitions identified by sources [{solid,side}] and the latest saved-partition snapshot. One source per brush across 1–16 brushes. Restores a shared flat draft for viewport/list inspection, picking, highlighting, framing, independent extrusion, supported two-quad raising, keeping or cancellation. No geometry/history mutation; reject pending previews and stale or missing source records atomically."
    )]
    pub(in crate::mcp) fn resume_component_partitions(
        &self,
        Parameters(args): Parameters<SavedPartitionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let sources = args
                .sources
                .iter()
                .map(|source| (source.solid, source.side))
                .collect::<Vec<_>>();
            session.resume_component_partitions(&sources, &args.snapshot)?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Remove 1–64 saved partition records by sources [{solid,side}] and the latest saved-partition snapshot in one undoable edit. Works for records whose geometry was deleted or changed. Brush planes remain unchanged. Reject pending previews, unknown sources and unsupported metadata atomically."
    )]
    pub(in crate::mcp) fn remove_saved_component_partitions(
        &self,
        Parameters(args): Parameters<SavedPartitionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let sources = args
                .sources
                .iter()
                .map(|source| (source.solid, source.side))
                .collect::<Vec<_>>();
            let count = session.remove_saved_component_partitions(
                Actor::Agent,
                &sources,
                &args.snapshot,
            )?;
            Ok(json!({"removed":count,"saved":session.saved_partition_report()}))
        })
    }

    #[tool(
        description = "Select draft loops by {solid,partition} from preview.partition.partitions[].loops[].id using replace/add/remove/toggle. Ridge raising affects both pieces; extrusion uses highlighted pieces. Changing highlights during an extrusion requires another preview before Apply. Does not alter source component selection, VMF geometry or history. Requires the current component preview token."
    )]
    pub(in crate::mcp) fn select_component_partitions(
        &self,
        Parameters(args): Parameters<SelectPartitionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.select_component_partitions(
                parse_token(&args.token)?,
                &args.partitions,
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Frame highlighted flat or raised partition loops, including extruded prism bounds, through the shared viewport camera operation. Requires the current component preview token and at least one highlighted loop. aspect defaults to 1.6. Leaves source geometry and history unchanged."
    )]
    pub(in crate::mcp) fn frame_component_partitions(
        &self,
        Parameters(args): Parameters<FramePartitionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.frame_component_partitions(
                Actor::Agent,
                parse_token(&args.token)?,
                args.aspect.unwrap_or(1.6),
            )?;
            Ok(
                json!({"camera":camera_json(&session.camera()),"components":selected_component_report(&session)?}),
            )
        })
    }

    #[tool(
        description = "Read staged flat, raised or extruded partition pieces under a viewport pixel through the same Session query as a UI click. Requires the current draft token; x/y are continuous pixels from top-left, with the camera and size of render_view. Uses the nearest visible brush surface, hidden ownership and shared tool filters. Visible extruded bodies map to their original piece refs. Returns partition refs, world hit positions and camera depth; flat/raised hits within half a pixel of a shared boundary may contain both pieces ordered by depth then solid/piece ID. Read-only. Pass a chosen ref and the same token to select_component_partitions; ordinary UI clicks replace highlights, Ctrl toggles, empty ordinary clicks clear. Source geometry, source selection, draft token and undo history stay unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_component_partitions(
        &self,
        Parameters(args): Parameters<PartitionPixelArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let picks = session.pick_component_partitions(
                parse_token(&args.token)?,
                camera,
                crate::session::Pointer {
                    position: DVec2::new(args.x, args.y),
                    size,
                },
            )?;
            Ok(
                json!({"token":args.token,"camera":camera_json(&camera),"width":size.0,"height":size.1,"total":picks.len(),
                "candidates":picks.iter().map(|pick|json!({"partition":{"solid":pick.partition.solid,"partition":pick.partition.partition},"position":pick.position.to_array(),"depth":pick.depth})).collect::<Vec<_>>() }),
            )
        })
    }
}
