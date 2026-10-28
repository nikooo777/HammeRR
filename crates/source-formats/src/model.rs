use std::io;

use crate::mdl::ModelHeader;

mod attachments;
mod bones;
mod hardware;

mod portable;
mod reader;

mod vtx;
mod vvd;

pub use hardware::{
    HardwareCharge, HardwareLayout, HardwareLimits, HardwareSection, MAX_HARDWARE_BYTES,
    MAX_HARDWARE_SECTIONS, MAX_HARDWARE_VERTICES, MAX_HARDWARE_WORK,
};
pub use portable::{validate_portable_physics, validate_portable_static};

pub fn material_candidates<'a>(
    name: &'a str,
    directories: &'a [String],
) -> impl Iterator<Item = String> + 'a {
    directories
        .iter()
        .map(move |directory| {
            if directory.is_empty() {
                name.to_owned()
            } else {
                format!("{directory}/{name}")
            }
        })
        .chain(std::iter::once(name.to_owned()))
}

pub const MAX_VERTICES: usize = 1_000_000;
pub const MAX_INDICES: usize = 6_000_000;
const MAX_MATERIALS: usize = 4096;
const MAX_SKIN_ENTRIES: usize = 1_000_000;
const MAX_BODY_PARTS: usize = 128;
const MAX_MODELS: usize = 512;
const MAX_MESHES: usize = 4096;
const MAX_LODS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug)]
pub struct Part {
    pub material: usize,
    pub indices: Vec<u32>,
}

#[derive(Debug)]
pub struct BodyVariant {
    pub name: String,
    pub parts: std::ops::Range<usize>,
    pub bounds: Option<[[f32; 3]; 2]>,
}

#[derive(Debug)]
pub struct BodyGroup {
    pub name: String,
    pub base: u32,
    pub variants: Vec<BodyVariant>,
}

#[derive(Debug)]
pub struct Attachment {
    pub name: String,
    pub bone: usize,
    pub flags: u32,
    pub transform: glam::DAffine3,
}

impl Attachment {
    pub fn world_aligned(&self) -> bool {
        self.flags & attachments::WORLD_ALIGN != 0
    }
}

impl BodyGroup {
    pub fn selected(&self, body: u32) -> usize {
        if self.variants.len() <= 1 {
            0
        } else {
            (body / self.base) as usize % self.variants.len()
        }
    }
}

#[derive(Debug)]
pub struct Model {
    pub header: ModelHeader,
    pub vertices: Vec<Vertex>,
    pub parts: Vec<Part>,
    pub materials: Vec<String>,
    pub material_directories: Vec<String>,
    pub skins: Vec<Vec<usize>>,
    pub bounds: [[f32; 3]; 2],
    pub body_parts: usize,
    pub reference_bones: usize,
    pub body_groups: Vec<BodyGroup>,
    pub attachments: Vec<Attachment>,
}

