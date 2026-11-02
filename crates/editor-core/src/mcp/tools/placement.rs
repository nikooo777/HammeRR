use super::super::*;

#[tool_router(router = placement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared repeated-placement preview, source snapshot, generated names, reference counts, copy poses, bounds and limits. preview.kind distinguishes an array from surface scatter. render_view shows the draft before Apply. The handle uses kind=repeat_array and a decimal-string token, or null when closed or stale. last_settings retains only the last successfully applied array request, without old source IDs or captured axes."
    )]
    pub(in crate::mcp) fn get_repeat_array(&self) -> ToolResult {
        run(|| Ok(self.session().repeat_array()))
    }

    #[tool(
        description = "Preview an array using get_repeat_array.snapshot. Copy n scales the originals by factor^n, rotates by n times the degree step around one captured pivot, then translates by n times the move step. World or source-local axes stay fixed for all copies. Positive XYZ factors scale brushes and plain point origins; authored MDL models require uniform factors and native modelscale within 1/64–64. Static/detail model resizing refuses. Naming and internal/external reference policies share Session validation. Defaults preserve unique name_copyN names, remapped exact internal links and unchanged external links. Ambiguous links and generated-name reference acquisition refuse. Invalid requests preserve the previous draft. Inspect get_repeat_array/render_view, then apply_repeat_array or cancel_repeat_array with its token. UI: Repeat array… → Preview array."
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
                    policies: args.policies,
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Preview deterministic surface scatter using the latest get_repeat_array.snapshot and captured document_generation as source_generation. Each copy repeats the explicit whole source assembly. Samples authored brush-face or prepared terrain triangles by actual area; faces are {solid,side}. Optional alignment maps the first stable source's local +Z to the sampled normal, then heading rotates around that normal; unaligned heading uses world Z. Each copy chooses one positive uniform factor from scale. The captured source pivot maps to the sample plus normal_offset. Shared naming/reference policies and modelscale limits apply. Invalid targets, stale IDs or unsupported source transforms refuse without replacing the previous draft. No model/instance surfaces, collision settling or overlap guarantee. Inspect get_repeat_array/render_view, then use apply_repeat_array or cancel_repeat_array with the shared token."
    )]
    pub(in crate::mcp) fn preview_surface_scatter(
        &self,
        Parameters(args): Parameters<SurfaceScatterArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let sources = ids_or_selection(&session, Some(&args.sources))?;
            let faces = args
                .faces
                .iter()
                .map(|face| map_core::surface::FaceId {
                    solid: face.solid,
                    side: face.side,
                })
                .collect::<Vec<_>>();
            session.preview_surface_scatter(
                &sources,
                &faces,
                crate::session::repeat::ScatterRequest {
                    count: args.count,
                    seed: args.seed,
                    align_normal: args.align_normal,
                    heading: args.heading,
                    scale: args.scale,
                    normal_offset: args.normal_offset,
                    texture_lock: args.texture_lock,
                    pivot_source: args.pivot_source,
                    pivot: args.pivot.map(vec3),
                    policies: args.policies,
                    source_generation: args.source_generation,
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Create a fresh array preview from get_repeat_array.last_settings using the current selection and latest snapshot. Only an applied array updates these settings; preview, cancellation and scatter do not. Re-resolves pivot intent and source-local axes without old source IDs or a captured frame. Missing settings, unavailable anchors, stale snapshots or invalid sources refuse without replacing a valid preview. Inspect get_repeat_array/render_view, then apply_repeat_array with the fresh token. UI: Repeat array… → Use last array settings → Preview array."
    )]
    pub(in crate::mcp) fn preview_last_repeat_array(
        &self,
        Parameters(args): Parameters<PreviewLastRepeatArrayArgs>,
    ) -> ToolResult {
        run(|| self.session().preview_last_repeat_array(&args.snapshot))
    }

    #[tool(
        description = "Apply the exact array or surface-scatter token in one atomic Undo step, checking geometry, poses, IDs, names, links and metadata against the preview. Selects the generated objects. Applied arrays remember their settings; scatter leaves last_settings unchanged. Stale/replaced tokens, changed maps or reloaded definitions refuse without modifying the map."
    )]
    pub(in crate::mcp) fn apply_repeat_array(
        &self,
        Parameters(args): Parameters<RepeatArrayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_repeat_array(Actor::Agent, args.token))
    }

    #[tool(
        description = "Discard the exact array or surface-scatter token without changing source geometry or Undo history. An old token cannot discard a newer draft. UI: Repeat array… or Surface scatter… → Cancel, Escape or close."
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
