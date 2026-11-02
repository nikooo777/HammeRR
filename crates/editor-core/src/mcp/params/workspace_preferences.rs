use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePreferencesInspectArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLayoutSetArgs {
    pub snapshot: String,
    pub layout: crate::workspace_preferences::Layout,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePreferencesAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl WorkspacePreferencesAction {
    pub(in crate::mcp) fn name(&self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Apply => "apply",
            Self::Reload => "reload",
            Self::Reset => "reset",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePreferencesSetArgs {
    pub snapshot: String,
    pub action: WorkspacePreferencesAction,
}
