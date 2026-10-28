use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{DVec3, Mat4};
use serde::Serialize;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::{ImageFormat, Vtf};

use crate::materials::MaterialInfo;
use crate::scene::{Batch, Vertex};

pub const MAX_VERTICES: usize = 65_536;
pub const MAX_INDICES: usize = 196_608;
pub const MAX_SURFACES: usize = 16_384;
pub const MAX_MATERIALS: usize = 64;
pub const MAX_SCALE: f64 = 128.0;
pub const MAX_CPU_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_GPU_ASSET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_GPU_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TARGET_BYTES: usize = 32 * 1024 * 1024;
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
const TARGET_PIXEL_BYTES: usize = 8;
static NEXT_SCENE: AtomicU64 = AtomicU64::new(1);

pub const POLICY: &str = "Editor 3D sky preview: explicitly selected sealed flat-world/func_detail miniature; perspective Textured views only. Camera origin + main eye / integer scale 1..128; unchanged direction/FOV, no entity-angle rotation. Editor near plane 4/scale miniature units preserves main-equivalent clipping; native Source uses a different near policy. Static opaque single-base LightmappedGeneric/UnlitGeneric LDR materials with checked axis-aligned static base-texture transforms; finite center/scale/translation values within ±1024, last duplicate operation wins, nonzero rotation rejected; no alpha test, translucency, proxies, normal/detail/blend layers, fog, authored lighting or runtime effects. Missing/unsupported requested assets fail eligibility. SKY receives the miniature; SKY2D retains only the existing 2D sky path. Empty miniature pixels retain the chosen 2D sky when its independent preview is enabled and ready, otherwise the regular authored sky tool surface. Mixed SKY/SKY2D is an editor policy, not native mixed-sky parity. No helpers, selection, guides, projected picking or editing. Limits: 65536 vertices,196608 indices,16384 surfaces,64 materials,64 MiB conservative CPU builder allocation,32 MiB retained GPU geometry/material payload shared across scenes,64 MiB separately charged transient uploads and one 32 MiB color/depth target. The 128 MiB explicit payload/upload/target budget excludes driver allocation overhead and is not a total VRAM ceiling. Retired identities flush pending uploads and wait at most one second for GPU completion before replacement admission. Uploads conservatively charge RGBA rows padded to256 bytes for every mip plus geometry/uniform bytes; live matching identities reuse bindings. Targets/textures are at most2048 pixels per axis. Targets downsample larger viewports preserving aspect; texture assets never downsample or substitute.";

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
    pub normal: DVec3,
    pub material: String,
    pub texture_u: [f64; 4],
    pub texture_v: [f64; 4],
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
}

#[derive(Debug)]
pub struct Scene {
    pub camera: Camera,
    pub stats: Stats,
    pub(crate) id: u64,
    pub(crate) gpu_bytes: usize,
    pub(crate) upload_bytes: usize,
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
    pub(crate) batches: Vec<Batch>,
    pub(crate) materials: Vec<MaterialBinding>,
}

#[derive(Debug)]
pub(crate) struct MaterialBinding {
    pub info: MaterialInfo,
    pub unlit: bool,
    pub uv_transform: [f32; 4],
}

#[derive(Default)]
struct Budget {
    cpu: usize,
    gpu: usize,
    reads: usize,
    upload: usize,
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