struct Mesh {
    material: usize,
    first: usize,
    count: usize,
    lod_counts: [usize; MAX_LODS],
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Model {
    pub fn parse(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> io::Result<Self> {
        Self::parse_inner(mdl, vvd, vtx, &mut hardware::Budget::ordinary()).map(|(model, _)| model)
    }

    /// Charges cumulative parsing allocations and work before growth, with zero-charge
    /// cancellation checkpoints. The returned mappings always address fixed-up root LOD 0.
    pub fn parse_with_hardware(
        mdl: &[u8],
        vvd: &[u8],
        vtx: &[u8],
        limits: HardwareLimits,
        charge: &mut impl FnMut(HardwareCharge) -> io::Result<()>,
    ) -> io::Result<(Self, HardwareLayout)> {
        let mut budget = hardware::Budget::hardware(limits, charge)?;
        let (model, layout) = Self::parse_inner(mdl, vvd, vtx, &mut budget)?;
        Ok((model, layout.unwrap()))
    }

    fn parse_inner(
        mdl: &[u8],
        vvd: &[u8],
        vtx: &[u8],
        budget: &mut hardware::Budget<'_>,
    ) -> io::Result<(Self, Option<HardwareLayout>)> {
        let header = ModelHeader::parse(mdl)?;
        if budget.enabled() && !header.is_static_prop() {
            return Err(invalid("model hardware layout requires a static-prop MDL"));
        }
        let reader = reader::Reader::new(mdl, 248)?;
        reader::Reader::new(vvd, 64)?;
        reader::Reader::new(vtx, 36)?;
        let attachment_count = reader.count(240, attachments::MAX_ATTACHMENTS)?;
        if attachment_count > 0 && budget.enabled() {
            let bone_count = reader.count(156, bones::MAX_BONES)?;
            budget.array::<glam::DAffine3>(bone_count * 5)?;
            budget.array::<usize>(bone_count * 2)?;
            budget.array::<Attachment>(attachment_count)?;
            budget.array::<u8>(attachment_count * reader::MAX_STRING_BYTES)?;
        }
        let reference = (!header.is_static_prop() || attachment_count > 0)
            .then(|| bones::reference(&reader))
            .transpose()?;
        let attachments = attachments::parse(
            &reader,
            reference.as_ref().map_or(&[], |pose| pose.poses.as_slice()),
        )?;
        let textures = reader.table(0, 204, 208, 64, MAX_MATERIALS)?;
        budget.array::<String>(textures.len)?;
        budget.array::<u8>(textures.len * reader::MAX_STRING_BYTES * 3)?;
        let mut materials = Vec::with_capacity(textures.len);
        for index in 0..textures.len {
            budget.checkpoint(index)?;
            let base = textures.at(index)?;
            let path = reader.string(reader.relative(base, 0)?)?;
            let path = crate::gamefs::asset_relative(&path)?;
            if path.is_empty() {
                return Err(invalid("MDL texture name cannot be empty"));
            }
            materials.push(path);
        }
        let directories = reader.table(0, 212, 216, 4, MAX_MATERIALS)?;
        budget.array::<String>(directories.len)?;
        budget.array::<u8>(directories.len * reader::MAX_STRING_BYTES * 3)?;
        let mut material_directories = Vec::with_capacity(directories.len);
        for index in 0..directories.len {
            material_directories.push(crate::gamefs::asset_relative(
                &reader.string(reader.count(directories.at(index)?, mdl.len())?)?,
            )?);
        }
        let skin_refs = reader.count(220, MAX_MATERIALS)?;
        let skin_entries = skin_refs
            .checked_mul(header.skins)
            .filter(|&size| size <= MAX_SKIN_ENTRIES)
            .ok_or_else(|| invalid("MDL skin table exceeds its resource limit"))?;
        let skin_start = reader.count(228, mdl.len())?;
        reader.bytes(skin_start, skin_entries * 2)?;
        budget.array::<Vec<usize>>(header.skins)?;
        budget.array::<usize>(skin_entries)?;
        let mut skins = Vec::with_capacity(header.skins);
        for family in 0..header.skins {
            budget.checkpoint(family)?;
            let mut skin = Vec::with_capacity(skin_refs);
            for index in 0..skin_refs {
                let material =
                    usize::from(reader.short(skin_start + (family * skin_refs + index) * 2)?);
                if material >= materials.len() {
                    return Err(invalid("MDL skin references an unavailable material"));
                }
                skin.push(material);
            }
            skins.push(skin);
        }
        let skinning =
            (!header.is_static_prop()).then(|| reference.as_ref().unwrap().skinning.as_slice());
        let vertex_data = vvd::parse(vvd, header.checksum, skinning, budget)?;
        let vertices = vertex_data.vertices;
        let bodies = reader.table(0, 232, 236, 16, MAX_BODY_PARTS)?;
        budget.array::<Vec<Vec<Mesh>>>(bodies.len)?;
        budget.array::<BodyGroup>(bodies.len)?;
        budget.array::<u8>(bodies.len * reader::MAX_STRING_BYTES)?;
        let mut meshes = Vec::with_capacity(bodies.len);
        let mut body_groups = Vec::with_capacity(bodies.len);
        let mut total_meshes = 0;
        let mut total_models = 0;
        let mut model_vertices = [0usize; MAX_LODS];
        for index in 0..bodies.len {
            budget.checkpoint(index)?;
            let base = bodies.at(index)?;
            let models = reader.table(base, 4, 12, 148, MAX_MODELS)?;
            total_models += models.len;
            if models.len == 0 || total_models > MAX_MODELS {
                return Err(invalid("MDL body variants exceed their resource limit"));
            }
            budget.array::<BodyVariant>(models.len)?;
            budget.array::<Vec<Mesh>>(models.len)?;
            budget.array::<u8>(models.len * 64)?;
            let value_base = reader.count(base + 8, i32::MAX as usize)? as u32;
            if models.len > 1 && value_base == 0 {
                return Err(invalid("MDL bodygroup base must be positive"));
            }
            let mut variants = Vec::with_capacity(models.len);
            let mut body_meshes = Vec::with_capacity(models.len);
            for variant in 0..models.len {
                let model = models.at(variant)?;
                let bytes = reader.bytes(model, 64)?;
                let name = std::str::from_utf8(
                    &bytes[..bytes
                        .iter()
                        .position(|&value| value == 0)
                        .unwrap_or(bytes.len())],
                )
                .map_err(|_| invalid("MDL body variant names must be UTF-8"))?
                .to_owned();
                let count = reader.count(model + 80, MAX_VERTICES)?;
                let byte_offset = reader.count(model + 84, MAX_VERTICES * 48)?;
                if !byte_offset.is_multiple_of(48) {
                    return Err(invalid("MDL model vertex offset is not aligned"));
                }
                let first = byte_offset / 48;
                if first + count > vertices.len() {
                    return Err(invalid("MDL model vertices exceed fixed-up VVD data"));
                }
                if budget.enabled() && first != model_vertices[0] {
                    return Err(invalid(
                        "MDL model vertex offset differs from root-LOD-0 mesh order",
                    ));
                }
                if budget.enabled()
                    && (reader.int(model + 64)? != 0 || reader.int(model + 100)? != 0)
                {
                    return Err(invalid(
                        "model hardware layout does not support specialized model types or eyes",
                    ));
                }
                let parts = reader.table(model, 72, 76, 116, MAX_MESHES)?;
                total_meshes += parts.len;
                if total_meshes > MAX_MESHES {
                    return Err(invalid("MDL mesh count exceeds its resource limit"));
                }
                budget.array::<Mesh>(parts.len)?;
                let mut model_meshes = Vec::with_capacity(parts.len);
                let mut mesh_vertices = [0usize; MAX_LODS];
                for part in 0..parts.len {
                    let mesh = parts.at(part)?;
                    let material = reader.count(mesh, MAX_MATERIALS)?;
                    if material >= skin_refs {
                        return Err(invalid("MDL mesh material is outside the skin table"));
                    }
                    let offset = reader.count(mesh + 12, count)?;
                    let size = reader.count(mesh + 8, count)?;
                    if offset + size > count {
                        return Err(invalid("MDL mesh vertex range exceeds its model"));
                    }
                    let mut lod_counts = [0usize; MAX_LODS];
                    if budget.enabled() {
                        if reader.count(mesh + 32, MAX_MESHES)? != total_meshes - parts.len + part {
                            return Err(invalid(
                                "MDL mesh IDs differ from native hardware mesh order",
                            ));
                        }
                        if reader.int(mesh + 16)? != 0 || reader.int(mesh + 24)? != 0 {
                            return Err(invalid(
                                "model hardware layout does not support flexes or specialized materials",
                            ));
                        }
                        if offset != mesh_vertices[0] {
                            return Err(invalid(
                                "MDL mesh vertex offset differs from root-LOD-0 mesh order",
                            ));
                        }
                        let mut previous = size;
                        for (lod, count) in lod_counts[..vertex_data.lods].iter_mut().enumerate() {
                            let value = reader.count(mesh + 52 + lod * 4, size)?;
                            if value > previous || (lod == 0 && value != size) {
                                return Err(invalid(
                                    "MDL mesh LOD vertex counts do not match its root vertices",
                                ));
                            }
                            *count = value;
                            mesh_vertices[lod] = mesh_vertices[lod]
                                .checked_add(value)
                                .ok_or_else(|| invalid("MDL mesh LOD vertex count overflow"))?;
                            previous = value;
                        }
                    }
                    model_meshes.push(Mesh {
                        material,
                        first: first + offset,
                        count: size,
                        lod_counts,
                    });
                }
                if budget.enabled() {
                    if mesh_vertices[0] != count {
                        return Err(invalid("MDL root mesh vertices do not cover their model"));
                    }
                    for (total, value) in model_vertices.iter_mut().zip(mesh_vertices) {
                        *total = total
                            .checked_add(value)
                            .ok_or_else(|| invalid("MDL model LOD vertex count overflow"))?;
                    }
                }
                body_meshes.push(model_meshes);
                variants.push(BodyVariant {
                    name,
                    parts: 0..0,
                    bounds: None,
                });
            }
            meshes.push(body_meshes);
            body_groups.push(BodyGroup {
                name: reader.string(reader.relative(base, 0)?)?,
                base: value_base,
                variants,
            });
        }
        if budget.enabled() && model_vertices != vertex_data.counts {
            return Err(invalid("MDL mesh LOD vertex totals do not match VVD"));
        }
        let root_lod = if budget.enabled() {
            usize::from(reader.byte(377)?)
        } else {
            0
        };
        let geometry = vtx::parse(
            vtx,
            header.checksum,
            vertex_data.lods,
            root_lod,
            &meshes,
            budget,
        )?;
        let parts = geometry.parts;
        let index_count = parts.iter().map(|part| part.indices.len()).sum::<usize>();
        budget.charge(0, index_count * 2)?;
        for (group, ranges) in body_groups.iter_mut().zip(geometry.models) {
            for (variant, range) in group.variants.iter_mut().zip(ranges) {
                variant.parts = range.clone();
                variant.bounds = part_bounds(&vertices, &parts[range], budget)?;
            }
        }
        let mut mins = [f32::INFINITY; 3];
        let mut maxs = [f32::NEG_INFINITY; 3];
        let mut triangles = 0;
        for part in &parts {
            triangles += part.indices.len() / 3;
            for (work, &index) in part.indices.iter().enumerate() {
                budget.checkpoint(work)?;
                let point = vertices[index as usize].position;
                for axis in 0..3 {
                    mins[axis] = mins[axis].min(point[axis]);
                    maxs[axis] = maxs[axis].max(point[axis]);
                }
            }
        }
        if triangles == 0 {
            return Err(invalid("MDL body variants have no displayable triangles"));
        }
        let mut result = Self {
            header,
            vertices,
            parts,
            materials,
            material_directories,
            skins,
            bounds: [mins, maxs],
            body_parts: bodies.len,
            reference_bones: skinning.map_or(0, <[_]>::len),
            body_groups,
            attachments,
        };
        result.bounds = result.bounds_for_body(0).unwrap_or(result.bounds);
        Ok((result, geometry.hardware))
    }

    pub fn parts_for_body(&self, body: u32) -> impl Iterator<Item = &Part> {
        self.indexed_parts_for_body(body).map(|(_, part)| part)
    }

    pub fn indexed_parts_for_body(&self, body: u32) -> impl Iterator<Item = (usize, &Part)> {
        self.parts
            .iter()
            .enumerate()
            .take(if self.body_groups.is_empty() {
                self.parts.len()
            } else {
                0
            })
            .chain(self.body_groups.iter().flat_map(move |group| {
                let range = group.variants[group.selected(body)].parts.clone();
                self.parts[range.clone()]
                    .iter()
                    .enumerate()
                    .map(move |(offset, part)| (range.start + offset, part))
            }))
            .filter(|(_, part)| !part.indices.is_empty())
    }

    pub fn bounds_for_body(&self, body: u32) -> Option<[[f32; 3]; 2]> {
        if self.body_groups.is_empty() {
            return Some(self.bounds);
        }
        self.body_groups
            .iter()
            .filter_map(|group| group.variants[group.selected(body)].bounds)
            .reduce(|a, b| {
                [
                    std::array::from_fn(|axis| a[0][axis].min(b[0][axis])),
                    std::array::from_fn(|axis| a[1][axis].max(b[1][axis])),
                ]
            })
    }

    pub fn set_bodygroup(&self, body: u32, group: usize, variant: usize) -> io::Result<u32> {
        let group = self
            .body_groups
            .get(group)
            .ok_or_else(|| invalid("Bodygroup index is out of range"))?;
        if variant >= group.variants.len() {
            return Err(invalid("Bodygroup variant is out of range"));
        }
        let value = i64::from(body) - (group.selected(body) as i64) * i64::from(group.base)
            + (variant as i64) * i64::from(group.base);
        u32::try_from(value)
            .ok()
            .filter(|&value| value <= i32::MAX as u32)
            .ok_or_else(|| invalid("Bodygroup value must fit a nonnegative native integer"))
    }

    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.vertices.capacity() * std::mem::size_of::<Vertex>()
            + self.parts.capacity() * std::mem::size_of::<Part>()
            + self.attachments.capacity() * std::mem::size_of::<Attachment>()
            + self
                .attachments
                .iter()
                .map(|attachment| attachment.name.capacity())
                .sum::<usize>()
            + self.body_groups.capacity() * std::mem::size_of::<BodyGroup>()
            + self
                .body_groups
                .iter()
                .map(|group| {
                    group.name.capacity()
                        + group.variants.capacity() * std::mem::size_of::<BodyVariant>()
                        + group
                            .variants
                            .iter()
                            .map(|variant| variant.name.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self
                .parts
                .iter()
                .map(|part| part.indices.capacity() * std::mem::size_of::<u32>())
                .sum::<usize>()
            + (self.materials.capacity() + self.material_directories.capacity())
                * std::mem::size_of::<String>()
            + self
                .materials
                .iter()
                .chain(&self.material_directories)
                .map(String::capacity)
                .sum::<usize>()
            + self.skins.capacity() * std::mem::size_of::<Vec<usize>>()
            + self
                .skins
                .iter()
                .map(|skin| skin.capacity() * std::mem::size_of::<usize>())
                .sum::<usize>()
    }
}

fn part_bounds(
    vertices: &[Vertex],
    parts: &[Part],
    budget: &mut hardware::Budget<'_>,
) -> io::Result<Option<[[f32; 3]; 2]>> {
    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    let mut present = false;
    for part in parts {
        for (work, &index) in part.indices.iter().enumerate() {
            budget.checkpoint(work)?;
            present = true;
            let point = vertices[index as usize].position;
            for axis in 0..3 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
    }
    Ok(present.then_some([low, high]))
}
