use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectCompiledVisibilityArgs {
    pub build_token: Option<String>,
    /// Includes position {source:{kind:position,position:[x,y,z]}} or position {source:{kind:camera}}; both require a retained build token.
    pub target: Option<crate::session::compiled_visibility::Target>,
    pub kind: Option<crate::session::compiled_visibility::Kind>,
    pub offset: Option<usize>,
    pub visibility_offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FrameCompiledVisibilityArgs {
    pub build_token: String,
    pub target: crate::session::compiled_visibility::Target,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct SetCompiledVisibilityOverlayArgs {
    pub build_token: Option<String>,
    pub target: Option<crate::session::compiled_visibility::Target>,
    /// Defaults to selected; destinations draws the complete native PVS/PAS world-leaf bounds set, capped at 512 leaves.
    #[serde(default)]
    pub mode: crate::session::compiled_visibility::GuideMode,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectSkyAssetsArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectCompiledWorldLightsArgs {
    /// Opaque successful-build token; required for nonzero offsets.
    pub build_token: Option<String>,
    /// ldr (default) or hdr; unavailable channels never fall back.
    pub channel: Option<String>,
    pub offset: Option<usize>,
    /// 1–64 native records; default 64.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FrameCompiledWorldLightArgs {
    pub build_token: String,
    pub channel: String,
    pub ordinal: usize,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct SelectCompiledWorldLightSourceArgs {
    pub build_token: String,
    pub channel: String,
    pub ordinal: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct SelectCompiledPropSourceArgs {
    pub build_token: String,
    /// Native zero-based static-prop ordinal from inspect_baked_prop_vertices.
    pub prop: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectBakedLightmapsArgs {
    pub channel: Option<String>,
    pub face: Option<usize>,
    pub source: Option<crate::session::baked_lightmaps::SourceReference>,
    #[serde(default)]
    pub selected_authored_face: bool,
    pub contributor_offset: Option<usize>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub style_index: Option<usize>,
    pub map_index: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FrameCompiledPropOriginArgs {
    pub build_token: String,
    /// Native zero-based static-prop ordinal, including disabled/non-baked receivers.
    pub prop: u32,
    /// Finite viewport aspect from 0.01 through 100; default 1.
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectBakedPropVerticesArgs {
    /// Opaque successful-build token; required for explicit prop/section or nonzero page offsets.
    pub build_token: Option<String>,
    /// ldr (default) or hdr; absent HDR never falls back to LDR.
    pub channel: Option<String>,
    /// Native zero-based sprp ordinal, including disabled-prop holes; not an authored entity ID.
    pub prop: Option<u32>,
    /// Ordered VHV hardware section index, not an MDL mesh number.
    pub section: Option<usize>,
    pub prop_offset: Option<usize>,
    /// 1–32 compiled props; default 32.
    pub prop_limit: Option<usize>,
    pub section_offset: Option<usize>,
    /// 1–32 hardware sections; default 32.
    pub section_limit: Option<usize>,
    pub vertex_offset: Option<usize>,
    /// 1–256 stored color rows within one section; default 256.
    pub vertex_limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PointfileLoadArgs {
    pub(in crate::mcp) path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PointfileQueryArgs {
    #[serde(default)]
    pub(in crate::mcp) offset: usize,
    pub(in crate::mcp) limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PointfileTokenArgs {
    pub(in crate::mcp) token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PointfileActionArgs {
    pub(in crate::mcp) token: Option<String>,
    pub(in crate::mcp) aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PointfileNavigateArgs {
    pub(in crate::mcp) index: usize,
    pub(in crate::mcp) token: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CameraSpec {
    /// "perspective", "top" (XY), "front" (XZ), or "side" (YZ).
    pub projection: Option<String>,
    /// Vertical span in Hammer units for a planar view, from 1 to 131072.
    pub ortho_height: Option<f32>,
    /// World-space center for a planar view; use position for perspective.
    pub center: Option<[f32; 3]>,
    /// Frame these objects (ids). Viewed from the editor camera's direction.
    pub focus: Option<Vec<String>>,
    /// Eye position [x, y, z].
    pub position: Option<[f32; 3]>,
    /// Point to look at from `position`; overrides yaw/pitch.
    pub look_at: Option<[f32; 3]>,
    /// Heading in degrees; 0 looks along +X, 90 along +Y.
    pub yaw: Option<f32>,
    /// Degrees above the horizon; negative looks down.
    pub pitch: Option<f32>,
    /// Vertical field of view in degrees.
    pub fov: Option<f32>,
}

impl CameraSpec {
    /// Starts from the editor camera and applies the given overrides.
    pub(in crate::mcp) fn resolve(&self, session: &Session, aspect: f32) -> Result<Camera, String> {
        self.resolve_with_instance_contents(session, aspect, None, None)
    }

    pub(in crate::mcp) fn resolve_with_instance_contents(
        &self,
        session: &Session,
        aspect: f32,
        instance_contents: Option<bool>,
        rope_preview: Option<bool>,
    ) -> Result<Camera, String> {
        if [self.fov, self.yaw, self.pitch, self.ortho_height]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
            || [self.position, self.center, self.look_at]
                .into_iter()
                .flatten()
                .any(|point| !Vec3::from_array(point).is_finite())
        {
            return Err("Camera coordinates and angles must be finite".into());
        }
        let mut camera = if let Some(projection) = &self.projection {
            session.camera_for_projection(
                Projection::parse(projection)
                    .ok_or("projection must be perspective, top, front or side")?,
            )
        } else {
            session.camera()
        };
        if let Some(height) = self.ortho_height {
            if !height.is_finite()
                || !(Camera::MIN_ORTHO_HEIGHT..=Camera::MAX_ORTHO_HEIGHT).contains(&height)
            {
                return Err("ortho_height must be between 1 and 131072 units".into());
            }
            camera.ortho_height = height;
        }
        if let Some(fov) = self.fov {
            camera.fov_y_degrees = fov.clamp(10.0, 150.0);
        }
        if let Some(focus) = &self.focus {
            let ids = parse_ids(focus)?;
            let (lo, hi) = session
                .framing_bounds_with_rope_preview(
                    session.render_document(),
                    &ids,
                    instance_contents,
                    rope_preview,
                )
                .ok_or("focus objects have no bounds")?;
            return Ok(camera.frame_bounds(lo.as_vec3(), hi.as_vec3(), aspect));
        }
        if let Some(center) = self.center {
            if camera.projection == Projection::Perspective || self.position.is_some() {
                return Err(
                    "center requires a planar view and cannot be combined with position".into(),
                );
            }
            camera = Camera::planar(
                camera.projection,
                Vec3::from_array(center),
                camera.ortho_height,
            );
        }
        if let Some(p) = self.position {
            camera.position = Vec3::from_array(p);
        }
        if camera.projection != Projection::Perspective {
            if self.look_at.is_some() || self.yaw.is_some() || self.pitch.is_some() {
                return Err(
                    "Planar views have fixed axes; use center, position and ortho_height".into(),
                );
            }
            return Ok(camera);
        }
        if let Some(target) = self.look_at {
            let fov = camera.fov_y_degrees;
            camera = Camera::looking_at(camera.position, Vec3::from_array(target));
            camera.fov_y_degrees = fov;
        } else {
            camera.yaw_degrees = self.yaw.unwrap_or(camera.yaw_degrees);
            camera.pitch_degrees = self
                .pitch
                .unwrap_or(camera.pitch_degrees)
                .clamp(-89.0, 89.0);
        }
        Ok(camera)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FrameArgs {
    /// Viewport width divided by height; defaults to 1 for conservative framing.
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VolumeFrameArgs {
    pub token: String,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FacePreviewArgs {
    /// Optional guard from get_face_selection.preview_token; rejects a replaced preview.
    pub token: Option<String>,
}

pub(in crate::mcp) fn view_state(view: ViewOptions) -> Value {
    json!({"instance_contents": view.instance_contents,"lighting_preview":view.lighting_preview,"lighting_sky_ambient":view.lighting_sky_ambient,"lighting_quality":view.lighting_quality.name(),"lighting_exposure_ev":view.lighting_exposure_ev,"normal_maps":view.normal_maps,"surface_shading":view.surface_shading,"tool_textures":view.tool_textures,"sky_preview":view.sky_preview,"sky3d_preview":view.sky3d_preview,"sky_channel":view.sky_channel.name(),"sky_exposure_ev":view.sky_exposure_ev,"rope_preview":view.rope_preview,"selection_outlines":view.selection_outlines,"selection_tint":view.selection_tint,"entity_images":view.entity_images,"additive_sprite_guides":view.additive_sprite_guides,"entity_model_guides":view.entity_model_guides,"fog":fog_parameters(view),"leak_path":view.leak_path,"surface":view.surface.name(),"smoothing_group":view.smoothing_group})
}

pub(in crate::mcp) fn fog_parameters(view: ViewOptions) -> Value {
    view.fog.map_or(Value::Null, |fog|json!({"color":fog.color(),"start":fog.start(),"end":fog.end(),"max_density":fog.max_density(),"enabled":fog.enabled()}))
}

pub(in crate::mcp) fn fog_controller_id(controller: Option<&str>) -> Result<Option<u32>, String> {
    controller
        .map(|id| match parse_id(id)? {
            ObjectId::Entity(id) => Ok(id),
            _ => Err("Fog controller must be an entity:<id>".into()),
        })
        .transpose()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FogPreviewArgs {
    pub enabled: bool,
    /// Explicit env_fog_controller entity:<id>. Required when enabling. No automatic Master selection.
    pub controller: Option<String>,
    /// Latest get_fog_preview.snapshot. Rejects map or view changes while reviewing the dialog.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct PickClipPointArgs {
    pub(in crate::mcp) x: f64,
    pub(in crate::mcp) y: f64,
    pub(in crate::mcp) width: Option<u32>,
    pub(in crate::mcp) height: Option<u32>,
    pub(in crate::mcp) camera: Option<CameraSpec>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepeatArrayArgs {
    /// Source objects; defaults to the selection. At most 16 distinct objects.
    pub ids: Option<Vec<String>>,
    /// Additional copies, excluding originals: 1–64, within the generated-object/side limits.
    pub count: u32,
    /// "world" (default) or "local". Local uses the first original source in stable object-ID order (brushes before entities), with the same frame for every copy.
    pub space: Option<String>,
    /// XYZ translation step in the chosen frame; copy n moves by n times this step.
    pub translation: Option<[f64; 3]>,
    /// XYZ degree step in the chosen frame; copy n rotates by n times this step about the fixed world pivot.
    pub rotation: Option<[f64; 3]>,
    /// Finite positive XYZ scale factors per copy, default [1,1,1]. Copy n uses factor^n on originals, before rotation/translation in the chosen frame about the fixed world pivot. Nonidentity factors require brushes or brush entities.
    pub scale: Option<[f64; 3]>,
    /// Fixed world pivot. Defaults to the source selection center.
    pub pivot: Option<[f64; 3]>,
    /// Keep textures attached to copied geometry. Defaults to true.
    pub texture_lock: Option<bool>,
    /// Latest get_repeat_array.snapshot. Invalid/stale requests retain the current draft.
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepeatArrayTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewPrefabArgs {
    pub name: String,
    pub snapshot: String,
    /// World position for the source pivot. Defaults to [0,0,0].
    pub position: Option<[f64; 3]>,
    /// World XYZ degrees, applied after scale about the source pivot. Defaults to [0,0,0].
    pub rotation: Option<[f64; 3]>,
    /// Finite positive world XYZ factors, default [1,1,1]. Nonidentity scale requires only brushes/brush entities.
    pub scale: Option<[f64; 3]>,
    /// Source-world pivot mapped to position. Defaults to the original authored bounds center.
    pub pivot: Option<[f64; 3]>,
    pub keep_external_links: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfacePlacementPickArgs {
    pub token: u64,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginSurfacePlacementPickArgs {
    pub ids: Option<Vec<String>>,
    pub method: Option<String>,
    pub source_corner: Option<SurfaceCornerRef>,
    #[serde(default)]
    pub align: bool,
    pub offset: Option<f64>,
    pub texture_lock: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetViewArgs {
    pub instance_contents: Option<bool>,
    /// Unshadowed authored constant point/spot/direct LDR sun preview; defaults off. One visible sun; optional LDR sky ambient, no HDR overrides/bounce/shadows/sky visibility.
    pub lighting_preview: Option<bool>,
    /// Include the admitted sun's unshadowed LDR sky ambient; defaults off. No sky visibility or occlusion.
    pub lighting_sky_ambient: Option<bool>,
    /// fast interpolates vertex lighting; detail evaluates fragments (default) and supports normal maps.
    pub lighting_quality: Option<String>,
    /// Editor exposure -8..8 EV (default 0), followed by per-channel Reinhard; no native tone-map claim.
    pub lighting_exposure_ev: Option<f64>,

    /// Opt-in authored mode5 Sprite resource guides with SRC_ALPHA/ONE RGB; raw guides remain the default. Requires Entity images.
    pub additive_sprite_guides: Option<bool>,
    /// Opt-in flat-brush normals under fixed editor light or Detail authored-light preview. Defaults off.
    pub normal_maps: Option<bool>,
    /// Stronger fixed ambient/directional sculpt-relief shading in Textured view. Defaults off, preserving ordinary weak shading. Authored-light receivers override it; tools/helpers remain unchanged.
    pub surface_shading: Option<bool>,
    /// Show selected object/face edges and model brackets. Defaults on; selection, picking and tool guides remain active.
    pub selection_outlines: Option<bool>,
    /// Tint selected brush faces and models without changing selection or outlines.
    pub selection_tint: Option<bool>,
    /// Show literal FGD icons and static Sprite resource guides; false keeps ordinary boxes without image admission.
    pub entity_images: Option<bool>,
    /// Show literal FGD studio/studioprop editor meshes; false keeps ordinary boxes.
    pub entity_model_guides: Option<bool>,
    /// Draw clip, trigger, nodraw and skybox brushes.
    pub tool_textures: Option<bool>,
    /// Preview the selected world sky on sky surfaces in perspective textured views. Requires all six supported faces of the explicit editor sky channel.
    pub sky_preview: Option<bool>,
    pub sky3d_preview: Option<bool>,
    /// Explicit editor sky channel: ldr (default) or hdr; never selects a game/build channel.
    pub sky_channel: Option<String>,
    /// HDR sky-only exposure in EV, finite -8 through 8; LDR pixels remain unchanged.
    pub sky_exposure_ev: Option<f64>,
    /// Show bounded static authored rope spans. No physics, parenting or attachment binding.
    pub rope_preview: Option<bool>,
    /// Show the current compilation's leak path through geometry.
    pub leak_path: Option<bool>,
    /// textured, flat, wireframe, lightmap_density, or smoothing_groups; works in every projection.
    pub surface: Option<String>,
    /// Highlight membership in this smoothing/hard-edge group, 1–32.
    pub smoothing_group: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenderArgs {
    pub instance_contents: Option<bool>,
    /// Unshadowed authored constant point/spot/direct LDR sun preview; defaults off. One visible sun; optional LDR sky ambient, no HDR overrides/bounce/shadows/sky visibility.
    pub lighting_preview: Option<bool>,
    /// Include the admitted sun's unshadowed LDR sky ambient; defaults off. No sky visibility or occlusion.
    pub lighting_sky_ambient: Option<bool>,
    /// fast interpolates vertex lighting; detail evaluates fragments (default) and supports normal maps.
    pub lighting_quality: Option<String>,
    /// Editor exposure -8..8 EV (default 0), followed by per-channel Reinhard; no native tone-map claim.
    pub lighting_exposure_ev: Option<f64>,

    /// Temporary authored Additive Sprite guide override; no shared settings or literal-icon changes.
    pub additive_sprite_guides: Option<bool>,
    /// Temporary flat-brush normal-map preview; never changes shared settings. Textured only, no terrain/models.
    pub normal_maps: Option<bool>,
    /// Temporary stronger fixed ambient/directional surface shading in Textured view; leaves shared settings unchanged. False preserves ordinary weak shading; authored-light receivers override it.
    pub surface_shading: Option<bool>,
    /// Temporary selected-edge/model-bracket visibility override; leaves shared settings and picking unchanged.
    pub selection_outlines: Option<bool>,
    /// Tint selected brush faces and models without changing selection or outlines.
    pub selection_tint: Option<bool>,
    /// Show literal FGD icons and static Sprite resource guides; false keeps ordinary boxes without image admission.
    pub entity_images: Option<bool>,
    /// Show literal FGD studio/studioprop editor meshes; false keeps ordinary boxes.
    pub entity_model_guides: Option<bool>,
    /// Camera overrides; defaults to the user's current view.
    pub camera: Option<CameraSpec>,
    /// Image size in pixels (default 1024x576).
    /// Include static policies and limit catalogues. Default false; statuses and diagnostics are always returned.
    #[serde(default)]
    pub include_details: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Draw tool brushes (nodraw, clip, trigger, skybox). Defaults to the editor setting.
    pub tool_textures: Option<bool>,
    /// Preview the selected world sky on sky surfaces in perspective textured views. Requires all six supported faces of the explicit editor sky channel.
    pub sky_preview: Option<bool>,
    pub sky3d_preview: Option<bool>,
    /// Explicit editor sky channel: ldr (default) or hdr; never selects a game/build channel.
    pub sky_channel: Option<String>,
    /// HDR sky-only exposure in EV, finite -8 through 8; LDR pixels remain unchanged.
    pub sky_exposure_ev: Option<f64>,
    /// Temporary static rope preview override; leaves shared view and picking unchanged.
    pub rope_preview: Option<bool>,
    /// Temporary static linear fog override. Requires a supported explicit or currently selected controller.
    pub fog_preview: Option<bool>,
    /// Temporary env_fog_controller entity:<id>; does not change shared settings.
    pub fog_controller: Option<String>,
    pub leak_path: Option<bool>,
    /// Optional viewport mode override; does not change the user's view.
    pub surface: Option<String>,
    pub smoothing_group: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PickArgs {
    pub instance_contents: Option<bool>,
    /// Pixel column and row in an image rendered with the same camera and size.
    pub x: f32,
    pub y: f32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraSpec>,
    /// Match a temporary render_view rope_preview override; defaults to the shared view.
    pub rope_preview: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RaycastArgs {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectHitArgs {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PrimitiveFrameArgs {
    pub token: String,
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextViewArgs {
    pub show_regions: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CameraViewSaveArgs {
    pub name: String,
    pub replace_id: Option<u32>,
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CameraViewIdArgs {
    pub id: u32,
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

pub(in crate::mcp) fn camera_view_guard(
    generation: Option<u64>,
    revision: Option<u64>,
) -> Result<Option<crate::session::saved_views::Guard>, String> {
    match (generation, revision) {
        (None, None) => Ok(None),
        (Some(generation), Some(revision)) => Ok(Some((generation, revision))),
        _ => Err("Supply both generation and revision from get_camera_views".into()),
    }
}

macro_rules! view_update {
    ($($args:ty),+) => { $(
        impl $args {
            pub(in crate::mcp) fn view_update(&self) -> crate::session::ViewUpdate<'_> {
                crate::session::ViewUpdate {
                    instance_contents: self.instance_contents,
                    lighting_preview: self.lighting_preview,
                    lighting_sky_ambient: self.lighting_sky_ambient,
                    lighting_quality: self.lighting_quality.as_deref(),
                    lighting_exposure_ev: self.lighting_exposure_ev,
                    additive_sprite_guides: self.additive_sprite_guides,
                    normal_maps: self.normal_maps,
                    surface_shading: self.surface_shading,
                    surface: self.surface.as_deref(),
                    smoothing_group: self.smoothing_group,
                    tool_textures: self.tool_textures,
                    leak_path: self.leak_path,
                    sky_preview: self.sky_preview,
                    sky3d_preview: self.sky3d_preview,
                    rope_preview: self.rope_preview,
                    selection_outlines: self.selection_outlines,
                    selection_tint: self.selection_tint,
                    entity_images: self.entity_images,
                    entity_model_guides: self.entity_model_guides,
                    sky_channel: self.sky_channel.as_deref(),
                    sky_exposure_ev: self.sky_exposure_ev,
                }
            }
        }
    )+ };
}
view_update!(SetViewArgs, RenderArgs);
