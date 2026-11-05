use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChooseModelArgs {
    pub token: u64,
    pub path: String,
    pub skin: Option<usize>,
    pub body: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginModelChoiceArgs {
    /// One whole point entity; exclusive with ids. Omit both to capture the whole selection.
    pub id: Option<String>,
    /// 1–64 whole point entities with World Model fields. Never expands child brushes or skips incompatible objects.
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelChoiceQueryArgs {
    pub token: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelBodygroupArgs {
    pub token: u64,
    pub group: usize,
    pub variant: usize,
}
