use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingRegionInspectArgs {
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingRegionSetArgs {
    pub snapshot: String,
    pub action: String,
    pub enabled: Option<bool>,
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum EditorVisibilitySetArgs {
    Hide {
        snapshot: String,
        ids: Option<Vec<String>>,
    },
    ShowAll {
        snapshot: String,
    },
}
