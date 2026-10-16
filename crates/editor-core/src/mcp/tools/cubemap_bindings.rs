use super::super::*;

#[tool_router(router = cubemap_bindings_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect compiler-authoritative cubemap face assignments without capturing reflections. Optional helper is an env_cubemap entity's numeric ID. query filters material/side IDs; offset defaults 0 and limit 50 (maximum 256). Reports manual/automatic choices, exact FaceIds, chosen integer sample positions, source materials, generated aliases, hidden/excluded helpers and actionable diagnostics. state includes the latest snapshot and shared draft owner/token/validity. Missing content or malformed bindings appear in diagnostics with no fabricated assignments. Unnamed maps use a provisional namespace. Explicit refresh can be expensive; routine UI refresh uses cached rows. UI: select env_cubemap → Cubemap faces… → Refresh.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_cubemap_bindings(
        &self,
        Parameters(args): Parameters<CubemapBindingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .get_cubemap_bindings(crate::session::cubemap_bindings::Query {
                    helper: args.helper,
                    query: args.query.unwrap_or_default(),
                    offset: args.offset.unwrap_or(0),
                    limit: args.limit.unwrap_or(50),
                })
        })
    }

    #[tool(
        description = "Shared Cubemap faces property draft. action=preview requires explicit numeric helper ID, faces [{solid,side}], operation bind|remove and latest get_cubemap_bindings.state.snapshot. Bind adds standard env_cubemap sides without taking another helper's manual ownership. Remove requires those faces belong to this helper; they then use the compiler's automatic sample, potentially another active helper. To clear all manual bindings, explicitly pass the helper's complete manual_faces list from inspection with operation remove. Preview validates the authoritative compiler plan and changes no source, selection or history; invalid input preserves the current draft. action=review with token transfers ownership and returns a new token. action=apply commits an owned valid token in one Undo step after planner revalidation. action=cancel cancels only the matching owned token, including stale cleanup. Changed map, selection, output namespace, content or replaced/foreign tokens reject. UI: Cubemap faces… → Bind selected faces / Remove selected bindings / Clear all manual bindings → Apply."
    )]
    pub(in crate::mcp) fn set_cubemap_bindings(
        &self,
        Parameters(args): Parameters<SetCubemapBindingsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                SetCubemapBindingsArgs::Preview {
                    helper,
                    faces,
                    operation,
                    snapshot,
                } => session.preview_cubemap_bindings(
                    Actor::Agent,
                    crate::session::cubemap_bindings::Request {
                        helper,
                        faces: faces
                            .into_iter()
                            .map(|face| map_core::surface::FaceId {
                                solid: face.solid,
                                side: face.side,
                            })
                            .collect(),
                        operation,
                    },
                    &snapshot,
                ),
                SetCubemapBindingsArgs::Review { token } => {
                    session.review_cubemap_bindings(Actor::Agent, token)
                }
                SetCubemapBindingsArgs::Apply { token } => {
                    session.apply_cubemap_bindings(Actor::Agent, token)
                }
                SetCubemapBindingsArgs::Cancel { token } => {
                    session.cancel_cubemap_bindings(Actor::Agent, token)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
