use super::*;

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RopePreviewAction {
    Settings,
    Prepare,
    Play,
    Pause,
    Seek,
    Advance,
    Reset,
    Cancel,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RopePreviewArgs {
    pub action: RopePreviewAction,
    /// Current opaque snapshot from get_rope_preview. Changed shared controls or sources require inspection again.
    pub expected_snapshot: String,
    /// Settings only: editor gravity acceleration in Hammer units per second squared, 0..3000.
    pub gravity: Option<f64>,
    /// Settings only: explicit world-space preview wind vector. Omitted components are not inferred from map entities.
    pub wind: Option<[f64; 3]>,
    /// Seek only: finite requested time from 0 through 60 seconds, using fixed-step interpolation.
    pub time_seconds: Option<f64>,
    /// Advance only: finite elapsed preview time. Headless clients explicitly advance without a GUI timer.
    pub seconds: Option<f64>,
    /// Prepare, Seek, Advance and Reset only: true returns while the owned worker runs; false waits outside the Session lock.
    pub background: Option<bool>,
}
