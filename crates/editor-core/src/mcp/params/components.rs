use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentRef {
    pub(in crate::mcp) solid: u32,
    /// One side identifies a whole face; two incident side IDs identify an edge; three or more identify a vertex.
    pub(in crate::mcp) sides: Vec<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentQueryArgs {
    /// Numeric brush IDs, including brushes inside entities. Defaults to selection.
    pub(in crate::mcp) solids: Option<Vec<u32>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentSelectionArgs {
    pub(in crate::mcp) components: Vec<ComponentRef>,
    /// Exact snapshot from get_brush_components or the last component operation.
    pub(in crate::mcp) snapshot: String,
    /// replace (default), add, remove or toggle.
    pub(in crate::mcp) mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentPickingArgs {
    pub(in crate::mcp) enabled: bool,
    pub(in crate::mcp) kind: Option<crate::session::component_picking::Kind>,
    pub(in crate::mcp) solids: Option<Vec<u32>>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentPixelArgs {
    pub(in crate::mcp) x: f64,
    pub(in crate::mcp) y: f64,
    pub(in crate::mcp) width: Option<u32>,
    pub(in crate::mcp) height: Option<u32>,
    pub(in crate::mcp) camera: Option<CameraSpec>,
    pub(in crate::mcp) kind: crate::session::component_picking::Kind,
    pub(in crate::mcp) solids: Option<Vec<u32>>,
    pub(in crate::mcp) radius: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentMoveArgs {
    pub(in crate::mcp) delta: [f64; 3],
    /// Snap the offset using the editor's current grid and snap setting.
    #[serde(default)]
    pub(in crate::mcp) snap: bool,
    /// Preserve texture coordinates at corresponding corners; rejects incompatible face deformations.
    #[serde(default)]
    pub(in crate::mcp) texture_lock: bool,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentSourcesArgs {
    pub(in crate::mcp) offset: Option<usize>,
    pub(in crate::mcp) limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentScopeArgs {
    pub(in crate::mcp) solid: u32,
    pub(in crate::mcp) action: crate::session::components::ScopeAction,
    pub(in crate::mcp) kind: crate::session::component_picking::Kind,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentWeldArgs {
    pub(in crate::mcp) snapshot: String,
    /// Preserve surviving corner UVs and average merged contributors per face. Defaults false (world projection).
    pub(in crate::mcp) texture_lock: Option<bool>,
    pub(in crate::mcp) target: Option<[f64; 3]>,
    pub(in crate::mcp) target_corner: Option<ComponentRef>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentScaleArgs {
    pub(in crate::mcp) factors: [f64; 3],
    pub(in crate::mcp) pivot: Option<[f64; 3]>,
    pub(in crate::mcp) texture_lock: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentRotateArgs {
    pub(in crate::mcp) degrees: [f64; 3],
    pub(in crate::mcp) pivot: Option<[f64; 3]>,
    pub(in crate::mcp) texture_lock: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentCreaseArgs {
    pub(in crate::mcp) pairs: Vec<crate::session::components::CreaseEdges>,
    pub(in crate::mcp) rise: f64,
    /// Preserve the two roof faces' corresponding corner UVs; neighbors retain world projection. Default false.
    pub(in crate::mcp) roof_texture_lock: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentPartitionArgs {
    pub(in crate::mcp) pairs: Vec<crate::session::components::CreaseEdges>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct SavedPartitionsArgs {
    pub(in crate::mcp) sources: Vec<FaceRef>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct RaisePartitionArgs {
    pub(in crate::mcp) token: String,
    /// Zero restores the flat draft; otherwise 0.125–4096 world units.
    pub(in crate::mcp) rise: f64,
    /// Attach the two roof faces only. Omit to retain the draft's current policy; defaults false when staged.
    pub(in crate::mcp) roof_texture_lock: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PartitionCutArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) edge_parameters: [f64; 2],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PartitionMaterialArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) partitions: Vec<crate::session::components::PartitionRef>,
    pub(in crate::mcp) material: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PartitionTextureSpec {
    pub(in crate::mcp) shift: [f64; 2],
    pub(in crate::mcp) scale: [f64; 2],
    pub(in crate::mcp) rotation: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PartitionTextureArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) partitions: Vec<crate::session::components::PartitionRef>,
    pub(in crate::mcp) texture: Option<PartitionTextureSpec>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::mcp) enum PartitionTextureJustify {
    Start,
    Center,
    End,
}

impl From<PartitionTextureJustify> for map_core::surface::Justify {
    fn from(value: PartitionTextureJustify) -> Self {
        match value {
            PartitionTextureJustify::Start => Self::Start,
            PartitionTextureJustify::Center => Self::Center,
            PartitionTextureJustify::End => Self::End,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PlacePartitionTextureArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) partitions: Vec<crate::session::components::PartitionRef>,
    pub(in crate::mcp) fit: Option<bool>,
    pub(in crate::mcp) justify_u: Option<PartitionTextureJustify>,
    pub(in crate::mcp) justify_v: Option<PartitionTextureJustify>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ExtrudePartitionArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) height: f64,
    pub(in crate::mcp) wall_material: Option<String>,
    pub(in crate::mcp) texture_lock: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct SelectPartitionsArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) partitions: Vec<crate::session::components::PartitionRef>,
    /// replace (default), add, remove or toggle. Selection highlights and frames loops.
    pub(in crate::mcp) mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct FramePartitionsArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PartitionPixelArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) x: f64,
    pub(in crate::mcp) y: f64,
    pub(in crate::mcp) width: Option<u32>,
    pub(in crate::mcp) height: Option<u32>,
    pub(in crate::mcp) camera: Option<CameraSpec>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentSplitArgs {
    pub(in crate::mcp) rise: f64,
    /// Preserve each fan triangle's original corner and center/midpoint UVs. Defaults false (world projection).
    pub(in crate::mcp) texture_lock: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ComponentMoveOptionsArgs {
    pub(in crate::mcp) enabled: Option<bool>,
    pub(in crate::mcp) snap: Option<bool>,
    pub(in crate::mcp) texture_lock: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct BeginComponentDragArgs {
    pub(in crate::mcp) handle: String,
    pub(in crate::mcp) pointer: Option<PointerArgs>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct UpdateComponentDragArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) pointer: Option<PointerArgs>,
    /// Total world-unit offsets along the handle axes since begin; view uses screen right/up.
    pub(in crate::mcp) offset: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityComponentArgs {
    pub id: Option<String>,
    /// Vector or color property key.
    pub key: String,
    /// Zero-based component; 0..2 for position/rotation/RGB, 3 for brightness.
    pub index: usize,
    /// Numeric component value, preserving the other components exactly.
    pub value: String,
}
