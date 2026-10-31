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

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::mcp) enum AtlasFitModeArgs {
    #[default]
    PerFace,
    Together,
    Pack,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct AtlasPackingArgs {
    /// Common multiplier of every face's current projected texel width and height; finite and positive.
    pub scale: f64,
    /// Whole source-image pixels reserved on every side of each island, inside the outer region padding.
    #[serde(default)]
    pub padding: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct PreviewAtlasFitArgs {
    pub material: String,
    pub rectangle: AtlasRectangleArgs,
    #[serde(default)]
    pub padding: u32,
    #[serde(default)]
    pub mode: AtlasFitModeArgs,
    pub reference: Option<FaceRef>,
    pub packing: Option<AtlasPackingArgs>,
    pub snapshot: String,
    pub faces: Option<Vec<FaceRef>>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct SaveAtlasRegionArgs {
    pub name: String,
    pub material: String,
    pub rectangle: AtlasRectangleArgs,
    #[serde(default)]
    pub padding: u32,
    pub snapshot: String,
    pub catalog_snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RenameAtlasRegionArgs {
    pub id: u32,
    pub name: String,
    pub catalog_snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct DeleteAtlasRegionArgs {
    pub id: u32,
    pub catalog_snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ChooseAtlasRegionArgs {
    pub id: u32,
    pub material: String,
    pub snapshot: String,
    pub catalog_snapshot: String,
}
