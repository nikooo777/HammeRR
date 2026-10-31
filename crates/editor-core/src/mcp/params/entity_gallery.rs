use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGalleryBrowseArgs {
    /// Case-insensitive class name or description substring. Default empty.
    pub search: Option<String>,
    /// Zero-based catalog page; 32 classes per page. Default zero.
    pub page: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGalleryExampleArgs {
    /// Exact loaded FGD point classname. One example per class in a batch.
    pub classname: String,
    /// Optional shared entity-property overrides; defaults come from the FGD. Origin is arrangement-owned.
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGalleryPreviewArgs {
    /// From 1 to 32 explicit loaded FGD point classes; none are silently skipped.
    pub examples: Vec<EntityGalleryExampleArgs>,
    /// XY center of the first visual cell at its bottom Z, in Source units.
    pub origin: [f64; 3],
    /// Row-major XY columns, from 1 to 32. Default four.
    pub columns: Option<usize>,
    /// Gap between visual cells, from 16 to 4096 units. Default 64.
    pub gap: Option<f64>,
    /// Current get_entity_gallery.snapshot or browse_entity_gallery.snapshot.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGalleryTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityGalleryFrameArgs {
    pub token: u64,
    /// Zero-based example to inspect; omit for the complete reviewed arrangement.
    pub index: Option<usize>,
    /// Viewport aspect ratio; default 1.5.
    pub aspect: Option<f32>,
}
