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
        description = "Create or edit a linked path through one shared owned draft. action=preview requires classname path_corner|path_track, prefix, ordered points [[x,y,z],…], latest get_linked_path.snapshot; closed defaults false. Generated names avoid occupied names and reference acquisition, and FGD defaults apply. action=inspect with id discovers the complete existing chain and its snapshot without mutation. action=edit takes ordered nodes [{id,origin:[x,y,z]},…] and that snapshot. Without seed every original node is required. To insert/remove, provide the inspected seed, omit id for new nodes, and optionally prefix (default path) for their safe names. Keep at least one original node. The reviewed draft lists source_id for retained nodes, null for new nodes, and removed IDs/names. Removal rejects nodes that own connections or receive any references beyond this chain’s ordinary targets. Retained IDs, names, other properties, flags, connections and external references stay fixed; new nodes receive FGD defaults. Class and open/closed topology stay fixed. Ambiguous names/IDs, branches/altpath, wildcard path links, foreign/missing targets, parented/hidden nodes and mixed classes reject. Both modes require 2–64 distinct finite points within ±65536. action=review with token transfers ownership and returns a new token. action=apply revalidates and commits in one Undo step, selecting the nodes. action=cancel discards only the matching owned token. Invalid requests preserve the preview; foreign or replaced tokens reject. Branching, renaming, train setup and native movement validation are unavailable. UI: Create linked path… → Load selected path / Preview / Review preview / Apply / Cancel."
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
                LinkedPathArgs::Edit {
                    nodes,
                    snapshot,
                    seed,
                    prefix,
                } => {
                    if !(crate::session::linked_path::MIN_POINTS
                        ..=crate::session::linked_path::MAX_POINTS)
                        .contains(&nodes.len())
                    {
                        return Err("Edit one complete path with 2–64 nodes".into());
                    }
                    let nodes = nodes
                        .into_iter()
                        .map(|node| {
                            let id = match node.id {
                                Some(id) => {
                                    let ObjectId::Entity(id) = parse_id(&id)? else {
                                        return Err("Existing path edits require entity IDs".into());
                                    };
                                    Some(id)
                                }
                                None => None,
                            };
                            Ok(crate::session::linked_path::EditNode {
                                id,
                                origin: DVec3::from_array(node.origin),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    if let Some(seed) = seed {
                        let ObjectId::Entity(seed) = parse_id(&seed)? else {
                            return Err("Existing path seed requires an entity ID".into());
                        };
                        session.preview_linked_path_changes(
                            Actor::Agent,
                            crate::session::linked_path::ChangeRequest {
                                seed,
                                nodes,
                                prefix: prefix.unwrap_or_else(|| "path".into()),
                            },
                            &snapshot,
                        )
                    } else {
                        if prefix.is_some() {
                            return Err("Provide the inspected seed when changing path membership or naming new nodes".into());
                        }
                        let nodes = nodes
                            .into_iter()
                            .map(|node| {
                                Ok(crate::session::linked_path::ExistingNode {
                                    id: node
                                        .id
                                        .ok_or("Provide the inspected seed to insert new nodes")?,
                                    origin: node.origin,
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        session.preview_existing_linked_path(
                            Actor::Agent,
                            crate::session::linked_path::EditRequest { nodes },
                            &snapshot,
                        )
                    }
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
