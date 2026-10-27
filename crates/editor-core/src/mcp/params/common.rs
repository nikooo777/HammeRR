use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct CloseDocumentArgs {
    pub review_token: String,
    #[serde(default)]
    pub discard_changes: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct StatisticsArgs {
    #[serde(default)]
    pub(in crate::mcp) section: crate::session::statistics::Section,
    #[serde(default)]
    pub(in crate::mcp) offset: usize,
    pub(in crate::mcp) limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenMapArgs {
    /// Absolute path of a .vmf file.
    pub path: String,
    /// Open even if the current map has unsaved changes, losing them.
    #[serde(default)]
    pub discard_changes: bool,
    /// Exact replacement_review from get_map_info; rejects edits, saves or partition-preview changes since review.
    pub expected_review: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewMapArgs {
    #[serde(default)]
    pub discard_changes: bool,
    /// Exact replacement_review from get_map_info; checked together with discard_changes.
    pub expected_review: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecentMapPathArgs {
    /// Exact absolute path returned by get_recent_maps.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveMapArgs {
    /// Where to save; defaults to the map's current file.
    pub path: Option<String>,
    /// Generation from get_map_info; rejects replacement while allowing edits in the same buffer.
    pub expected_generation: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Free text across IDs, class, name and materials.
    pub query: Option<String>,
    /// Filter stored VMF visibility.
    pub hidden: Option<bool>,
    /// Case-insensitive substring of the entity classname (e.g. 'info_player', 'func_').
    pub classname: Option<String>,
    #[serde(default)]
    pub exact_class: bool,
    /// Case-insensitive substring of targetname.
    pub targetname: Option<String>,
    /// Case-insensitive substring of a face material.
    pub material: Option<String>,
    /// Only objects whose bounds intersect this box: [[minx,miny,minz],[maxx,maxy,maxz]].
    pub within: Option<[[f64; 3]; 2]>,
    /// 'solid', 'entity', 'point_entity' or 'brush_entity'.
    pub kind: Option<String>,
    /// Maximum results (default 50, at most 500).
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VisibilityArgs {
    /// hide, show, isolate, show_all or restore. Hidden objects are NOT compiled.
    pub action: String,
    /// Defaults to selection for hide/show/isolate. Omit for show_all/restore.
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReportArgs {
    #[serde(default)]
    pub filter: ObjectFilter,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectReportArgs {
    #[serde(default)]
    pub filter: ObjectFilter,
    pub mode: Option<String>,
    #[serde(default)]
    pub frame: bool,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdsArgs {
    /// Object ids; defaults to the selection.
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectArgs {
    pub ids: Vec<String>,
    /// 'replace' (default), 'add', 'remove' or 'toggle'.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectGroupsArgs {
    pub group: Option<u32>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginObjectGroupArgs {
    /// Omit to create a group from the selection, preserving complete existing groups as nested members.
    pub group: Option<u32>,
    #[serde(default)]
    pub ungroup: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectGroupTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateObjectGroupArgs {
    pub token: u64,
    pub name: Option<String>,
    pub parent: Option<u32>,
    #[serde(default)]
    pub clear_parent: bool,
    /// Complete direct membership: solid:<id>, entity:<id> or group:<id>.
    pub members: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectObjectGroupArgs {
    pub group: u32,
    pub mode: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectObjectGroupMemberArgs {
    pub member: String,
    pub mode: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupSelectionBypassArgs {
    pub bypass: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPropertiesArgs {
    /// Entity id ('entity:<n>'); defaults to the single selected entity.
    pub id: Option<String>,
    /// Keys to set.
    pub set: Option<BTreeMap<String, String>>,
    /// Keys to remove.
    pub remove: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ShearArgs {
    /// Coordinate to shift: x, y or z.
    pub axis: String,
    /// Distinct reference coordinate: x, y or z.
    pub reference: String,
    pub factor: f64,
}

impl ShearArgs {
    pub(in crate::mcp) fn into_core(self) -> Result<map_core::transform::Shear, String> {
        pub(in crate::mcp) fn axis(value: &str) -> Result<usize, String> {
            match value.to_ascii_lowercase().as_str() {
                "x" => Ok(0),
                "y" => Ok(1),
                "z" => Ok(2),
                _ => Err("Choose x, y or z for each shear axis".into()),
            }
        }
        Ok(map_core::transform::Shear {
            axis: axis(&self.axis)?,
            reference: axis(&self.reference)?,
            factor: self.factor,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BeginTextureStrokeArgs {
    pub pointer: PointerArgs,
    pub radius: Option<f64>,
    /// material or projection; defaults to the shared texture operation, or material.
    pub mode: Option<String>,
    pub camera: Option<CameraSpec>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PointerArgs {
    pub position: [f64; 2],
    pub width: u32,
    pub height: u32,
}

impl PointerArgs {
    pub(in crate::mcp) fn resolve(self) -> Result<crate::session::Pointer, String> {
        crate::session::Pointer {
            position: glam::DVec2::from_array(self.position),
            size: (self.width, self.height),
        }
        .validate()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HandlesArgs {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraSpec>,
    /// Optional viewport point to hit-test against the rendered handles.
    pub point: Option<[f64; 2]>,
}

pub(in crate::mcp) fn parse_select_mode(value: &str) -> Result<SelectMode, String> {
    match value {
        "replace" => Ok(SelectMode::Replace),
        "add" => Ok(SelectMode::Add),
        "remove" => Ok(SelectMode::Remove),
        "toggle" => Ok(SelectMode::Toggle),
        _ => Err("mode must be replace, add, remove or toggle".into()),
    }
}

pub(in crate::mcp) fn select_mode_name(mode: SelectMode) -> &'static str {
    match mode {
        SelectMode::Replace => "replace",
        SelectMode::Add => "add",
        SelectMode::Remove => "remove",
        SelectMode::Toggle => "toggle",
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DuplicateArgs {
    pub ids: Option<Vec<String>>,
    /// Offset of the copies [dx, dy, dz].
    pub offset: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectionModeArgs {
    pub mode: String,
}

pub(in crate::mcp) fn parse_token(value: &str) -> Result<u64, String> {
    value.parse().map_err(|_| "Invalid preview token".into())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoveryListArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub current_map_only: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoverySettingsArgs {
    pub enabled: Option<bool>,
    /// 30 to 3600 seconds; autosaves only when committed map content changed.
    pub interval_seconds: Option<u64>,
    /// 1 to 50 snapshots per map, including previous-file backups.
    pub keep_per_map: Option<usize>,
    /// Optional stale-settings guard from get_recovery.status.settings_revision.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadRecoverySettingsArgs {
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoveryIdArgs {
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RestoreRecoveryArgs {
    pub id: String,
    /// Explicitly discard unsaved edits in the current map.
    #[serde(default)]
    pub discard_changes: bool,
    /// Optional stale-map guard; supply both generation and revision from get_recovery.status.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
    /// Exact replacement_review from get_map_info; also checks saves and partition-preview changes.
    pub expected_review: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TextureChainArgs {
    /// Connected target faces, including intermediate surfaces; omitted uses selected faces.
    pub faces: Option<Vec<FaceRef>>,
    /// Review the shared draft, then use commit_face_preview or cancel_face_preview with its token.
    #[serde(default)]
    pub preview: bool,
}

pub(in crate::mcp) fn texture_justification(
    value: Option<&str>,
    vertical: bool,
) -> Result<Option<(map_core::surface::Justify, u32)>, String> {
    use map_core::surface::Justify;
    let placement = match value {
        None => return Ok(None),
        Some("center") => Justify::Center,
        Some("left") if !vertical => Justify::Start,
        Some("right") if !vertical => Justify::End,
        Some("top") if vertical => Justify::Start,
        Some("bottom") if vertical => Justify::End,
        _ => {
            return Err(if vertical {
                "justify_v must be top, center or bottom"
            } else {
                "justify_u must be left, center or right"
            }
            .into());
        }
    };
    Ok(Some((placement, 0)))
}

pub(in crate::mcp) fn skybox_state(session: &mut Session) -> Value {
    session.sky_preview_state(session.view(), session.camera())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TieArgs {
    /// World brushes ('solid:<n>'); defaults to the selection.
    pub ids: Option<Vec<String>>,
    /// Brush entity class, e.g. func_detail, func_door, trigger_multiple.
    pub classname: String,
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteArgs {
    pub snapshot: String,
    /// Omit or use [0,0,0] to paste in place. World units, within ±65536.
    pub offset: Option<[f64; 3]>,
    pub texture_lock: Option<bool>,
    /// Explicitly retain outgoing external references by name; default false.
    pub keep_external_links: Option<bool>,
    /// Defaults to true only for an unchanged same-map Cut. Other pastes generate unique names.
    pub restore_cut_names: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabsArgs {
    pub query: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub selected: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectPrefabThumbnailArgs {
    pub name: String,
    /// flat (default) omits textures; textured admits bounded static opaque base textures.
    pub style: Option<crate::session::prefabs::ThumbnailStyle>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderPrefabThumbnailArgs {
    pub name: String,
    pub style: Option<crate::session::prefabs::ThumbnailStyle>,
    /// Rebuild pixels even when the exact source and content settings are cached.
    pub refresh: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderPrefabGalleryArgs {
    /// Reviewed get_prefabs.snapshot; stale catalog, selection or clipboard state rejects the page.
    pub snapshot: String,
    pub query: Option<String>,
    pub offset: Option<usize>,
    /// Number of entries, 1–8; default 8.
    pub limit: Option<usize>,
    /// flat (default) or textured; unsupported assets produce per-entry errors without substitution.
    pub style: Option<crate::session::prefabs::ThumbnailStyle>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavePrefabArgs {
    pub name: String,
    pub snapshot: String,
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabEntryArgs {
    pub name: String,
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenamePrefabArgs {
    /// Existing exact library entry name.
    pub name: String,
    /// New portable name, 1–80 UTF-8 bytes. Case-only changes are allowed.
    pub new_name: String,
    /// Latest get_prefabs.snapshot; stale library/map/selection state rejects.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayersArgs {
    pub query: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub selected: Option<u32>,
    pub member_offset: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginLayerArgs {
    pub id: Option<u32>,
    pub delete: Option<bool>,
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerTokenArgs {
    pub token: u64,
    pub version: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateLayerArgs {
    pub token: u64,
    pub version: u64,
    pub name: String,
    pub color: [u8; 3],
    pub parent: Option<u32>,
    pub members: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerSelectionArgs {
    pub id: u32,
    pub snapshot: String,
    pub recursive: Option<bool>,
    pub include_hidden: Option<bool>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerVisibilityArgs {
    pub id: u32,
    pub snapshot: String,
    pub recursive: Option<bool>,
    pub action: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StepsArgs {
    /// How many steps (default 1).
    pub steps: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InspectorArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextScopeArgs {
    /// map, solid:<id>, entity:<id>, region:<id>, or selection.
    /// begin_context_edit also accepts new_region.
    pub scope: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ContextNumber {
    Number(f64),
    Text(String),
}

impl ContextNumber {
    pub(in crate::mcp) fn text(self) -> String {
        match self {
            Self::Number(value) => source_formats::vmf::format_number(value),
            Self::Text(value) => value,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextEditArgs {
    pub token: String,
    pub summary: Option<String>,
    pub body: Option<String>,
    /// Region minimum XYZ; numbers or unfinished numeric strings.
    pub mins: Option<[ContextNumber; 3]>,
    pub maxs: Option<[ContextNumber; 3]>,
    /// Editor-only RGB guide color, with channels from 0 to 255.
    pub color: Option<[u8; 3]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextListArgs {
    pub query: Option<String>,
    /// map, object or region; omitted includes all scopes.
    pub kind: Option<String>,
    pub offset: Option<usize>,
    /// At most 100 summaries, default 30. Full text uses get_context_note.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextQueryArgs {
    pub object: Option<String>,
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
    pub point: Option<[f64; 3]>,
    pub include_map: Option<bool>,
    /// Serialized JSON response budget, 2048..65536 bytes; default 8192.
    /// A single note too large for the budget returns a summary with body_omitted.
    pub max_bytes: Option<usize>,
    /// Opaque next_cursor from the same query; stale map cursors are rejected.
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextDeleteArgs {
    pub scope: String,
    pub generation: u64,
    pub revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextFocusArgs {
    pub scope: String,
    pub aspect: Option<f32>,
}

pub(in crate::mcp) fn context_scope(
    session: &Session,
    value: &str,
) -> Result<map_core::context::Scope, String> {
    if value == "selection" {
        return session
            .selected_context_scope()
            .ok_or_else(|| "Select one object or faces belonging to one brush".into());
    }
    map_core::context::Scope::parse(value)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MapChecksArgs {
    /// Filter severity: error, warning or info.
    pub severity: Option<String>,
    /// Search issue codes, messages, IDs and source paths.
    pub query: Option<String>,
    pub offset: Option<usize>,
    /// Page size, capped at 200; default 50.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MapIssueArgs {
    /// Current report token returned by run_map_checks.
    pub report: String,
    /// Issue ID within that report.
    pub issue: String,
    #[serde(default)]
    pub frame: bool,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MapFixesArgs {
    pub report: String,
    /// Between 1 and 200 issue IDs with an advertised repair.
    pub issues: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MapFixTokenArgs {
    /// Exact token from the corresponding begin/preview call; stale or replaced drafts are rejected.
    pub token: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CompileTokenArgs {
    pub token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompileArgs {
    /// Output .bsp path; defaults to the current map filename with a .bsp extension.
    pub path: Option<String>,
    /// Generation from get_map_info; rejects replacement while allowing edits in the same buffer.
    pub expected_generation: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveBuildRecipeArgs {
    pub name: String,
    /// Omit to create; supply a custom recipe ID to replace with current settings.
    pub replace_id: Option<String>,
    /// Per-Session action revision from get_build_recipes; rejects changed recipes or local settings.
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BuildRecipeIdArgs {
    pub id: String,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadBuildRecipesArgs {
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileSettingsArgs {
    /// Binary alpha-test shadows from plain LightmappedGeneric world/func_detail faces and same-model lit receivers at their admitted initial baked pose; default false. Requires direct_ldr or direct_ldr_hdr. Model cutouts do not cast global shadows; opaque func_brush Shadows Yes requests use the separate entity property. Models do not supply reflected radiance. Independent of native model-requested prop texture shadows. Unsupported requested materials/scopes, fractional transparency and terrain alpha casters reject compilation without replacing prior output.
    pub brush_texture_shadows: Option<bool>,
    /// Default-off final-slot ambient probe refinement. Requires lit mode and Balanced or Detailed sampling; at most twice the candidate gathers, unchanged probe counts/spacing, independent LDR/HDR choices. No general quality or native-pixel guarantee.
    pub refine_ambient_probes: Option<bool>,
    /// Bake static-prop vertex lighting for admitted opaque or strict default-alpha-test selected VertexLitGeneric materials; default false. Cutout terminals allow only canonical $basetexture and literal $alphatest 1, with up to eight Patch include edges and at most one basetexture insert or replace per Patch. Every hardware vertex retains VHV alpha 255; receiver-only textures are unread unless casting or packing needs them. Requires direct_ldr or direct_ldr_hdr and bounces:0 through 4. Indirect reception uses the preceding world/terrain lighting pass and texture-header or authored material reflectivity. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention selected before dimension reads by trimmed %notooltexture 0 or 1 only. Empty/other values and failed required providers reject without fallback; invalid depths reject atomically. Unsupported receivers fail the requested build; disablevertexlighting:1 opts out one prop.
    pub bake_static_prop_vertices: Option<bool>,
    /// Pack the exact complete authored sky family and supported LDR/HDR dependencies; default false. Incomplete or unsupported families fail during compilation without automatic family fallback.
    pub pack_sky_assets: Option<bool>,
    /// Pack original files for complete supported static Water or opaque SLIME material/Patch/texture closures; default false. SLIME supports non-reflective static LightmappedGeneric, UnlitGeneric and unambiguous scalar Patch chains with an explicit basetexture and exact native self-bottom and only declared basetexture, bumpmap and tool 2D textures, including overridden textures; static fog/color/UV and reflectivity metadata are supported; envmaps, additional resources and effects reject. Water requires explicit static cubemaps with original LDR/HDR siblings throughout. Runtime/default env_cubemap and incomplete or unsupported graphs fail compilation. Available in every lighting mode without native capture.
    pub pack_water_assets: Option<bool>,
    /// Pack original bounded static MDL 44/48, VVD, DX90 VTX, every declared admitted opaque or default-alpha-test VertexLitGeneric material and basetexture VTF, plus matching PHY only for emitted VPhysics props. Patch chains allow eight include edges with one basetexture insert or replace per Patch; all declared textures, including overridden ones, stay packed. Deeper edits win within each type; replacements apply after all inserts. Default false; independent of vertex lighting and available in every lighting mode. Missing or unsupported inputs stop publication. Runtime requires standard CS:S shaders and surface/prop-data definitions with DX90 or newer.
    pub pack_static_prop_assets: Option<bool>,
    /// Generate one native reflection bounce per selected lighting channel; default false. Requires direct_ldr or direct_ldr_hdr; incompatible combinations reject atomically.
    pub generate_reflections: Option<bool>,
    /// Whole degrees 0–90; zero disables automatic smoothing on unassigned world/model faces.
    pub smoothing_angle: Option<u8>,
    /// Exact integer work units shared by preparation, LDR and HDR lighting. Range 1–10,000,000,000; default 1,000,000,000. Retains sampling quality and independent resource limits; omitted/null retains the current value.
    #[schemars(range(min = compiler::MIN_LIGHTING_WORK_LIMIT, max = compiler::MAX_LIGHTING_WORK_LIMIT))]
    pub lighting_work_limit: Option<u64>,
    /// 0 (direct only, default) through 4 diffuse world/terrain-source bounces into world/model and opted-in prop receivers; both lit modes.
    pub bounces: Option<u8>,
    /// fast (default conservative flood) or detailed (bounded portal separator clipping).
    pub visibility: Option<String>,
    pub sky_quality: Option<String>,
    /// fullbright, direct_ldr or direct_ldr_hdr; shared by synchronous, background and compile-and-play builds.
    #[schemars(extend("enum" = ["fullbright", "direct_ldr", "direct_ldr_hdr", null]))]
    pub lighting: Option<String>,
    /// none (default) or engine; engine uses the verified installed CS:S Linux64 library in an isolated worker.
    pub physics: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeCaptureRuntimeArgs {
    /// Verified CS:S installation root. Omit or null to clear its override and use the mounted game's verified default.
    pub engine_root: Option<String>,
    /// Existing Steam client root. Omit or null to clear its override and use the checked default.
    pub steam_root: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GameLogArgs {
    /// Tail lines per log (default 200, maximum 2000).
    pub lines: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileRegionPreviewArgs {
    /// Latest snapshot from get_compile_region.
    pub(in crate::mcp) snapshot: String,
    /// Remove region metadata; no bounds/enabled values may accompany clear.
    pub(in crate::mcp) clear: Option<bool>,
    /// Whether the applied region limits builds. Required unless clear is true.
    pub(in crate::mcp) enabled: Option<bool>,
    /// Minimum XYZ in world units. Required unless clear is true.
    pub(in crate::mcp) mins: Option<[f64; 3]>,
    /// Maximum XYZ in world units. Required unless clear is true.
    pub(in crate::mcp) maxs: Option<[f64; 3]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileRegionTokenArgs {
    pub(in crate::mcp) token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompileRegionFrameArgs {
    pub(in crate::mcp) aspect: Option<f32>,
}
