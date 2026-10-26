use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGraphArgs {
    /// Explicit authored entity IDs, 1–64; hidden roots refuse.
    pub roots: Vec<String>,
    pub direction: crate::session::entity_graph::Direction,
    /// Traversal depth 1–16; defaults to 2.
    pub depth: Option<usize>,
    /// Filters displayed rows only, never connected selection/framing; at most 128 bytes.
    pub search: Option<String>,
    /// Returned snapshot is required for nonzero page offsets; stale pages reject.
    pub snapshot: Option<String>,
    #[serde(default)]
    pub node_offset: usize,
    #[serde(default)]
    pub edge_offset: usize,
    #[serde(default)]
    pub unresolved_offset: usize,
    /// Each page has at most 64 rows; defaults to 64.
    pub limit: Option<usize>,
}

impl EntityGraphArgs {
    pub fn traversal(&self) -> Result<crate::session::entity_graph::Traversal, String> {
        if self.roots.len() > crate::session::entity_graph::MAX_ROOTS {
            return Err("Graph supports at most 64 roots".into());
        }
        let roots = self
            .roots
            .iter()
            .map(|value| match crate::identity::parse_id(value)? {
                ObjectId::Entity(id) => Ok(id),
                ObjectId::Solid(_) => Err("Graph roots must be entity IDs".into()),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(crate::session::entity_graph::Traversal {
            roots,
            direction: self.direction,
            depth: self.depth.unwrap_or(2),
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectEntityGraphArgs {
    pub roots: Vec<String>,
    pub direction: crate::session::entity_graph::Direction,
    pub depth: Option<usize>,
    /// Exact snapshot from inspect_entity_graph for the same roots/direction/depth.
    pub snapshot: String,
    /// replace (default), add, remove or toggle; explicit graph IDs bypass click seed filters.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameEntityGraphArgs {
    pub roots: Vec<String>,
    pub direction: crate::session::entity_graph::Direction,
    pub depth: Option<usize>,
    pub snapshot: String,
    /// Viewport aspect 0.01–100; defaults to 1.
    pub aspect: Option<f32>,
}

pub(in crate::mcp) fn entity_graph_traversal(
    roots: Vec<String>,
    direction: crate::session::entity_graph::Direction,
    depth: Option<usize>,
) -> Result<crate::session::entity_graph::Traversal, String> {
    EntityGraphArgs {
        roots,
        direction,
        depth,
        search: None,
        snapshot: None,
        node_offset: 0,
        edge_offset: 0,
        unresolved_offset: 0,
        limit: None,
    }
    .traversal()
}
