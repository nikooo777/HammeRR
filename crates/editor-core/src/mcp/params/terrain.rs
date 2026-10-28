use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct TerrainMoveHandlesArgs {
    pub(in crate::mcp) faces: Option<Vec<FaceRef>>,
    pub(in crate::mcp) mask_token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct BeginTerrainMoveDragArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) handle: String,
    pub(in crate::mcp) pointer: Option<PointerArgs>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct UpdateTerrainMoveDragArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) pointer: Option<PointerArgs>,
    pub(in crate::mcp) offset: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainFacesArgs {
    pub faces: Option<Vec<FaceRef>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainSubdivisionArgs {
    pub faces: Option<Vec<FaceRef>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainSewArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub blend: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainSmoothingArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub angle: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerrainTriangleChoice {
    Automatic,
    Enabled,
    Disabled,
}

pub(in crate::mcp) fn terrain_triangle_choice(
    choice: TerrainTriangleChoice,
) -> map_core::displacement::triangles::Choice {
    use map_core::displacement::triangles::Choice;
    match choice {
        TerrainTriangleChoice::Automatic => Choice::Automatic,
        TerrainTriangleChoice::Enabled => Choice::Enabled,
        TerrainTriangleChoice::Disabled => Choice::Disabled,
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainTriangleTarget {
    pub face: FaceRef,
    pub triangles: Vec<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainTriangleTagsArgs {
    pub targets: Vec<TerrainTriangleTarget>,
    pub walkable: Option<TerrainTriangleChoice>,
    pub buildable: Option<TerrainTriangleChoice>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainTriangleTargetsArgs {
    pub targets: Vec<TerrainTriangleTarget>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainTriangleFrameArgs {
    pub targets: Vec<TerrainTriangleTarget>,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainTrianglePickArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexTarget {
    pub face: FaceRef,
    pub vertices: Vec<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexTargetsArgs {
    pub targets: Vec<TerrainVertexTarget>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexFrameArgs {
    pub targets: Vec<TerrainVertexTarget>,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexMoveArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub offset: [f64; 3],
    pub mask_token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexSource {
    /// Decimal strings from get_terrain_vertices(preview=false). Required to reject stale native IDs.
    #[schemars(length(min = 1, max = 20))]
    pub generation: String,
    #[schemars(length(min = 1, max = 20))]
    pub revision: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexPosition {
    pub vertex: usize,
    pub position: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexHeight {
    pub vertex: usize,
    pub z: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum TerrainVertexValue {
    Position(TerrainVertexPosition),
    Height(TerrainVertexHeight),
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexSetTarget {
    pub face: FaceRef,
    /// Unique native row-major IDs, each with either position=[x,y,z] or z. No interpolation or omitted samples.
    #[schemars(length(min = 1, max = 289))]
    pub vertices: Vec<TerrainVertexValue>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexSetArgs {
    pub source: TerrainVertexSource,
    /// One group per face; at most 4624 samples in the complete request.
    #[schemars(length(min = 1, max = 16))]
    pub targets: Vec<TerrainVertexSetTarget>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVertexMaskArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainVolumeArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub volume_token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainCollisionArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub physics_collision: Option<bool>,
    pub player_collision: Option<bool>,
    pub ray_collision: Option<bool>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerrainDirection {
    WorldZ,
    FaceNormal,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerrainSculptMode {
    Raise,
    Flatten,
    Smooth,
    Noise,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainSculptArgs {
    pub mask_token: Option<String>,
    pub faces: Option<Vec<FaceRef>>,
    pub mode: TerrainSculptMode,
    pub center: [f64; 3],
    pub radius: Option<f64>,
    pub direction: Option<TerrainDirection>,
    pub amount: Option<f64>,
    pub strength: Option<f64>,
    pub seed: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerrainBlendMode {
    Paint,
    Fill,
    Invert,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainBlendArgs {
    pub mask_token: Option<String>,
    pub faces: Option<Vec<FaceRef>>,
    pub mode: TerrainBlendMode,
    pub center: Option<[f64; 3]>,
    pub radius: Option<f64>,
    pub direction: Option<TerrainDirection>,
    pub value: Option<f64>,
    pub strength: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainStrokeArgs {
    pub sculpt: Option<TerrainSculptArgs>,
    pub blend: Option<TerrainBlendArgs>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainStrokeTokenArgs {
    pub token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainStrokeUpdateArgs {
    pub token: String,
    pub points: Option<Vec<[f64; 3]>>,
    pub origin: Option<[f64; 3]>,
    pub direction: Option<[f64; 3]>,
}

pub(in crate::mcp) fn terrain_sculpt_operation(
    args: &TerrainSculptArgs,
) -> map_core::displacement::Operation {
    use map_core::displacement::{
        Operation,
        brush::{Brush, Sculpt},
    };
    let brush = Brush {
        center: vec3(args.center),
        radius: args.radius.unwrap_or(128.0),
        axis: terrain_direction(args.direction),
    };
    let operation = match args.mode {
        TerrainSculptMode::Raise => Sculpt::Raise(args.amount.unwrap_or(16.0)),
        TerrainSculptMode::Flatten => Sculpt::Flatten(args.strength.unwrap_or(0.5)),
        TerrainSculptMode::Smooth => Sculpt::Smooth(args.strength.unwrap_or(0.5)),
        TerrainSculptMode::Noise => Sculpt::Noise {
            amplitude: args.amount.unwrap_or(16.0),
            seed: args.seed.unwrap_or(1),
        },
    };
    Operation::Sculpt { brush, operation }
}

pub(in crate::mcp) fn terrain_blend_operation(
    args: &TerrainBlendArgs,
) -> Result<map_core::displacement::Operation, String> {
    use map_core::displacement::{
        Operation,
        brush::{Blend, Brush},
    };
    let center = match args.mode {
        TerrainBlendMode::Paint => args.center.ok_or("center is required for blend painting")?,
        _ => [0.0; 3],
    };
    let brush = Brush {
        center: vec3(center),
        radius: args.radius.unwrap_or(128.0),
        axis: terrain_direction(args.direction),
    };
    let operation = match args.mode {
        TerrainBlendMode::Paint => Blend::Paint {
            value: args.value.unwrap_or(128.0),
            strength: args.strength.unwrap_or(1.0),
        },
        TerrainBlendMode::Fill => Blend::Fill(args.value.unwrap_or(128.0)),
        TerrainBlendMode::Invert => Blend::Invert,
    };
    Ok(Operation::Blend { brush, operation })
}

pub(in crate::mcp) fn terrain_direction(
    value: Option<TerrainDirection>,
) -> map_core::displacement::brush::Axis {
    match value {
        Some(TerrainDirection::FaceNormal) => map_core::displacement::brush::Axis::FaceNormal,
        _ => map_core::displacement::brush::Axis::WorldZ,
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainResolutionArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub power: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerrainGridsArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub preview: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTerrainArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub power: Option<u32>,
    pub start_corner: Option<u32>,
}
