use super::super::*;

#[tool_router(router = placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared repeat-array preview, snapshot, generated names, copied entity fields, remapped-link counts, bounds, limits and fixed world/source-local frame. Count excludes originals. render_view shows this draft without changing source geometry/history. New targetnames and certain exact internal FGD/I/O/parent links are remapped per copy; external links remain unchanged. Shared names, ambiguous internal references, wildcard captures of generated names and overlay face-ID references reject. Context notes retain authorship/observations and full budgets; saved partitions stay on originals. Custom scripts/opaque reference semantics are preserved verbatim and are outside the supported remapping policy."
    )]
    pub(in crate::mcp) fn get_repeat_array(&self) -> ToolResult {
        run(|| Ok(self.session().repeat_array()))
    }

    #[tool(
        description = "Preview bounded repeated placement using the latest get_repeat_array.snapshot. Copy n uses the committed originals, rotates by n × XYZ degrees in the chosen frame about one fixed world pivot, then translates by n × the step in that same frame. space is world (default) or local: local captures the first original source in stable object-ID order (brushes before entities), using its existing transform frame once for every copy. The frame never follows earlier copies. Defaults to source center pivot and attached textures. Count is 1–64 additional copies, at most 16 sources, 256 generated objects including owned brushes and 8192 sides, within ±65536. Names become unique name_copyN values, exact internal links follow each copy, external links remain. Ambiguous source names/links and new-name reference acquisitions reject atomically. Invalid updates retain the last valid preview. UI: Repeat array… → World / Source local → Preview array. Inspect get_repeat_array and render_view, then apply_repeat_array or cancel_repeat_array with the returned token."
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
            let pivot = match args.pivot {
                Some(pivot) => vec3(pivot),
                None => session.selection_pivot(&ids)?,
            };
            session.preview_repeat_array_in_space(
                &ids,
                map_core::repeat::Options {
                    count: args.count,
                    translation: args.translation.map(vec3).unwrap_or(DVec3::ZERO),
                    rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                    pivot,
                    basis: glam::DMat3::IDENTITY,
                    texture_lock: args.texture_lock.unwrap_or(true),
                },
                local,
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Apply the exact repeat-array token in one atomic Undo step, verifying generated geometry, IDs, names, links and metadata against the preview. Selects all new top-level objects. Stale/replaced tokens, changed maps or reloaded entity definitions reject without modifying the map."
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
