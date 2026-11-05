use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSceneDisplayArgs {
    /// legacy or hdr. Omitted mode preserves the current Scene display.
    pub mode: Option<String>,
    /// Finite -8..8 EV. Switching from Legacy to HDR requires an explicit value.
    pub exposure_ev: Option<f64>,
}
