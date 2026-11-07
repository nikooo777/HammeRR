use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementArgs {
    pub request: crate::session::rope_placement::Request,
    /// Latest get_rope_placement.snapshot.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementUpdateArgs {
    pub token: u64,
    pub request: crate::session::rope_placement::Request,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementBeginPickArgs {
    pub token: u64,
    pub endpoint: crate::session::rope_placement::Endpoint,
    pub width: u32,
    pub height: u32,
    pub camera: Option<CameraSpec>,
    /// Latest get_rope_placement.pick_snapshot.
    pub pick_snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementRayArgs {
    pub token: u64,
    pub ray_origin: [f64; 3],
    pub ray_direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePlacementPixelArgs {
    pub token: u64,
    pub pointer: PointerArgs,
}
