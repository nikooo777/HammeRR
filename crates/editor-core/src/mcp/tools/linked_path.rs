use super::super::*;

#[tool_router(router = linked_path_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared linked path creation or existing-chain edit preview, snapshot, ownership, validity and ordered nodes with exact IDs, origins, targetnames and next targets. Source and history stay unchanged until Apply. render_view shows the draft. Use set_linked_path action=inspect with one entity ID to discover an existing complete simple path_corner/path_track chain. UI: Create linked path….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_linked_path(&self) -> ToolResult {
        run(|| Ok(self.session().linked_path()))
    }

    #[tool(
        description = "Create or edit a linked path through one shared owned draft. action=preview requires classname path_corner|path_track, prefix, ordered points [[x,y,z],…], latest get_linked_path.snapshot; closed defaults false. Generated names avoid occupied names and reference acquisition, and FGD defaults apply. action=inspect with id discovers the complete existing chain and its snapshot without mutation. action=edit requires every inspected node exactly once as ordered nodes [{id,origin:[x,y,z]},…] plus that snapshot; reorders links and repositions existing nodes, retaining IDs, names, other properties, flags, connections and external references. Class and open/closed topology stay fixed. Ambiguous names/IDs, branches/altpath, wildcard path links, foreign/missing targets, parented/hidden nodes and mixed classes reject. Both modes require 2–64 distinct finite points within ±65536. action=review with token transfers ownership and returns a new token. action=apply revalidates and commits in one Undo step, selecting the nodes. action=cancel discards only the matching owned token. Invalid requests preserve the preview; foreign or replaced tokens reject. Insertion, removal, train setup and native movement validation are unavailable. UI: Create linked path… → Load selected path / Preview / Review preview / Apply / Cancel."
    )]
    pub(in crate::mcp) fn set_linked_path(
        &self,
        Parameters(args): Parameters<LinkedPathArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                LinkedPathArgs::Inspect { id } => {
                    let ObjectId::Entity(id) = parse_id(&id)? else {
                        return Err("Existing path inspection requires an entity ID".into());
                    };
                    session.inspect_existing_linked_path(id)
                }
                LinkedPathArgs::Edit { nodes, snapshot } => {
                    if !(crate::session::linked_path::MIN_POINTS
                        ..=crate::session::linked_path::MAX_POINTS)
                        .contains(&nodes.len())
                    {
                        return Err("Edit one complete path with 2–64 nodes".into());
                    }
                    let nodes = nodes
                        .into_iter()
                        .map(|node| {
                            let ObjectId::Entity(id) = parse_id(&node.id)? else {
                                return Err("Existing path edits require entity IDs".into());
                            };
                            Ok(crate::session::linked_path::ExistingNode {
                                id,
                                origin: DVec3::from_array(node.origin),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    session.preview_existing_linked_path(
                        Actor::Agent,
                        crate::session::linked_path::EditRequest { nodes },
                        &snapshot,
                    )
                }
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
