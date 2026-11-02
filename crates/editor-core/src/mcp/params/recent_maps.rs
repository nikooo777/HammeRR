use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecentMapsInspectArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenRecentMapArgs {
    /// Exact path returned by get_recent_maps.
    pub path: String,
    /// Shared list serial from get_recent_maps.
    pub serial: u64,
    /// Explicitly discard the reviewed current map's unsaved changes.
    #[serde(default)]
    pub discard_changes: bool,
    /// Exact replacement_review from get_map_info; rejects changed buffers.
    pub expected_review: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelocateRecentMapArgs {
    /// Exact existing record path returned by get_recent_maps.
    pub old_path: String,
    /// Explicit absolute path to the existing destination VMF.
    pub new_path: String,
    /// Shared list serial from get_recent_maps.
    pub serial: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelRecentMapArgs {
    /// Exact worker_token from get_recent_maps.
    pub token: u64,
}
