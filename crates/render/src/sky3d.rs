use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, Mat4, Vec3};
use serde::Serialize;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::{ImageFormat, Vtf};

use crate::materials::MaterialInfo;
use crate::scene::{Batch, Vertex};

type AssetReader<'a> = dyn FnMut(&str, usize) -> Result<Option<Vec<u8>>, String> + 'a;

pub const MAX_VERTICES: usize = 65_536;
pub const MAX_INDICES: usize = 196_608;
pub const MAX_SURFACES: usize = 16_384;
pub const MAX_MATERIALS: usize = 64;
pub const MAX_SCALE: f64 = 128.0;
pub const MAX_CPU_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_GPU_ASSET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_GPU_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TARGET_BYTES: usize = 48 * 1024 * 1024;
pub const MAX_GPU_BYTES: usize = MAX_GPU_ASSET_BYTES + MAX_GPU_UPLOAD_BYTES + MAX_TARGET_BYTES;
pub const MAX_TARGET_DIMENSION: u32 = 2048;
pub const MAX_TEXTURE_DIMENSION: u32 = 2048;
const MAX_VMT_BYTES: usize = 256 * 1024;
const MAX_VTF_BYTES: usize = 8 * 1024 * 1024;
const MAX_ASSET_READS: usize = 640;
const MATERIAL_RESERVE: usize = 64 * 1024;
const PARSER_RESERVE: usize = 8 * 1024 * 1024;
const GPU_BINDING_RESERVE: usize = 4096;
const VMT_ALLOCATION_FACTOR: usize = 8;
const LEGACY_TARGET_PIXEL_BYTES: usize = 8;
const HDR_TARGET_PIXEL_BYTES: usize = 12;
static NEXT_SCENE: AtomicU64 = AtomicU64::new(1);

pub const POLICY: &str = "Editor 3D sky preview: explicitly selected sealed world/func_detail miniature with flat surfaces, admitted whole authored fine terrain patches and whole fixed prop_static LOD0 meshes; perspective Textured views only. Camera origin + main eye / integer scale 1..128; unchanged direction/FOV, no entity-angle rotation. Editor near plane 4/scale miniature units preserves main-equivalent clipping; native Source uses a different near policy. Static single-base LightmappedGeneric/UnlitGeneric world/terrain materials and opaque qualified VertexLitGeneric fixed-model materials, with checked axis-aligned static base-texture transforms; finite center/scale/translation values within ±1024, last duplicate operation wins, nonzero rotation rejected; flat world/func_detail surfaces may use plain alpha-tested cutouts with validated received-float defaults and native byte-quantized references, or static $translucent 1 with texture alpha; terrain and models remain opaque. Alpha testing and translucency are mutually exclusive. No additive/modulated coverage, proxies, normal maps, detail/blend layers, authored lighting or runtime effects. Alpha brushes do not seal the selected component; opaque brushes must enclose it. Translucent batches draw after opaque/cutout batches, with depth testing, no depth writes and stable back-to-front sorting by actual batch AABB center depth in miniature coordinates. Intersecting or cyclic translucent surfaces can blend in the wrong order; no native BSP transparency ordering is claimed. Straight fogged RGB and texture alpha accumulate into the existing premultiplied miniature target; its main composite preserves the remaining 2D background contribution. HDR exposure follows linear composition. Terrain uses BASE texture coordinates and shared vertex normals. Fixed models retain authored normalized UVs and transformed source normals through the shared static-prop pose. Partial-patch/model clipping, native model/terrain LOD appearance and baked model lighting are unsupported. The selected sky_camera supplies independent static LDR fog, evaluated in miniature coordinates after one reciprocal range scaling and the current initial radial context; no world controller/water/timeline/far-Z inheritance. Direction uses the checked received 1/32 coordinate grid before biased normalization. Standard Sky 2D backgrounds remain unchanged; combined fog with UnlitGeneric 2D backgrounds requires explicit $nofog 1. HDR scenes compose miniature fog before the shared manual exposure; no native clipping, repeated client draw-history normalization or pixel parity. Missing/unsupported requested assets fail eligibility. Shared Session preparation captures consumed material/texture sources and retires readiness when their original files or winning lookup paths change; content-catalogue refresh remains explicit. SKY receives the miniature; SKY2D retains only the existing 2D sky path. Empty miniature pixels retain the chosen 2D sky when its independent preview is enabled and ready, otherwise the regular authored sky tool surface. Mixed SKY/SKY2D is an editor policy, not native mixed-sky parity. No helpers, selection, guides, projected picking or editing. Limits: 65536 vertices,196608 indices,16384 surfaces,64 materials,64 MiB conservative CPU builder allocation,32 MiB retained GPU geometry/material payload shared across scenes,64 MiB separately charged transient uploads and one 32 MiB legacy or48 MiB HDR color/depth target. The 144 MiB explicit payload/upload/target budget excludes driver allocation overhead and is not a total VRAM ceiling. Retired identities flush pending uploads and wait at most one second for GPU completion before replacement admission. Uploads conservatively charge RGBA rows padded to256 bytes for every mip plus geometry/uniform bytes; live matching identities reuse bindings. Targets/textures are at most2048 pixels per axis. Targets downsample larger viewports preserving aspect; texture assets never downsample or substitute.";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub origin: DVec3,
    pub scale: f64,
}

