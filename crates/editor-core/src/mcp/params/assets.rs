use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MaterialSearchPageArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectMaterialFacesArgs {
    pub token: String,
    /// replace (default), add, remove or toggle. Does not reveal hidden faces.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplaceMaterialsArgs {
    /// Token returned by find_material_faces. Rejects changed map or search selection.
    pub token: String,
    pub material: String,
    /// Draft without changing history; finish with commit_face_preview/cancel_face_preview.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BrowseMaterialsArgs {
    pub query: Option<String>,
    pub group: Option<String>,
    pub offset: Option<usize>,
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// all (default), directory, vpk, unavailable, or an exact ID from sources. Filters winning mounts, not shadowed copies; no stock-content inference.
    pub source: Option<String>,
    /// Exclude every maps/... material, including generated patches and cubemap variants. This path filter also excludes authored materials under maps/.
    #[serde(default)]
    pub exclude_generated_map_materials: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FavoriteMaterialArgs {
    pub material: String,
    pub favorite: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetMaterialArgs {
    pub material: String,
    /// Objects whose faces all get the material; defaults to the selection
    /// when `faces` is also absent.
    pub ids: Option<Vec<String>>,
    /// Individual faces, as reported by get_objects or pick_at_pixel.
    pub faces: Option<Vec<FaceRef>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectModelArgs {
    pub path: String,
    pub skin: Option<usize>,
    pub body: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowseModelsArgs {
    pub query: Option<String>,
    /// all, used, or folder:models/... from groups.
    pub group: Option<String>,
    pub offset: Option<usize>,
    /// 1 to 100, default 60. Listing does not decode model meshes.
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// all (default), directory, vpk, unavailable, or an exact ID from sources. Selects the current winning mount; no stock-content inference.
    pub source: Option<String>,
}

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
    pub id: Option<String>,
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowseSoundsArgs {
    pub query: Option<String>,
    pub group: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundArgs {
    pub value: String,
    pub variant: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundPreviewArgs {
    pub value: String,
    pub variant: Option<usize>,
    pub volume: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginSoundChoiceArgs {
    pub id: Option<String>,
    pub key: String,
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChooseSoundArgs {
    pub token: u64,
    pub value: String,
    pub variant: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundVolumeArgs {
    pub volume: f32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelAttachmentArgs {
    pub id: Option<String>,
    /// Native attachment index from inspect_entity_model.attachments, starting at 1.
    pub attachment: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameModelAttachmentArgs {
    pub id: Option<String>,
    pub attachment: usize,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelChoiceTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderModelArgs {
    pub path: String,
    pub skin: Option<usize>,
    pub body: Option<u32>,
    /// 64 to 1024, default 640.
    pub width: Option<u32>,
    /// 64 to 1024, default 400.
    pub height: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchMaterialsArgs {
    /// Case-insensitive substring, e.g. 'brick', 'cs_italy', 'tools/'.
    pub query: String,
    /// Maximum results from 1 to 200 (default 50).
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Result offset (default 0).
    pub offset: Option<usize>,
    /// all (default), directory, vpk, unavailable, or an exact ID from sources. Selects the current winning mount; no stock-content inference.
    pub source: Option<String>,
    /// Exclude every maps/... material, including generated patches and cubemap variants. Also excludes authored materials under maps/.
    #[serde(default)]
    pub exclude_generated_map_materials: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MaterialPreviewArgs {
    /// Material path without materials/ or .vmt, e.g. concrete/concretefloor001a.
    pub material: String,
    pub channel: Option<crate::session::MaterialPreviewChannel>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowseParticlesArgs {
    pub query: Option<String>,
    pub group: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParticleNameArgs {
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginParticleChoiceArgs {
    pub id: Option<String>,
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChooseParticleArgs {
    pub token: u64,
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterialSourceArgs {
    pub material: String,
}
