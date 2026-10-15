use super::super::*;

#[tool_router(router = placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared repeat-array preview, snapshot, generated names, copied entity fields, remapped-link counts, bounds and limits. Count excludes originals. render_view shows this draft without changing source geometry/history. New targetnames and certain exact internal FGD/I/O/parent links are remapped per copy; external links remain unchanged. Shared names, ambiguous internal references, wildcard captures of generated names and overlay face-ID references reject. Context notes retain authorship/observations and full budgets; saved partitions stay on originals. Custom scripts/opaque reference semantics are preserved verbatim and are outside the supported remapping policy."
    )]
    pub(in crate::mcp) fn get_repeat_array(&self) -> ToolResult {
        run(|| Ok(self.session().repeat_array()))
    }

    #[tool(
        description = "Preview bounded repeated placement using the latest get_repeat_array.snapshot. Copy n uses the committed originals, n × XYZ world rotation about one fixed pivot, then n × world translation; defaults to source center pivot and attached textures. Count is 1–64 additional copies, at most 16 sources, 256 generated objects including owned brushes and 8192 sides, within ±65536. Names become unique name_copyN values, exact internal links follow each copy, external links remain. Ambiguous source names/links and new-name reference acquisitions reject atomically. Invalid updates retain the last valid preview. UI: Repeat array… → Preview array. Inspect get_repeat_array and render_view, then apply_repeat_array or cancel_repeat_array with the returned token."
    )]
    pub(in crate::mcp) fn preview_repeat_array(
        &self,
        Parameters(args): Parameters<RepeatArrayArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let pivot = match args.pivot {
                Some(pivot) => vec3(pivot),
                None => session.selection_pivot(&ids)?,
            };
            session.preview_repeat_array(
                &ids,
                map_core::repeat::Options {
                    count: args.count,
                    translation: args.translation.map(vec3).unwrap_or(DVec3::ZERO),
                    rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                    pivot,
                    texture_lock: args.texture_lock.unwrap_or(true),
                },
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
