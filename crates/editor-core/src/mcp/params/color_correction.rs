use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadColorCorrectionArgs {
    /// Explicit physical Source .raw LUT path. Exactly 98304 RGB8 bytes, 32 cubed entries, red index fastest.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetColorCorrectionArgs {
    /// Enable or disable the loaded LUT for the whole editor viewport and render_view.
    pub enabled: bool,
    /// Finite blend weight from 0 to 1. Zero preserves input display bytes; one uses the LUT output.
    pub weight: f32,
    /// Optional admitted identity from get_color_correction_preview. Enabling refuses if another LUT was loaded since inspection.
    pub expected_identity: Option<u64>,
}
