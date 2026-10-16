use super::super::*;

#[tool_router(router = linked_path_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared linked path creation or existing-chain edit preview, snapshot, ownership, validity and ordered nodes with exact IDs, origins, targetnames and next targets. pick_snapshot guards owned row positioning; preview.picking reports row/viewport validity and last_pick reports surface provenance. Source and history stay unchanged until Apply. render_view shows the draft. Use set_linked_path action=inspect with one entity ID to discover an existing complete simple path_corner/path_track chain. UI: Create linked path….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_linked_path(&self) -> ToolResult {
        run(|| Ok(self.session().linked_path()))
    }

    #[tool(
        description = "Create or edit a linked path through one shared owned draft. action=preview requires classname path_corner|path_track, prefix, ordered points [[x,y,z],…], latest get_linked_path.snapshot; closed defaults false. Generated names avoid occupied names and reference acquisition, and FGD defaults apply. action=inspect with id discovers the complete existing chain and its snapshot without mutation. action=edit takes ordered nodes [{id,origin:[x,y,z]},…] and that snapshot. Without seed every original node is required. To insert/remove, provide the inspected seed, omit id for new nodes, and optionally prefix (default path) for their safe names. Keep at least one original node. The reviewed draft lists source_id for retained nodes, null for new nodes, and removed IDs/names. Removal rejects nodes that own connections or receive any references beyond this chain’s ordinary targets. Retained IDs, names, other properties, flags, connections and external references stay fixed; new nodes receive FGD defaults. Class and open/closed topology stay fixed. Ambiguous names/IDs, branches/altpath, wildcard path links, foreign/missing targets, parented/hidden nodes and mixed classes reject. Both modes require 2–64 distinct finite points within ±65536. action=review with token transfers ownership and returns a new token. action=apply revalidates and commits in one Undo step, selecting the nodes. action=cancel discards only the matching owned token. Invalid requests preserve the preview; foreign or replaced tokens reject. action=begin_pick arms one zero-based row of an existing owned preview with token, snapshot=get_linked_path.pick_snapshot, width/height 1–8192 and optional temporary render_view camera. action=pick_ray takes token and finite origin/direction arrays; action=pick_pixel takes token, continuous x/y and the captured width/height; pixel centers are i+0.5,j+0.5. The first visible brush, terrain triangle or supported model mesh supplies exact unsnapped coordinates. Path nodes, helper boxes and empty space supply no substitute plane. A hit updates only that row, returns a fresh token and last_pick, and finishes picking. Misses, coincident rows and stale scene/camera/viewport preserve the draft. Apply rejects while armed. cancel_pick disarms the matching owned gesture with rows unchanged, including stale cleanup. Numeric edits and existing Review/Apply remain available; points never attach to surfaces. Branching, renaming, train setup and native movement validation are unavailable. UI: Create linked path… → Load selected path / Preview / Pick position… / Cancel position pick / Review preview / Apply / Cancel."
    )]
    pub(in crate::mcp) fn set_linked_path(
        &self,
        Parameters(args): Parameters<LinkedPathArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                LinkedPathArgs::BeginPick {
                    token,
                    row,
                    camera,
                    width,
                    height,
                    snapshot,
                } => {
                    let camera = camera
                        .unwrap_or_default()
                        .resolve(&session, width as f32 / height as f32)?;
                    session.begin_linked_path_pick(
                        Actor::Agent,
                        token,
                        row,
                        camera,
                        (width, height),
                        &snapshot,
                    )
                }
                LinkedPathArgs::PickRay {
                    token,
                    origin,
                    direction,
                } => session.pick_linked_path_ray(
                    Actor::Agent,
                    token,
                    Ray {
                        origin: vec3(origin),
                        direction: vec3(direction),
                    },
                ),
                LinkedPathArgs::PickPixel {
                    token,
                    x,
                    y,
                    width,
                    height,
                } => session.pick_linked_path_pixel(
                    Actor::Agent,
                    token,
                    crate::session::Pointer {
                        position: glam::DVec2::new(x, y),
                        size: (width, height),
                    },
                ),
                LinkedPathArgs::CancelPick { token } => {
                    session.cancel_linked_path_pick(Actor::Agent, token)
                }
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
