use super::super::*;

#[tool_router(router = entity_gallery_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Browse every loaded FGD pointentity class in explicit 32-class pages, with total matches, defaults, helper declarations and placement availability. Search matches class names/descriptions. Surface-only classes and instances show explicit unsupported reasons. Does not create entities or change previews. UI: Entity → Point-entity gallery….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryBrowseArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().browse_entity_gallery(
                args.search.as_deref().unwrap_or_default(),
                args.page.unwrap_or_default(),
            )
        })
    }

    #[tool(
        description = "Inspect the shared point-entity gallery preview: owned token, document/FGD validity, arrangement, each example's exact properties/origin/bounds and helper/model/icon status and errors. Actual examples use render_view's shared preview scene; source map and Undo remain unchanged until Apply. Missing assets remain explicitly unavailable. Repeated static helper catalogues are omitted; get_entity_inspector provides full helper details for placed examples. UI: Point-entity gallery….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_gallery(&self) -> ToolResult {
        ok(self.session().get_entity_gallery())
    }

    #[tool(
        description = "Preview 1–32 explicit distinct loaded FGD point classes as actual editor examples, using shared entity creation/defaults/property validation. Row-major XY cells fit model/helper/icon bounds plus gap; origin is first cell center and bottom Z. Per-class properties may supply real model paths. No classes or missing assets are silently skipped. Competing geometry drafts must finish first. Source/history/selection stay unchanged; save/compile/play refuse pending gallery drafts. Render/frame and inspect diagnostics, then Apply or Cancel. UI: Point-entity gallery… → Preview selected."
    )]
    pub(in crate::mcp) fn preview_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().preview_entity_gallery(
                Actor::Agent,
                crate::session::entity_gallery::Request {
                    examples: args
                        .examples
                        .into_iter()
                        .map(|example| crate::session::entity_gallery::Example {
                            classname: example.classname,
                            properties: example.properties.unwrap_or_default(),
                        })
                        .collect(),
                    origin: vec3(args.origin),
                    columns: args.columns.unwrap_or(4),
                    gap: args.gap.unwrap_or(64.0),
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Review and take ownership of the valid shared gallery draft. Returns a new token; previous owner's tokens reject. UI: Review shared gallery."
    )]
    pub(in crate::mcp) fn review_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .review_entity_gallery(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Frame the matching valid gallery preview, or one zero-based example, in the shared viewport camera. No source map/history/selection changes. Use render_view to inspect actual mounted helpers/models/icons. UI: Frame preview / each example's Frame."
    )]
    pub(in crate::mcp) fn frame_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.frame_entity_gallery(args.token, args.index, args.aspect.unwrap_or(1.5))?;
            Ok(session.get_entity_gallery())
        })
    }

    #[tool(
        description = "Apply the exact owned valid gallery preview in one atomic Undo step and select all created examples. Replays existing entity creation and verifies exact IDs/properties against the reviewed preview. Stale maps/FGD/scene, foreign or replaced tokens reject; any replay failure rolls the complete edit back. UI: Place reviewed examples."
    )]
    pub(in crate::mcp) fn apply_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_entity_gallery(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel the matching owned gallery draft, including stale cleanup. Source map, selection and Undo history stay unchanged. Foreign/replaced tokens cannot clear replacement previews. UI: Cancel preview / Escape / close gallery."
    )]
    pub(in crate::mcp) fn cancel_entity_gallery(
        &self,
        Parameters(args): Parameters<EntityGalleryTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_entity_gallery(Actor::Agent, args.token)?;
            Ok(session.get_entity_gallery())
        })
    }
}
