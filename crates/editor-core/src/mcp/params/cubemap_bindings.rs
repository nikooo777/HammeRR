use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CubemapBindingsArgs {
    pub helper: Option<u32>,
    pub query: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum SetCubemapBindingsArgs {
    Preview {
        helper: u32,
        faces: Vec<CubemapFaceRef>,
        operation: crate::session::cubemap_bindings::Operation,
        snapshot: String,
    },
    Review {
        token: u64,
    },
    Apply {
        token: u64,
    },
    Cancel {
        token: u64,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CubemapFaceRef {
    pub solid: u32,
    pub side: u32,
}
