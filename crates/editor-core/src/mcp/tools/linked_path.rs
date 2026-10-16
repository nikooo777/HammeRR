use super::super::*;

#[tool_router(router = linked_path_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared Create linked path preview, snapshot, ownership, validity and ordered nodes with exact IDs, origins, targetnames and next targets. Supports path_corner/path_track, 2–64 distinct finite points within ±65536, an ASCII name prefix up to 96 bytes and an optional closed loop. Source and history stay unchanged until Apply. Generated names avoid existing names case-insensitively and reject acquisition by existing exact/wildcard references. Mounted FGD point-class defaults and property validation apply. render_view shows the draft. UI: Create linked path….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_linked_path(&self) -> ToolResult {
        run(|| Ok(self.session().linked_path()))
    }

    #[tool(
        description = "Create a linked path through one shared owned draft. action=preview requires classname path_corner|path_track, prefix, ordered world points [[x,y,z],…] and latest get_linked_path.snapshot; closed defaults false. Names use prefix_001 onward, adding a collision suffix when occupied. Each target links to the next generated name; an open path's final target is absent and a closed path returns to the first name. FGD defaults are inherited, with generated links authoritative. Invalid requests preserve the current preview. action=review with token explicitly transfers ownership and returns a new token. action=apply revalidates and creates all nodes in one Undo step, selecting them. action=cancel discards only the matching owned token, including stale cleanup. Foreign Apply/Cancel and replaced or stale tokens reject. Existing paths, train entities, alternate branches and native movement behavior are outside this creation operation. UI: Create linked path… → Preview / Review preview / Apply / Cancel."
    )]
    pub(in crate::mcp) fn set_linked_path(
        &self,
        Parameters(args): Parameters<LinkedPathArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                LinkedPathArgs::Preview {
                    classname,
                    prefix,
                    points,
                    closed,
                    snapshot,
                } => session.preview_linked_path(
                    Actor::Agent,
                    crate::session::linked_path::Request {
                        classname,
                        prefix,
                        points: points.into_iter().map(DVec3::from_array).collect(),
                        closed,
                    },
                    &snapshot,
                ),
                LinkedPathArgs::Review { token } => session.review_linked_path(Actor::Agent, token),
                LinkedPathArgs::Apply { token } => session.apply_linked_path(Actor::Agent, token),
                LinkedPathArgs::Cancel { token } => session.cancel_linked_path(Actor::Agent, token),
            }
        })
    }
}

#[cfg(test)]
mod tests;