    fn read(&mut self, fs: &GameFs, path: &str, vmt: bool) -> Result<Option<Vec<u8>>, String> {
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
        let bytes = fs
            .read_limited(
                &asset_relative(path).map_err(|error| error.to_string())?,
                maximum,
            )
            .map_err(|error| format!("3D sky asset {path}: {error}"))?;
        if let Some(bytes) = &bytes {
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
    for surface in surfaces {
        let count = surface.points.len();
        if count < 3 {
            return Err("3D sky surface polygon requires at least three vertices".into());
        }
        vertices = checked_sum(vertices, count, MAX_VERTICES, "vertices")?;
        indices = checked_sum(indices, (count - 2) * 3, MAX_INDICES, "indices")?;
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
    budget.cpu(PARSER_RESERVE + geometry * 2 + surfaces.len() * (size_of::<Batch>() + 128))?;
    budget.gpu(geometry + GPU_BINDING_RESERVE)?;
    budget.upload(geometry + GPU_BINDING_RESERVE)?;
    Ok((vertices, indices, budget))
}

fn load_material(
    fs: &GameFs,
    name: &str,
    budget: &mut Budget,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<Option<MaterialBinding>, String> {
    let material = Material::load_with_reader(&material_path(name), &mut |path| {
        checkpoint().map_err(std::io::Error::other)?;
        budget.read(fs, path, true).map_err(std::io::Error::other)
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
    if semantics.surface & source_formats::bsp::surf::SKY != 0 {
        return Ok(None);
    }
    if !semantics.draws || material.is_tool() {
        return Err(format!(
            "3D sky material {name} is an unsupported tool surface"
        ));
    }
    if !matches!(
        material.shader.as_str(),
        "lightmappedgeneric" | "unlitgeneric"
    ) || !material.blocks.is_empty()
        || material.translucent()
        || material.alpha_test()
    {
        return Err(format!(
            "3D sky material {name} requires static opaque LightmappedGeneric or UnlitGeneric"
        ));
    }
    for (key, _) in &material.params {
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
        ) {
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
        .read(fs, &path, false)?
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
        uv_transform,
    }))
}

fn no_checkpoint() -> Result<(), String> {
    Ok(())
}

impl Scene {
    pub fn build(camera: Camera, surfaces: &[Surface], fs: &GameFs) -> Result<Self, String> {
        Self::build_controlled(camera, surfaces, fs, no_checkpoint)
    }

    pub fn build_controlled(
        camera: Camera,
        surfaces: &[Surface],
        fs: &GameFs,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        checkpoint()?;
        camera.validate()?;
        let (vertex_count, index_count, mut budget) = geometry_budget(surfaces)?;
        let mut names = BTreeMap::new();
        let mut materials = Vec::new();
        for surface in surfaces {
            checkpoint()?;
            let name = surface.material.trim().to_ascii_lowercase();
            if names.contains_key(&name) {
                continue;
            }
            if names.len() >= MAX_MATERIALS {
                return Err(format!("3D sky materials exceed budget {MAX_MATERIALS}"));
            }
            let material = load_material(fs, &name, &mut budget, &mut checkpoint)?;
            let index = material.map(|material| {
                let index = materials.len();
                materials.push(material);
                index
            });
            names.insert(name, index);
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        let mut indices = Vec::with_capacity(index_count);
        let mut batches = Vec::with_capacity(surfaces.len());
        for surface in surfaces {
            checkpoint()?;
            let Some(material) = names[&surface.material.trim().to_ascii_lowercase()] else {
                continue;
            };
            let info = &materials[material].info;
            let transform = materials[material].uv_transform;
            let normal = surface.normal.normalize().as_vec3().to_array();
            let first = vertices.len() as u32;
            for point in &surface.points {
                let project = |axis: [f64; 4], size: u32| {
                    ((point.dot(DVec3::new(axis[0], axis[1], axis[2])) + axis[3]) / f64::from(size))
                        as f32
                };
                let uv = [
                    project(surface.texture_u, info.width),
                    project(surface.texture_v, info.height),
                ];
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
            batches.push(Batch {
                material,
                first_index,
                index_count: indices.len() as u32 - first_index,
                rope: false,
                model_translucent: false,
            });
        }
        checkpoint()?;
        Ok(Self {
            camera,
            stats: Stats {
                vertices: vertices.len(),
                indices: indices.len(),
                surfaces: batches.len(),
                materials: materials.len(),
                cpu_bytes: budget.cpu,
                gpu_bytes: budget.gpu,
                upload_bytes: budget.upload,
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
    if size.0 == 0 || size.1 == 0 || gpu_asset_bytes > MAX_GPU_ASSET_BYTES {
        return Err("3D sky target requires nonzero dimensions and admitted GPU assets".into());
    }
    let ratio = (f64::from(MAX_TARGET_DIMENSION) / f64::from(size.0))
        .min(f64::from(MAX_TARGET_DIMENSION) / f64::from(size.1))
        .min(1.0);
    let width = (f64::from(size.0) * ratio).floor().max(1.0) as u32;
    let height = (f64::from(size.1) * ratio).floor().max(1.0) as u32;
    let bytes = width as usize * height as usize * TARGET_PIXEL_BYTES;
    checked_sum(0, bytes, MAX_TARGET_BYTES, "color/depth target allocation")?;
    checked_sum(
        gpu_asset_bytes,
        bytes,
        MAX_GPU_BYTES - MAX_GPU_UPLOAD_BYTES,
        "retained GPU payload and target allocation",
    )?;
    Ok((width, height))
}

#[cfg(test)]
mod tests;
