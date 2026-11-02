use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisibilityCategoriesArgs {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisibilityCategorySetArgs {
    pub snapshot: String,
    pub id: String,
    pub excluded: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisibilityFilterExcludeArgs {
    pub snapshot: String,
    pub filter: ObjectFilter,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisibilityExclusionRemoveArgs {
    pub snapshot: String,
    pub index: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisibilityProfilesArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum VisibilityProfileSetArgs {
    Save {
        snapshot: String,
        name: String,
        #[serde(default)]
        replace: bool,
    },
    Rename {
        snapshot: String,
        name: String,
        new_name: String,
    },
    Delete {
        snapshot: String,
        name: String,
    },
    Apply {
        snapshot: String,
        name: String,
    },
}
