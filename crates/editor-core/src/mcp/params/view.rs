use super::*;

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
                .object_bounds_in(session.render_document(), &ids)
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
    json!({"tool_textures":view.tool_textures,"sky_preview":view.sky_preview,"fog":fog_parameters(view),"leak_path":view.leak_path,"surface":view.surface.name(),"smoothing_group":view.smoothing_group})
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
    pub position: Option<[f64; 3]>,
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
    /// Draw clip, trigger, nodraw and skybox brushes.
    pub tool_textures: Option<bool>,
    /// Preview the selected world sky on sky surfaces in perspective textured views. Requires all six LDR faces.
    pub sky_preview: Option<bool>,
    /// Show the current compilation's leak path through geometry.
    pub leak_path: Option<bool>,
    /// textured, flat, wireframe, lightmap_density, or smoothing_groups; works in every projection.
    pub surface: Option<String>,
    /// Highlight membership in this smoothing/hard-edge group, 1–32.
    pub smoothing_group: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenderArgs {
    /// Camera overrides; defaults to the user's current view.
    pub camera: Option<CameraSpec>,
    /// Image size in pixels (default 1024x576).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Draw tool brushes (nodraw, clip, trigger, skybox). Defaults to the editor setting.
    pub tool_textures: Option<bool>,
    /// Preview the selected world sky on sky surfaces in perspective textured views. Requires all six LDR faces.
    pub sky_preview: Option<bool>,
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
    /// Pixel column and row in an image rendered with the same camera and size.
    pub x: f32,
    pub y: f32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraSpec>,
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
