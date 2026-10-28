use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCompiledPropLeafBoundsArgs {
    #[schemars(
        description = "Exact owned build token from inspect_baked_prop_vertices. Required when prop is present; omit when clearing."
    )]
    build_token: Option<String>,
    #[schemars(
        description = "Zero-based native static-prop ordinal. Omit or null to clear only the prop leaf bounds guide."
    )]
    prop: Option<u32>,
}

#[tool_router(router = prop_leaf_bounds_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared compiled static-prop leaf bounds guide: requested/displayed state, exact build token/prop, native SPRP first_leaf and leaf_count, complete native leaf ordinals/bounds and resource limits. Stale, replaced or unavailable output suppresses display with an explicit reason. A separate purple guide; compiled visibility guides remain independent. Bounded original-source metadata checks; no graphics initialization or model geometry reload.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_compiled_prop_leaf_bounds(&self) -> ToolResult {
        ok(self.session().compiled_prop_leaf_bounds())
    }

    #[tool(
        description = "Share Baked prop vertices → Show leaf bounds / Clear leaf bounds. To show, require exact build_token and zero-based prop ordinal from inspect_baked_prop_vertices. Reads the same owned BSP and validated native SPRP first_leaf/leaf_count/leaf table, then shows ALL membership leaf bounds, including disabled props or builds without vertex baking. Complete sets above 512 leaves or 256 KiB bounds/renderer storage, malformed ranges/bounds, stale/replaced output and active viewport work refuse before replacing the previous valid guide. No truncation, model geometry reload, exact mesh clipping or runtime-visibility claim. Purple native boxes coexist with the independent cyan visibility guide. Preserves camera, selection, map/history/settings and ordinary drafts. Omit/null prop and omit build_token to clear this guide, including a suppressed guide. No arbitrary file or game."
    )]
    pub(in crate::mcp) fn set_compiled_prop_leaf_bounds(
        &self,
        Parameters(args): Parameters<SetCompiledPropLeafBoundsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .set_compiled_prop_leaf_bounds(Actor::Agent, args.build_token, args.prop)
        })
    }
}
