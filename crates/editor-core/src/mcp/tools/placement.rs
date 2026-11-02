use super::super::*;

#[tool_router(router = placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared repeat-array preview, snapshot, generated names, copied entity fields, remapped-link counts, bounds, limits and fixed world/source-local frame. preview.request records pivot intent separately from preview.pivot, the resolved world point. last_settings contains the last successfully applied request or null; it retains steps and pivot intent without old source IDs or captured axes. Count excludes originals. render_view shows this draft without changing source geometry/history. New targetnames and certain exact internal FGD/I/O/parent links are remapped per copy; external links remain unchanged. Shared names, ambiguous internal references, wildcard captures of generated names and overlay face-ID references reject. Context notes retain authorship/observations and full budgets; saved partitions stay on originals. Custom scripts/opaque reference semantics are preserved verbatim and are outside the supported remapping policy. handle contains kind=repeat_array and a decimal-string token, or null when closed or stale; snapshot remains the source guard."
    )]
    pub(in crate::mcp) fn get_repeat_array(&self) -> ToolResult {
        run(|| Ok(self.session().repeat_array()))
    }

    #[tool(
        description = "Preview bounded repeated placement using the latest get_repeat_array.snapshot. Copy n uses the committed originals, scales each XYZ axis by its per-copy factor raised to n, then rotates by n × XYZ degrees about one fixed world pivot, then translates by n × the step. All steps use the chosen frame. Scale factors are finite and positive, default [1,1,1]; nonidentity scaling supports brushes and brush entities only, including checked terrain. Point/model entity scaling rejects. Keep textures attached also stretches their projection with scaling. space is world (default) or local: local captures the first original source in stable object-ID order (brushes before entities), using its existing transform frame once for every copy. The frame never follows earlier copies. pivot_source accepts selection_center, current_transform_pivot or custom_world_point. Omitted source with no pivot retains the selection-center default; a legacy explicit pivot means custom_world_point. Custom source requires pivot; explicit selection/current sources reject pivot coordinates. Current transform pivot resolves attached anchors through the shared Session resolver and rejects unavailable anchors without fallback. The resolved point is captured once for the whole preview. Textures default attached. Count is 1–64 additional copies, at most 16 sources, 256 generated objects including owned brushes and 8192 sides, within ±65536. Names become unique name_copyN values, exact internal links follow each copy, external links remain. Ambiguous source names/links and new-name reference acquisitions reject atomically. Invalid updates retain the last valid preview. UI: Repeat array… → World / Source local → Preview array. Inspect get_repeat_array and render_view, then apply_repeat_array or cancel_repeat_array with the returned token. Returns handle with kind=repeat_array and decimal-string token, separate from snapshot. Changed selection and competing geometry previews invalidate it; repeating the current object selection preserves it. Closed or stale reports have handle=null."
    )]
    pub(in crate::mcp) fn preview_repeat_array(
        &self,
        Parameters(args): Parameters<RepeatArrayArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let local = match args.space.as_deref().unwrap_or("world") {
                "world" => false,
                "local" => true,
                _ => return Err("space must be world or local".into()),
            };
            session.preview_repeat_array_request(
                &ids,
                crate::session::repeat::Request {
                    count: args.count,
                    translation: args.translation.map(vec3).unwrap_or(DVec3::ZERO),
                    rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                    scale: args.scale.map(vec3).unwrap_or(DVec3::ONE),
                    pivot_source: args.pivot_source,
                    pivot: args.pivot.map(vec3),
                    local,
                    texture_lock: args.texture_lock.unwrap_or(true),
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Create a fresh repeat-array preview from get_repeat_array.last_settings using the current selection and latest snapshot. Settings are captured only by successful Apply array, not by preview or cancellation. Re-resolves selection-center/current-transform-pivot intent and source-local axes; never reuses old object IDs or an old frame. A custom world point remains fixed. Missing saved settings, unavailable attached pivots, stale snapshots or invalid current sources reject without replacing a valid preview. Inspect get_repeat_array/render_view, then apply_repeat_array with the fresh token. UI: Repeat array… → Use last array settings → Preview array."
    )]
    pub(in crate::mcp) fn preview_last_repeat_array(
        &self,
        Parameters(args): Parameters<PreviewLastRepeatArrayArgs>,
    ) -> ToolResult {
        run(|| self.session().preview_last_repeat_array(&args.snapshot))
    }

    #[tool(
        description = "Apply the exact repeat-array token in one atomic Undo step, verifying generated geometry, IDs, names, links and metadata against the preview. Selects all new top-level objects and remembers the applied steps/pivot intent for a later fresh preview. Preview/cancel does not replace last_settings. Stale/replaced tokens, changed maps or reloaded entity definitions reject without modifying the map."
    )]
    pub(in crate::mcp) fn apply_repeat_array(
        &self,
        Parameters(args): Parameters<RepeatArrayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_repeat_array(Actor::Agent, args.token))
    }

    #[tool(
        description = "Discard the exact repeat-array token without changing source geometry or Undo history. A token for an old array cannot discard another array, numeric transform or primitive preview. UI: Repeat array… → Cancel, Escape or close."
    )]
    pub(in crate::mcp) fn cancel_repeat_array(
        &self,
        Parameters(args): Parameters<RepeatArrayTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_repeat_array(args.token)?;
            Ok(session.repeat_array())
        })
    }
}
