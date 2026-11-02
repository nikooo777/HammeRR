use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkplaneInspectArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum WorkplaneArgs {
    Update {
        settings: crate::session::workplane::Update,
        /// Optional snapshot from get_workplane; rejects changed guide settings.
        snapshot: Option<String>,
    },
    FromFace {
        solid: u32,
        side: u32,
        snapshot: Option<String>,
    },
    FromRay {
        ray_origin: [f64; 3],
        ray_direction: [f64; 3],
        snapshot: Option<String>,
    },
    Reset {
        snapshot: Option<String>,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkplaneIntersectionArgs {
    pub ray_origin: [f64; 3],
    pub ray_direction: [f64; 3],
    /// Omit or null to use the plane's current snapping setting.
    pub snap: Option<bool>,
}
