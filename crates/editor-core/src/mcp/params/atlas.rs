use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectMaterialAtlasArgs {
    pub material: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct AtlasRectangleArgs {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct PreviewAtlasFitArgs {
    pub material: String,
    pub rectangle: AtlasRectangleArgs,
    #[serde(default)]
    pub padding: u32,
    pub snapshot: String,
    pub faces: Option<Vec<FaceRef>>,
    pub token: Option<String>,
}
