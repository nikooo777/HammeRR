use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityPlacementArgs {
    pub classname: String,
    pub properties: Option<BTreeMap<String, String>>,
    /// Finite normal offset within ±65536; default zero.
    pub offset: Option<f64>,
    /// Rotate the entity's existing local up to the surface normal; default false.
    pub align: Option<bool>,
    /// Snap two tangential world coordinates to the shared grid, preserving contact; defaults to shared snapping.
    pub snap: Option<bool>,
    /// Latest get_entity_placement.snapshot.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityPlacementTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityPlacementPickArgs {
    pub token: u64,
    pub ray_origin: [f64; 3],
    pub ray_direction: [f64; 3],
}
