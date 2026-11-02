use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentProfilesInspectArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveContentProfileArgs {
    pub name: String,
    /// Omit or null to capture the current content paths and build settings.
    pub config: Option<crate::content_profiles::Config>,
    /// Omit to create; supply a profile ID to replace that saved profile.
    pub replace_id: Option<String>,
    /// Shared revision from get_content_profiles.
    pub revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyContentProfileArgs {
    pub id: String,
    /// Shared revision from get_content_profiles.
    pub revision: u64,
    /// Current content intent from get_content_profiles.
    pub intent: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveContentProfileArgs {
    pub id: String,
    /// Shared revision from get_content_profiles.
    pub revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadContentProfilesArgs {
    /// Shared revision from get_content_profiles.
    pub revision: u64,
}