impl Camera {
    pub fn validate(self) -> Result<(), String> {
        if !self.origin.is_finite()
            || !self.origin.as_vec3().is_finite()
            || !self.scale.is_finite()
            || self.scale.fract() != 0.0
            || !(1.0..=MAX_SCALE).contains(&self.scale)
        {
            return Err("3D sky camera requires finite origin and integer scale 1..128".into());
        }
        Ok(())
    }

    pub fn transform(self, main: &crate::Camera) -> Result<crate::Camera, String> {
        self.validate()?;
        let position = (self.origin + main.position.as_dvec3() / self.scale).as_vec3();
        if main.projection != crate::Projection::Perspective
            || !position.is_finite()
            || !main.yaw_degrees.is_finite()
            || !main.pitch_degrees.is_finite()
            || !main.fov_y_degrees.is_finite()
            || !(0.0..180.0).contains(&main.fov_y_degrees)
            || main.fov_y_degrees == 0.0
        {
            return Err("3D sky preview requires a finite perspective camera".into());
        }
        Ok(crate::Camera { position, ..*main })
    }

    pub fn view_projection(self, main: &crate::Camera, aspect: f32) -> Result<Mat4, String> {
        use glam::camera::rh::{proj::directx, view};

        let camera = self.transform(main)?;
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("3D sky preview requires a positive finite target aspect".into());
        }
        let view = view::look_at_mat4(
            camera.position,
            camera.position + camera.forward(),
            camera.up(),
        );
        let projection = directx::perspective_infinite_reverse(
            camera.fov_y_degrees.to_radians(),
            aspect,
            crate::Camera::NEAR / self.scale as f32,
        );
        let matrix = projection * view;
        if !matrix.is_finite() {
            return Err("3D sky camera projection exceeds finite range".into());
        }
        Ok(matrix)
    }
}

