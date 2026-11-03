use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentRefreshInspectArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentWatchArgs {
    pub enabled: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadContentArgs {
    /// Current content intent from get_content_refresh.
    pub intent: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelContentRefreshArgs {
    /// Exact decimal pending.token from get_content_refresh.
    pub token: String,
}
