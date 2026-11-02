use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingRegionInspectArgs {
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum WorkingRegionSetArgs {
    Set {
        snapshot: String,
        mins: [f64; 3],
        maxs: [f64; 3],
        enabled: Option<bool>,
        options: Option<crate::session::working_region::Options>,
    },
    SelectionBounds {
        snapshot: String,
        enabled: Option<bool>,
    },
    Configure {
        snapshot: String,
        options: crate::session::working_region::Options,
    },
    Clear {
        snapshot: String,
    },
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