#[derive(Debug, Clone)]
pub struct Surface {
    pub points: Vec<DVec3>,
    pub texture_points: Option<Vec<DVec3>>,
    pub normalized_uvs: Option<Vec<[f32; 2]>>,
    pub normals: Option<Vec<DVec3>>,
    pub model: bool,
    pub normal: DVec3,
    pub material: String,
    pub texture_u: [f64; 4],
    pub texture_v: [f64; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MaterialUse {
    Flat,
    Terrain,
    Model,
}

impl Surface {
    fn material_use(&self) -> Result<MaterialUse, String> {
        match (
            self.model,
            self.texture_points.is_some(),
            self.normalized_uvs.is_some(),
            self.normals.is_some(),
        ) {
            (false, false, false, false) => Ok(MaterialUse::Flat),
            (false, true, false, true) => Ok(MaterialUse::Terrain),
            (true, false, true, true) => Ok(MaterialUse::Model),
            _ => {
                Err("3D sky surface has inconsistent model, UV or vertex-normal attributes".into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Stats {
    pub vertices: usize,
    pub indices: usize,
    pub surfaces: usize,
    pub materials: usize,
    pub cpu_bytes: usize,
    pub gpu_bytes: usize,
    pub upload_bytes: usize,
    pub source_attribute_bytes: usize,
    pub draw_metadata_bytes: usize,
    pub translucent_surfaces: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Draw {
    pub batch: Batch,
    pub center: Vec3,
    pub translucent: bool,
}

#[derive(Debug)]
pub struct Scene {
    pub camera: Camera,
    pub fog: crate::fog::Profile,
    pub stats: Stats,
    pub(crate) id: u64,
    pub(crate) gpu_bytes: usize,
    pub(crate) upload_bytes: usize,
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
    pub(crate) batches: Vec<Draw>,
    pub(crate) materials: Vec<MaterialBinding>,
}

#[derive(Debug)]
pub(crate) struct MaterialBinding {
    pub info: MaterialInfo,
    pub unlit: bool,
    pub alpha_reference: Option<f32>,
    pub uv_transform: [f32; 4],
}

#[derive(Default)]
struct Budget {
    cpu: usize,
    gpu: usize,
    reads: usize,
    upload: usize,
    attributes: usize,
    draws: usize,
}

impl Budget {
    fn cpu(&mut self, bytes: usize) -> Result<(), String> {
        self.cpu = checked_sum(self.cpu, bytes, MAX_CPU_BYTES, "CPU allocation")?;
        Ok(())
    }

    fn gpu(&mut self, bytes: usize) -> Result<(), String> {
        self.gpu = checked_sum(
            self.gpu,
            bytes,
            MAX_GPU_ASSET_BYTES,
            "retained GPU geometry/material payload",
        )?;
        Ok(())
    }

    fn upload(&mut self, bytes: usize) -> Result<(), String> {
        self.upload = checked_sum(
            self.upload,
            bytes,
            MAX_GPU_UPLOAD_BYTES,
            "transient GPU upload allocation",
        )?;
        Ok(())
    }

    fn read(
        &mut self,
        read: &mut AssetReader<'_>,
        path: &str,
        vmt: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        self.reads = checked_sum(self.reads, 1, MAX_ASSET_READS, "asset reads")?;
        let factor = if vmt { VMT_ALLOCATION_FACTOR } else { 1 };
        let maximum = ((MAX_CPU_BYTES - self.cpu) / factor).min(if vmt {
            MAX_VMT_BYTES
        } else {
            MAX_VTF_BYTES
        });
        if maximum == 0 {
            return Err("3D sky CPU allocation budget exhausted before asset read".into());
        }
        let bytes = read(
            &asset_relative(path).map_err(|error| error.to_string())?,
            maximum,
        )
        .map_err(|error| format!("3D sky asset {path}: {error}"))?;
        if let Some(bytes) = &bytes {
            if bytes.len() > maximum {
                return Err(format!(
                    "3D sky asset {path} exceeded its source read limit {maximum}"
                ));
            }
            self.cpu(bytes.len() * factor)?;
        }
        Ok(bytes)
    }
}

fn checked_sum(value: usize, addition: usize, limit: usize, label: &str) -> Result<usize, String> {
    value
        .checked_add(addition)
        .filter(|total| *total <= limit)
        .ok_or_else(|| format!("3D sky {label} exceeds budget {limit}"))
}

fn rgba_upload_bytes(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("3D sky transient GPU upload requires nonzero dimensions".into());
    }
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let bytes = (width as usize)
        .checked_mul(4)
        .and_then(|row| row.checked_add(alignment - 1))
        .map(|row| row / alignment * alignment)
        .and_then(|row| row.checked_mul(height as usize))
        .ok_or_else(|| "3D sky transient GPU upload size overflow".to_owned())?;
    checked_sum(
        0,
        bytes,
        MAX_GPU_UPLOAD_BYTES,
        "transient GPU upload allocation",
    )
}

fn geometry_budget(surfaces: &[Surface]) -> Result<(usize, usize, Budget), String> {
    if surfaces.len() > MAX_SURFACES {
        return Err(format!("3D sky surfaces exceed budget {MAX_SURFACES}"));
    }
    let mut vertices = 0;
    let mut indices = 0;
    let mut attributes = 0;
    for surface in surfaces {
        let count = surface.points.len();
        if count < 3 {
            return Err("3D sky surface polygon requires at least three vertices".into());
        }
        vertices = checked_sum(vertices, count, MAX_VERTICES, "vertices")?;
        indices = checked_sum(indices, (count - 2) * 3, MAX_INDICES, "indices")?;
        surface.material_use()?;
        attributes = checked_sum(
            attributes,
            2 * size_of::<Option<Vec<DVec3>>>()
                + size_of::<Option<Vec<[f32; 2]>>>()
                + size_of::<usize>(),
            MAX_CPU_BYTES,
            "surface attribute allocation",
        )?;
        if let Some(points) = &surface.texture_points {
            if points.len() != count || points.iter().any(|point| !point.is_finite()) {
                return Err("3D sky surface has invalid texture-point attributes".into());
            }
            attributes = checked_sum(
                attributes,
                points.len() * size_of::<DVec3>(),
                MAX_CPU_BYTES,
                "surface attribute allocation",
            )?;
        }
        if let Some(normals) = &surface.normals {
            if normals.len() != count || normals.iter().any(|normal| !valid_vertex_normal(*normal))
            {
                return Err("3D sky surface has invalid vertex-normal attributes".into());
            }
            attributes = checked_sum(
                attributes,
                normals.len() * size_of::<DVec3>(),
                MAX_CPU_BYTES,
                "surface attribute allocation",
            )?;
        }
        if let Some(uvs) = &surface.normalized_uvs {
            if uvs.len() != count || uvs.iter().flatten().any(|value| !value.is_finite()) {
                return Err("3D sky model surface has invalid normalized UV attributes".into());
            }
            attributes = checked_sum(
                attributes,
                uvs.len() * size_of::<[f32; 2]>(),
                MAX_CPU_BYTES,
                "surface attribute allocation",
            )?;
        }
        if !surface.normal.is_finite()
            || !surface.normal.length_squared().is_finite()
            || surface.normal.length_squared() < 1e-12
            || surface.material.len() > 1024
            || surface.material.trim().is_empty()
            || surface
                .texture_u
                .iter()
                .chain(&surface.texture_v)
                .any(|value| !value.is_finite())
            || surface
                .points
                .iter()
                .any(|point| !point.is_finite() || !point.as_vec3().is_finite())
        {
            return Err("3D sky surface has invalid polygon, normal, UV axes or material".into());
        }
    }
    let geometry = vertices * size_of::<Vertex>() + indices * size_of::<u32>();
    let mut budget = Budget::default();
    let draws = surfaces.len() * (2 * size_of::<Draw>() + 2 * size_of::<&Draw>())
        + 2 * size_of::<Vec<Draw>>()
        + size_of::<Vec<&Draw>>();
    budget.cpu(PARSER_RESERVE + geometry * 2 + surfaces.len() * 128 + draws)?;
    budget.cpu(attributes)?;
    budget.attributes = attributes;
    budget.draws = draws;
    budget.gpu(geometry + GPU_BINDING_RESERVE)?;
    budget.upload(geometry + GPU_BINDING_RESERVE)?;
    Ok((vertices, indices, budget))
}

fn valid_vertex_normal(normal: DVec3) -> bool {
    let length = normal.length_squared();
    if !normal.is_finite() || !length.is_finite() || length <= 0.0 {
        return false;
    }
    let packed = normal.as_vec3();
    let packed_length = packed.length_squared();
    packed.is_finite() && packed_length.is_finite() && packed_length > 0.0
}

fn plain_alpha_material(bytes: &[u8]) -> Result<Material, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let nodes = source_formats::keyvalues::parse_static_material_bounded(text, 3, 4096)
        .map_err(|error| error.to_string())?;
    let [root] = nodes.as_slice() else {
        return Err("alpha surfaces require one plain material root".into());
    };
    if !root.key.eq_ignore_ascii_case("LightmappedGeneric")
        && !root.key.eq_ignore_ascii_case("UnlitGeneric")
    {
        return Err(
            "alpha surfaces require a plain LightmappedGeneric or UnlitGeneric material".into(),
        );
    }
    let children = root
        .children()
        .ok_or("alpha surfaces require a shader block")?;
    let mut keys = BTreeSet::new();
    for node in children {
        let key = node.key.to_ascii_lowercase();
        if !keys.insert(key.clone()) {
            return Err("alpha surfaces require unique scalar material parameters".into());
        }
        node.as_str()
            .ok_or("alpha surfaces require scalar material parameters")?;
    }
    let parsed = Material::parse(text).map_err(|error| error.to_string())?;
    if parsed.shader != root.key.to_ascii_lowercase()
        || parsed.params.len() != children.len()
        || parsed
            .params
            .iter()
            .zip(children)
            .any(|((key, value), node)| {
                !node.key.eq_ignore_ascii_case(key) || node.as_str() != Some(value.as_str())
            })
    {
        return Err("alpha surface raw and parsed scalar parameters disagree".into());
    }
    Ok(parsed)
}

fn load_material(
    read: &mut AssetReader<'_>,
    name: &str,
    usage: MaterialUse,
    budget: &mut Budget,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<Option<MaterialBinding>, String> {
    let model = usage == MaterialUse::Model;
    let mut first_source = true;
    let mut alpha_source = None;
    let material = Material::load_with_reader(&material_path(name), &mut |path| {
        checkpoint().map_err(std::io::Error::other)?;
        let bytes = budget
            .read(read, path, true)
            .map_err(std::io::Error::other)?;
        if first_source {
            first_source = false;
            alpha_source = bytes.as_ref().map(|bytes| plain_alpha_material(bytes));
        }
        Ok(bytes)
    })
    .map_err(|error| format!("3D sky material {name}: {error}"))?
    .ok_or_else(|| format!("3D sky material {name} is missing"))?;
    if material
        .get("%detailtype")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(format!(
            "3D sky material {name} requests unsupported generated detail geometry"
        ));
    }
    let semantics = material.compile_semantics();
    if model
        && semantics.surface
            & (source_formats::bsp::surf::SKY
                | source_formats::bsp::surf::SKY2D
                | source_formats::bsp::surf::NODRAW)
            != 0
    {
        return Err(format!(
            "3D sky model material {name} has unsupported sky or invisible semantics"
        ));
    }
    if semantics.surface & source_formats::bsp::surf::SKY != 0 {
        return Ok(None);
    }
    if !semantics.draws || material.is_tool() {
        return Err(format!(
            "3D sky material {name} is an unsupported tool surface"
        ));
    }
    let supported_shader = if model {
        material.shader == "vertexlitgeneric"
    } else {
        matches!(
            material.shader.as_str(),
            "lightmappedgeneric" | "unlitgeneric"
        )
    };
    let cutout = material.alpha_test();
    let translucent = material.translucent();
    if cutout && translucent {
        return Err(format!(
            "3D sky material {name} cannot combine alpha testing and translucency"
        ));
    }
    if !supported_shader
        || !material.blocks.is_empty()
        || ((cutout || translucent) && usage != MaterialUse::Flat)
    {
        let shaders = if model {
            "VertexLitGeneric"
        } else {
            "LightmappedGeneric or UnlitGeneric"
        };
        let coverage = if usage == MaterialUse::Flat {
            "static"
        } else {
            "static opaque"
        };
        return Err(format!(
            "3D sky material {name} requires {coverage} {shaders}"
        ));
    }
    let mut alpha_reference = None;
    if cutout || translucent {
        let flag = if cutout { "$alphatest" } else { "$translucent" };
        if material
            .get(flag)
            .and_then(|value| value.trim().parse::<f64>().ok())
            != Some(1.0)
        {
            return Err(format!(
                "3D sky alpha material {name} requires scalar {flag} 1"
            ));
        }
        let raw = alpha_source
            .ok_or_else(|| format!("3D sky alpha material {name} has no original material source"))?
            .map_err(|error| format!("3D sky alpha material {name}: {error}"))?;
        if raw.shader != material.shader || raw.params != material.params {
            return Err(format!(
                "3D sky alpha material {name} has inconsistent raw and resolved parameters"
            ));
        }
    }
    if cutout {
        let reference = material
            .alpha_test_reference_byte()
            .map_err(|error| format!("3D sky cutout material {name}: {error}"))?;
        alpha_reference = Some(f32::from(reference) / 255.0);
    }
    for (key, value) in &material.params {
        if !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
                | "%detailtype"
                | "%compilepassbullets"
        ) && !(model && key == "$model" && value.trim() == "1")
            && !(cutout && matches!(key.as_str(), "$alphatest" | "$alphatestreference"))
            && !(translucent && key == "$translucent")
        {
            return Err(format!(
                "3D sky material {name} has unsupported parameter {key}"
            ));
        }
    }
    let uv_transform = crate::sky::texture_transform(material.get("$basetexturetransform"))
        .map_err(|error| format!("3D sky material {name}: {error}"))?;
    let texture = material
        .get("$basetexture")
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| format!("3D sky material {name} has no base texture"))?;
    let path = texture_path(texture);
    checkpoint()?;
    let bytes = budget
        .read(read, &path, false)?
        .ok_or_else(|| format!("3D sky base texture {path} is missing"))?;
    let texture =
        Vtf::parse(bytes).map_err(|error| format!("3D sky base texture {path}: {error}"))?;
    if texture.frames != 1
        || texture.faces != 1
        || texture.depth != 1
        || texture.width > MAX_TEXTURE_DIMENSION
        || texture.height > MAX_TEXTURE_DIMENSION
        || !matches!(
            texture.format,
            ImageFormat::Rgb888
                | ImageFormat::Bgr888
                | ImageFormat::Rgba8888
                | ImageFormat::Bgra8888
                | ImageFormat::Bgrx8888
                | ImageFormat::Dxt1
                | ImageFormat::Dxt3
                | ImageFormat::Dxt5
        )
    {
        return Err(format!(
            "3D sky base texture {path} requires static 2D LDR RGB/RGBA/DXT, at most {MAX_TEXTURE_DIMENSION} pixels per axis"
        ));
    }
    let mut decoded_bytes = 0;
    let mut upload_bytes = 0;
    for mip in 0..texture.mip_count {
        let (width, height) = texture.mip_size(mip);
        decoded_bytes += width as usize * height as usize * 4;
        upload_bytes = checked_sum(
            upload_bytes,
            rgba_upload_bytes(width, height)?,
            MAX_GPU_UPLOAD_BYTES,
            "transient GPU upload allocation",
        )?;
        texture
            .image_data(mip, 0, 0)
            .map_err(|error| format!("3D sky texture {path}: {error}"))?;
    }
    budget.cpu(decoded_bytes * 2 + MATERIAL_RESERVE)?;
    budget.gpu(decoded_bytes + GPU_BINDING_RESERVE)?;
    budget.upload(upload_bytes + GPU_BINDING_RESERVE)?;
    for mip in 0..texture.mip_count {
        checkpoint()?;
        texture
            .decode_rgba8(mip, 0, 0)
            .map_err(|error| format!("3D sky texture {path}: {error}"))?;
    }
    Ok(Some(MaterialBinding {
        info: MaterialInfo::from_resolved(name, Some(&material), Some(texture), None, Some(path)),
        unlit: material.shader == "unlitgeneric",
        alpha_reference,
        uv_transform,
    }))
}

fn no_checkpoint() -> Result<(), String> {
    Ok(())
}

impl Scene {
    pub fn build(camera: Camera, surfaces: &[Surface], fs: &GameFs) -> Result<Self, String> {
        Self::build_controlled(
            camera,
            crate::fog::Profile::initial(),
            surfaces,
            fs,
            no_checkpoint,
        )
    }

    pub fn build_controlled(
        camera: Camera,
        fog: crate::fog::Profile,
        surfaces: &[Surface],
        fs: &GameFs,
        checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::build_controlled_with_reader(
            camera,
            fog,
            surfaces,
            |path, maximum| {
                fs.read_limited(path, maximum)
                    .map_err(|error| error.to_string())
            },
            checkpoint,
        )
    }

    pub fn build_controlled_with_reader(
        camera: Camera,
        fog: crate::fog::Profile,
        surfaces: &[Surface],
        mut read: impl FnMut(&str, usize) -> Result<Option<Vec<u8>>, String>,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        checkpoint()?;
        camera.validate()?;
        crate::fog::sky_view(fog, camera.scale as f32, Default::default())?;
        let (vertex_count, index_count, mut budget) = geometry_budget(surfaces)?;
        let mut names = BTreeMap::new();
        let mut materials = Vec::new();
        for surface in surfaces {
            checkpoint()?;
            let name = surface.material.trim().to_ascii_lowercase();
            let usage = surface.material_use()?;
            let key = (name.clone(), usage);
            if names.contains_key(&key) {
                continue;
            }
            if names.len() >= MAX_MATERIALS {
                return Err(format!("3D sky materials exceed budget {MAX_MATERIALS}"));
            }
            let material = load_material(&mut read, &name, usage, &mut budget, &mut checkpoint)?;
            let index = material.map(|material| {
                let index = materials.len();
                materials.push(material);
                index
            });
            names.insert(key, index);
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        let mut indices = Vec::with_capacity(index_count);
        let mut batches = Vec::with_capacity(surfaces.len());
        for surface in surfaces {
            checkpoint()?;
            let key = (
                surface.material.trim().to_ascii_lowercase(),
                surface.material_use()?,
            );
            let Some(material) = names[&key] else {
                continue;
            };
            let info = &materials[material].info;
            let transform = materials[material].uv_transform;
            let flat_normal = surface.normal.normalize().as_vec3().to_array();
            let first = vertices.len() as u32;
            for (index, point) in surface.points.iter().enumerate() {
                let texture_point = surface
                    .texture_points
                    .as_ref()
                    .map_or(point, |points| &points[index]);
                let normal = surface
                    .normals
                    .as_ref()
                    .map_or(flat_normal, |normals| normals[index].as_vec3().to_array());
                let project = |axis: [f64; 4], size: u32| {
                    ((texture_point.dot(DVec3::new(axis[0], axis[1], axis[2])) + axis[3])
                        / f64::from(size)) as f32
                };
                let uv = surface.normalized_uvs.as_ref().map_or_else(
                    || {
                        [
                            project(surface.texture_u, info.width),
                            project(surface.texture_v, info.height),
                        ]
                    },
                    |uvs| uvs[index],
                );
                let uv = [
                    uv[0] * transform[0] + transform[2],
                    uv[1] * transform[1] + transform[3],
                ];
                if uv.iter().any(|value| !value.is_finite()) {
                    return Err("3D sky surface texture coordinates exceed finite range".into());
                }
                vertices.push(Vertex {
                    position: point.as_vec3().to_array(),
                    normal,
                    uv,
                    color: [255; 4],
                    lightmap: [0.0, 0.0, -1.0],
                    smoothing: 0,
                    opacity: 1.0,
                    texture_alpha: 0,
                    tangent: [0.0; 4],
                    baked_irradiance: [0.0; 3],
                    lighting_receiver: 0,
                });
            }
            let first_index = indices.len() as u32;
            for index in 1..surface.points.len() - 1 {
                let cross = (surface.points[index] - surface.points[0])
                    .cross(surface.points[index + 1] - surface.points[0]);
                let [a, b] = if cross.dot(surface.normal) > 0.0 {
                    [index + 1, index]
                } else {
                    [index, index + 1]
                };
                indices.extend_from_slice(&[first, first + a as u32, first + b as u32]);
            }
            let (mins, maxs) = vertices[first as usize..]
                .iter()
                .map(|vertex| Vec3::from_array(vertex.position).as_dvec3())
                .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), point| {
                    (mins.min(point), maxs.max(point))
                });
            let center = ((mins + maxs) * 0.5).as_vec3();
            if !center.is_finite() {
                return Err("3D sky surface has a nonfinite draw center".into());
            }
            batches.push(Draw {
                batch: Batch {
                    material,
                    first_index,
                    index_count: indices.len() as u32 - first_index,
                    rope: false,
                    model_translucent: false,
                },
                center,
                translucent: info.translucent,
            });
        }
        checkpoint()?;
        Ok(Self {
            camera,
            fog,
            stats: Stats {
                vertices: vertices.len(),
                indices: indices.len(),
                surfaces: batches.len(),
                materials: materials.len(),
                cpu_bytes: budget.cpu,
                gpu_bytes: budget.gpu,
                upload_bytes: budget.upload,
                source_attribute_bytes: budget.attributes,
                draw_metadata_bytes: budget.draws,
                translucent_surfaces: batches.iter().filter(|draw| draw.translucent).count(),
            },
            id: NEXT_SCENE.fetch_add(1, Ordering::Relaxed),
            gpu_bytes: budget.gpu,
            upload_bytes: budget.upload,
            vertices,
            indices,
            batches,
            materials,
        })
    }
}

pub fn target_size(size: (u32, u32), gpu_asset_bytes: usize) -> Result<(u32, u32), String> {
    target_size_for_display(size, gpu_asset_bytes, false)
}

pub fn target_size_for_display(
    size: (u32, u32),
    gpu_asset_bytes: usize,
    hdr: bool,
) -> Result<(u32, u32), String> {
    if size.0 == 0 || size.1 == 0 || gpu_asset_bytes > MAX_GPU_ASSET_BYTES {
        return Err("3D sky target requires nonzero dimensions and admitted GPU assets".into());
    }
    let ratio = (f64::from(MAX_TARGET_DIMENSION) / f64::from(size.0))
        .min(f64::from(MAX_TARGET_DIMENSION) / f64::from(size.1))
        .min(1.0);
    let width = (f64::from(size.0) * ratio).floor().max(1.0) as u32;
    let height = (f64::from(size.1) * ratio).floor().max(1.0) as u32;
    let pixel_bytes = if hdr {
        HDR_TARGET_PIXEL_BYTES
    } else {
        LEGACY_TARGET_PIXEL_BYTES
    };
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(pixel_bytes))
        .ok_or("3D sky target allocation overflowed")?;
    checked_sum(0, bytes, MAX_TARGET_BYTES, "color/depth target allocation")?;
    checked_sum(
        gpu_asset_bytes,
        bytes,
        MAX_GPU_BYTES - MAX_GPU_UPLOAD_BYTES,
        "retained GPU payload and target allocation",
    )?;
    Ok((width, height))
}
