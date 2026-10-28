use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;

use glam::{DMat4, DVec3, Vec3};
use source_formats::bsp::{
    Bsp, FaceGeometry, LightingKind, RgbExp32, TexData, TexInfo, lump, surf,
};

use crate::scene::{Batch, Vertex};
use crate::{LightingChannel, SceneMesh};

mod decode;
pub mod materials;
pub mod props;
mod settings;
#[cfg(test)]
mod tests;
pub use crate::compiled_details::{MAX_EXPOSURE_EV, MAX_STYLE_GAIN, MIN_EXPOSURE_EV};
pub use settings::{LightingSettings, StyleGain};

pub const MAX_CPU_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_GPU_BYTES: usize = 256 * 1024 * 1024;
pub const BAKED_FACE_RECEIVER: u32 = 4;
pub const BAKED_PROP_RECEIVER: u32 = 5;
const RECEIVER_SHIFT: u32 = 4;
const GPU_FACE_BYTES: usize = 32;
const GPU_PARAMS_BYTES: usize = 16;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MaterialReference {
    pub name: String,
    pub dimensions: [u32; 2],
    pub directional: bool,
    pub lightmapped: bool,
    pub used: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SurfaceInfo {
    pub source: LightingSource,
    pub ordinal: usize,
    pub model: usize,
    pub material: usize,
    pub terrain: bool,
    pub vertices: usize,
    pub triangles: usize,
    pub dimensions: [u32; 2],
    pub maps: usize,
    pub styles: Vec<u8>,
    pub missing_surface_samples: usize,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingSource {
    BakedLightmap,
    UnlitMaterial,
}

#[derive(Debug, Clone, Copy)]
pub struct FaceLighting {
    pub width: u32,
    pub height: u32,
    pub map_offsets: [u32; 4],
    pub map_count: u32,
}

#[derive(Debug)]
pub struct LightingData {
    pub samples: Vec<[f32; 4]>,
    pub faces: Vec<FaceLighting>,
    pub exposure_ev: f32,
    pub channel: LightingChannel,
    pub effective_style_gains: Vec<StyleGain>,
}

impl LightingData {
    pub fn byte_len(&self) -> Result<usize, String> {
        sum_bytes(&[
            bytes(self.samples.len(), size_of::<[f32; 4]>())?,
            bytes(self.faces.len(), GPU_FACE_BYTES)?,
            GPU_PARAMS_BYTES,
        ])
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.exposure_ev.is_finite()
            || !(MIN_EXPOSURE_EV as f32..=MAX_EXPOSURE_EV as f32).contains(&self.exposure_ev)
            || self.byte_len()? > MAX_GPU_BYTES
        {
            return Err(
                "Baked surface lighting exceeds its finite exposure or GPU working budget".into(),
            );
        }
        for sample in &self.samples {
            if sample[..3]
                .iter()
                .any(|&value| !value.is_finite() || value < 0.0)
                || sample[3] != 0.0
            {
                return Err(
                    "Baked surface radiance exceeds finite floating-point display arithmetic"
                        .into(),
                );
            }
        }
        for face in &self.faces {
            if face.map_count == 0 {
                if face.width != 0 || face.height != 0 || face.map_offsets != [0; 4] {
                    return Err(
                        "Unlit surface descriptors must have no baked lightmap storage".into(),
                    );
                }
                continue;
            }
            if face.width == 0 || face.height == 0 || ![1, 4].contains(&face.map_count) {
                return Err("Baked surface lightmap descriptor is invalid".into());
            }
            let count = bytes(face.width as usize, face.height as usize)?;
            for &offset in &face.map_offsets[..face.map_count as usize] {
                let end = (offset as usize)
                    .checked_add(count)
                    .ok_or("Baked surface sample range overflow")?;
                if end > self.samples.len() {
                    return Err("Baked surface sample range is incomplete".into());
                }
            }
        }
        Ok(())
    }

    pub fn cpu_byte_len(&self) -> Result<usize, String> {
        sum_bytes(&[
            bytes(self.samples.len(), size_of::<[f32; 4]>())?,
            bytes(self.faces.len(), size_of::<FaceLighting>())?,
            bytes(self.effective_style_gains.len(), size_of::<StyleGain>())?,
            size_of::<Self>(),
        ])
    }
}

#[derive(Debug)]
pub struct Scene {
    materials: Vec<MaterialReference>,
    channels: [Option<Channel>; 2],
    retained_bytes: usize,
}

#[derive(Debug)]
struct Channel {
    surfaces: Vec<SurfaceInfo>,
    layouts: Vec<SampleLayout>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    wires: Vec<crate::scene::LineVertex>,
    parts: Vec<Part>,
    samples: Vec<RgbExp32>,
}

#[derive(Debug)]
struct SampleLayout {
    offset: usize,
    count: usize,
    maps: usize,
}

#[derive(Debug)]
struct Part {
    surface: usize,
    material: usize,
    first_index: u32,
    index_count: u32,
    wires: std::ops::Range<usize>,
}

impl Scene {
    pub fn decode(bsp: &Bsp, model_transforms: &[DMat4]) -> Result<Self, String> {
        Self::decode_controlled(bsp, model_transforms, &mut || Ok(()))
    }

    pub fn decode_controlled(
        bsp: &Bsp,
        model_transforms: &[DMat4],
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> Result<Self, String> {
        check().map_err(message)?;
        let (texinfo, texdata, names) = material_tables(bsp)?;
        let table_bytes = sum_bytes(&[
            bytes(texinfo.len(), size_of::<TexInfo>())?,
            bytes(texdata.len(), size_of::<TexData>())?,
            bytes(names.len(), size_of::<String>())?,
            names.iter().map(String::len).sum(),
        ])?;
        let mut materials = Vec::with_capacity(texdata.len());
        let mut material_working_bytes = sum_bytes(&[
            table_bytes,
            bytes(texdata.len(), size_of::<MaterialReference>())?,
        ])?;
        for data in &texdata {
            let name = usize::try_from(data.name_index)
                .ok()
                .and_then(|index| names.get(index))
                .ok_or("Baked surface TexData material name is missing")?;
            let width = u32::try_from(data.width)
                .ok()
                .filter(|&value| value != 0)
                .ok_or("Baked surface TexData width must be positive")?;
            let height = u32::try_from(data.height)
                .ok()
                .filter(|&value| value != 0)
                .ok_or("Baked surface TexData height must be positive")?;
            if name.len() > 256 || name.is_empty() {
                return Err("Baked surface material name must contain 1–256 bytes".into());
            }
            material_working_bytes = material_working_bytes
                .checked_add(name.len())
                .filter(|&value| value <= MAX_CPU_BYTES)
                .ok_or("Baked surface material references exceed the 256 MiB CPU working budget")?;
            materials.push(MaterialReference {
                name: name.clone(),
                dimensions: [width, height],
                directional: false,
                lightmapped: false,
                used: false,
            });
        }
        let mut retained_bytes = sum_bytes(&[
            bytes(materials.len(), size_of::<MaterialReference>())?,
            materials.iter().map(|material| material.name.len()).sum(),
        ])?;
        let mut channels = [None, None];
        for channel in [LightingChannel::Ldr, LightingChannel::Hdr] {
            check().map_err(message)?;
            let kind = kind(channel);
            let face_lump = match channel {
                LightingChannel::Ldr => lump::FACES,
                LightingChannel::Hdr => lump::FACES_HDR,
            };
            let faces_empty = bsp.lumps[face_lump].data.is_empty();
            let lighting = bsp.lighting(kind).map_err(message)?;
            if faces_empty && lighting.is_none() {
                continue;
            }
            if faces_empty {
                return Err(format!(
                    "Selected {} lighting has no FACE table",
                    channel.name()
                ));
            }
            let geometry_preflight = geometry_bytes(bsp, face_lump)?;
            if sum_bytes(&[retained_bytes, table_bytes, geometry_preflight])? > MAX_CPU_BYTES {
                return Err(
                    "Baked surface geometry decoding exceeds the 256 MiB CPU working budget".into(),
                );
            }
            let geometry = FaceGeometry::new_controlled(bsp, kind, check).map_err(message)?;
            validate_transforms(model_transforms, geometry.models().len())?;
            let decoded = Channel::decode(
                bsp,
                &geometry,
                &texinfo,
                &mut materials,
                model_transforms,
                lighting.map_or(&[], |lighting| lighting.samples()),
                sum_bytes(&[retained_bytes, table_bytes, geometry.retained_bytes()])?,
                check,
            )?;
            retained_bytes = retained_bytes
                .checked_add(decoded.byte_len()?)
                .filter(|&value| value <= MAX_CPU_BYTES)
                .ok_or("Baked surface retained data exceeds the 256 MiB CPU working budget")?;
            channels[channel_index(channel)] = Some(decoded);
        }
        if channels.iter().all(Option::is_none) {
            return Err("The retained BSP has no baked FACE lighting channel".into());
        }
        check().map_err(message)?;
        Ok(Self {
            materials,
            channels,
            retained_bytes,
        })
    }

    pub fn materials(&self) -> &[MaterialReference] {
        &self.materials
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn available_channels(&self) -> Vec<LightingChannel> {
        [LightingChannel::Ldr, LightingChannel::Hdr]
            .into_iter()
            .filter(|&channel| self.channels[channel_index(channel)].is_some())
            .collect()
    }

    pub fn surfaces(&self, channel: LightingChannel) -> Result<&[SurfaceInfo], String> {
        Ok(&self.channel(channel)?.surfaces)
    }

    pub fn available_styles(&self, channel: LightingChannel) -> Result<Vec<u8>, String> {
        Ok(self
            .channel(channel)?
            .surfaces
            .iter()
            .flat_map(|surface| surface.styles.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn lighting_bytes(&self, channel: LightingChannel) -> Result<usize, String> {
        self.channel(channel)?.lighting_bytes()
    }

    pub fn lighting_cpu_bytes(&self, channel: LightingChannel) -> Result<usize, String> {
        self.channel(channel)?.lighting_cpu_bytes()
    }

    pub fn validate_settings(&self, settings: &LightingSettings) -> Result<(), String> {
        settings.validate()?;
        let styles = self.available_styles(settings.channel)?;
        if settings
            .style_gains
            .iter()
            .any(|entry| !styles.contains(&entry.style))
        {
            return Err(
                "Baked surface gains must address style IDs stored in the selected channel".into(),
            );
        }
        Ok(())
    }

    pub fn lighting(&self, settings: &LightingSettings) -> Result<Arc<LightingData>, String> {
        self.validate_settings(settings)?;
        let channel = self.channel(settings.channel)?;
        let data = channel.compose(settings, self.retained_bytes)?;
        Ok(Arc::new(data))
    }

    pub fn append(
        &self,
        mesh: &mut SceneMesh,
        materials: &[Option<Arc<materials::Material>>],
        settings: &LightingSettings,
    ) -> Result<(), String> {
        let lighting = self.lighting(settings)?;
        self.append_prepared(mesh, materials, lighting)
    }

    pub fn append_prepared(
        &self,
        mesh: &mut SceneMesh,
        materials: &[Option<Arc<materials::Material>>],
        lighting: Arc<LightingData>,
    ) -> Result<(), String> {
        self.append_prepared_filtered(mesh, materials, lighting, |_| true)
    }

    pub fn append_prepared_filtered(
        &self,
        mesh: &mut SceneMesh,
        materials: &[Option<Arc<materials::Material>>],
        lighting: Arc<LightingData>,
        include: impl Fn(&SurfaceInfo) -> bool,
    ) -> Result<(), String> {
        if mesh.baked_faces.is_some() {
            return Err("The scene already contains a baked surface layer".into());
        }
        let channel = self.channel(lighting.channel)?;
        lighting.validate()?;
        if lighting.faces.len() != channel.surfaces.len()
            || lighting
                .faces
                .iter()
                .zip(&channel.surfaces)
                .any(|(face, surface)| {
                    [face.width, face.height] != surface.dimensions
                        || face.map_count as usize != surface.maps
                })
            || channel.parts.iter().any(|part| {
                materials
                    .get(part.material)
                    .and_then(Option::as_ref)
                    .is_none_or(|material| {
                        let surface = &channel.surfaces[part.surface];
                        material.lighting
                            != match surface.source {
                                LightingSource::UnlitMaterial => materials::MaterialLighting::Unlit,
                                LightingSource::BakedLightmap => {
                                    materials::MaterialLighting::Lightmap {
                                        directional: surface.maps == 4,
                                    }
                                }
                            }
                    })
            })
        {
            return Err(
                "Baked surface lighting or material ranges do not match final geometry".into(),
            );
        }
        let vertex_count = mesh
            .vertices
            .len()
            .checked_add(channel.vertices.len())
            .filter(|&count| count <= u32::MAX as usize)
            .ok_or("Baked surface vertex range overflow")?;
        let index_count = mesh
            .indices
            .len()
            .checked_add(channel.indices.len())
            .filter(|&count| count <= u32::MAX as usize)
            .ok_or("Baked surface index range overflow")?;
        let line_count = [
            mesh.lines.len(),
            mesh.reference_lines.len(),
            mesh.leak_lines.len(),
            mesh.wire_lines.len(),
            mesh.tool_wire_lines.len(),
            mesh.rope_lines.len(),
            channel.wires.len(),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .ok_or("Baked surface scene line count overflow")?;
        let mesh_bytes = sum_bytes(&[
            bytes(vertex_count, size_of::<Vertex>())?,
            bytes(index_count, size_of::<u32>())?,
            bytes(mesh.batches.len() + channel.parts.len(), size_of::<Batch>())?,
            bytes(line_count, size_of::<crate::scene::LineVertex>())?,
            bytes(mesh.icons.len(), size_of::<crate::icons::Instance>())?,
            bytes(
                mesh.detail_billboards.len(),
                size_of::<crate::compiled_details::Billboard>(),
            )?,
            bytes(
                mesh.detail_parts.len(),
                size_of::<crate::compiled_details::Part>(),
            )?,
            bytes(
                materials.len(),
                size_of::<Option<Arc<materials::Material>>>(),
            )?,
        ])?;
        let material_cpu_bytes = materials
            .iter()
            .flatten()
            .try_fold(0usize, |total, material| {
                total.checked_add(material.decoded_bytes())
            })
            .ok_or("Baked surface material byte count overflow")?;
        let material_gpu_bytes = materials
            .iter()
            .flatten()
            .try_fold(0usize, |total, material| {
                total.checked_add(material.gpu_bytes())
            })
            .ok_or("Baked surface material GPU byte count overflow")?;
        let cpu_bytes = sum_bytes(&[
            self.retained_bytes,
            lighting.cpu_byte_len()?,
            bytes(lighting.faces.len(), GPU_FACE_BYTES)?,
            mesh_bytes,
            material_cpu_bytes,
        ])?;
        let gpu_bytes = sum_bytes(&[lighting.byte_len()?, mesh_bytes, material_gpu_bytes])?;
        if cpu_bytes > MAX_CPU_BYTES || gpu_bytes > MAX_GPU_BYTES {
            return Err("Baked surface scene exceeds the 256 MiB CPU or GPU working budget".into());
        }
        let vertex_base = mesh.vertices.len() as u32;
        mesh.vertices
            .try_reserve_exact(channel.vertices.len())
            .map_err(|error| format!("Baked surface vertices unavailable: {error}"))?;
        mesh.indices
            .try_reserve_exact(channel.indices.len())
            .map_err(|error| format!("Baked surface indices unavailable: {error}"))?;
        mesh.batches
            .try_reserve_exact(channel.parts.len())
            .map_err(|error| format!("Baked surface batches unavailable: {error}"))?;
        mesh.wire_lines
            .try_reserve_exact(channel.wires.len())
            .map_err(|error| format!("Baked surface wire lines unavailable: {error}"))?;
        let mut attached_materials = Vec::new();
        attached_materials
            .try_reserve_exact(materials.len())
            .map_err(|error| format!("Baked surface materials unavailable: {error}"))?;
        attached_materials.extend_from_slice(materials);
        mesh.vertices.extend_from_slice(&channel.vertices);
        let first_batch = mesh.batches.len();
        let mut visible = 0;
        for part in &channel.parts {
            let surface = &channel.surfaces[part.surface];
            if !include(surface) {
                continue;
            }
            let start = part.first_index as usize;
            let end = start + part.index_count as usize;
            let first_index = mesh.indices.len() as u32;
            mesh.indices.extend(
                channel.indices[start..end]
                    .iter()
                    .map(|index| vertex_base + index),
            );
            mesh.wire_lines
                .extend_from_slice(&channel.wires[part.wires.clone()]);
            mesh.batches.push(Batch {
                material: part.material,
                first_index,
                index_count: part.index_count,
                rope: false,
                model_translucent: false,
            });
            mesh.mins = mesh.mins.min(Vec3::from_array(surface.mins));
            mesh.maxs = mesh.maxs.max(Vec3::from_array(surface.maxs));
            visible += 1;
        }
        mesh.baked_face_batches = first_batch..mesh.batches.len();
        mesh.baked_face_visible_surfaces = visible;
        mesh.baked_face_materials = attached_materials;
        mesh.baked_faces = Some(lighting);
        Ok(())
    }

    fn channel(&self, channel: LightingChannel) -> Result<&Channel, String> {
        self.channels[channel_index(channel)]
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "The retained BSP has no {} FACE lighting channel",
                    channel.name()
                )
            })
    }
}

fn kind(channel: LightingChannel) -> LightingKind {
    match channel {
        LightingChannel::Ldr => LightingKind::Ldr,
        LightingChannel::Hdr => LightingKind::Hdr,
    }
}

fn channel_index(channel: LightingChannel) -> usize {
    match channel {
        LightingChannel::Ldr => 0,
        LightingChannel::Hdr => 1,
    }
}

fn message(error: io::Error) -> String {
    error.to_string()
}

fn bytes(count: usize, size: usize) -> Result<usize, String> {
    count
        .checked_mul(size)
        .ok_or_else(|| "Baked surface byte count overflow".into())
}

fn sum_bytes(values: &[usize]) -> Result<usize, String> {
    values
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(*value))
        .ok_or_else(|| "Baked surface byte count overflow".into())
}

type MaterialTables = (Vec<TexInfo>, Vec<TexData>, Vec<String>);

fn material_tables(bsp: &Bsp) -> Result<MaterialTables, String> {
    let table = &bsp.lumps[lump::TEXDATA_STRING_TABLE];
    let data = &bsp.lumps[lump::TEXDATA_STRING_DATA];
    for index in [
        lump::TEXINFO,
        lump::TEXDATA,
        lump::TEXDATA_STRING_TABLE,
        lump::TEXDATA_STRING_DATA,
    ] {
        let record = &bsp.lumps[index];
        if record.version != 0 || record.fourcc != [0; 4] {
            return Err(
                "Baked surface material tables require uncompressed version-zero records".into(),
            );
        }
    }
    if !table.data.len().is_multiple_of(size_of::<i32>()) {
        return Err("Baked surface material string offsets are truncated".into());
    }
    let mut working_bytes = sum_bytes(&[
        bsp.lumps[lump::TEXINFO].data.len(),
        bsp.lumps[lump::TEXDATA].data.len(),
        table.data.len(),
        data.data.len(),
        bytes(table.data.len() / size_of::<i32>(), size_of::<String>())?,
        bytes(
            bsp.lumps[lump::TEXDATA].data.len() / size_of::<TexData>(),
            size_of::<MaterialReference>(),
        )?,
    ])?;
    if working_bytes > MAX_CPU_BYTES {
        return Err(
            "Baked surface material references exceed the 256 MiB CPU working budget".into(),
        );
    }
    for offset in table.data.as_chunks::<{ size_of::<i32>() }>().0 {
        let offset = i32::from_le_bytes(*offset);
        let rest = usize::try_from(offset)
            .ok()
            .and_then(|offset| data.data.get(offset..))
            .ok_or("Baked surface material string offset is outside its retained table")?;
        let length = rest
            .iter()
            .take(257)
            .position(|&value| value == 0)
            .ok_or("Baked surface material names must be terminated within 256 bytes")?;
        std::str::from_utf8(&rest[..length])
            .map_err(|_| "Baked surface material names must be UTF-8")?;
        working_bytes = working_bytes
            .checked_add(length * 2)
            .filter(|&value| value <= MAX_CPU_BYTES)
            .ok_or("Baked surface material strings exceed the 256 MiB CPU working budget")?;
    }
    Ok((
        bsp.records().map_err(message)?,
        bsp.records().map_err(message)?,
        bsp.texdata_strings().map_err(message)?,
    ))
}

fn geometry_bytes(bsp: &Bsp, faces: usize) -> Result<usize, String> {
    let tables = [
        faces,
        lump::MODELS,
        lump::PLANES,
        lump::SURFEDGES,
        lump::EDGES,
        lump::VERTEXES,
        lump::VERTNORMALS,
        lump::VERTNORMALINDICES,
        lump::DISPINFO,
        lump::DISP_VERTS,
        lump::DISP_TRIS,
    ];
    let raw_bytes = tables
        .iter()
        .try_fold(0usize, |total, &index| {
            total.checked_add(bsp.lumps[index].data.len())
        })
        .ok_or("Baked surface geometry byte count overflow")?;
    sum_bytes(&[
        raw_bytes,
        bytes(
            bsp.lumps[faces].data.len() / size_of::<source_formats::bsp::Face>(),
            size_of::<usize>(),
        )?,
    ])
}

fn validate_transforms(transforms: &[DMat4], models: usize) -> Result<(), String> {
    if transforms.len() != models || transforms.first() != Some(&DMat4::IDENTITY) {
        return Err(
            "Baked surface transforms must bind every emitted model with Model0 identity".into(),
        );
    }
    for transform in transforms {
        let axes = [transform.x_axis, transform.y_axis, transform.z_axis];
        if !transform.is_finite()
            || transform.w_axis.w != 1.0
            || axes
                .iter()
                .any(|axis| axis.w != 0.0 || (axis.truncate().length_squared() - 1.0).abs() > 1e-5)
            || axes[0].truncate().dot(axes[1].truncate()).abs() > 1e-5
            || axes[0].truncate().dot(axes[2].truncate()).abs() > 1e-5
            || axes[1].truncate().dot(axes[2].truncate()).abs() > 1e-5
            || (transform.determinant() - 1.0).abs() > 1e-5
        {
            return Err("Baked surface model transforms must be finite rigid initial poses".into());
        }
    }
    Ok(())
}
