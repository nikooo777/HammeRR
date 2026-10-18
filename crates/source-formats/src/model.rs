use std::io;

use crate::mdl::ModelHeader;

mod attachments;
mod bones;
mod reader;
#[cfg(test)]
mod tests;
mod vtx;
mod vvd;

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
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Model {
    pub fn parse(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> io::Result<Self> {
        let header = ModelHeader::parse(mdl)?;
        let reader = reader::Reader::new(mdl, 248)?;
        let attachment_count = reader.count(240, attachments::MAX_ATTACHMENTS)?;
        let reference = (!header.is_static_prop() || attachment_count > 0)
            .then(|| bones::reference(&reader))
            .transpose()?;
        let attachments = attachments::parse(
            &reader,
            reference.as_ref().map_or(&[], |pose| pose.poses.as_slice()),
        )?;
        let textures = reader.table(0, 204, 208, 64, MAX_MATERIALS)?;
        let mut materials = Vec::with_capacity(textures.len);
        for index in 0..textures.len {
            let base = textures.at(index)?;
            let path = reader.string(reader.relative(base, 0)?)?;
            let path = crate::gamefs::asset_relative(&path)?;
            if path.is_empty() {
                return Err(invalid("MDL texture name cannot be empty"));
            }
            materials.push(path);
        }
        let directories = reader.table(0, 212, 216, 4, MAX_MATERIALS)?;
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
        let mut skins = Vec::with_capacity(header.skins);
        for family in 0..header.skins {
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
        let vertices = vvd::parse(vvd, header.checksum, skinning)?;
        let bodies = reader.table(0, 232, 236, 16, MAX_BODY_PARTS)?;
        let mut meshes = Vec::with_capacity(bodies.len);
        let mut body_groups = Vec::with_capacity(bodies.len);
        let mut total_meshes = 0;
        let mut total_models = 0;
        for index in 0..bodies.len {
            let base = bodies.at(index)?;
            let models = reader.table(base, 4, 12, 148, MAX_MODELS)?;
            total_models += models.len;
            if models.len == 0 || total_models > MAX_MODELS {
                return Err(invalid("MDL body variants exceed their resource limit"));
            }
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
                let parts = reader.table(model, 72, 76, 116, MAX_MESHES)?;
                total_meshes += parts.len;
                if total_meshes > MAX_MESHES {
                    return Err(invalid("MDL mesh count exceeds its resource limit"));
                }
                let mut model_meshes = Vec::with_capacity(parts.len);
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
                    model_meshes.push(Mesh {
                        material,
                        first: first + offset,
                        count: size,
                    });
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
        let geometry = vtx::parse(vtx, header.checksum, &meshes)?;
        let parts = geometry.parts;
        for (group, ranges) in body_groups.iter_mut().zip(geometry.models) {
            for (variant, range) in group.variants.iter_mut().zip(ranges) {
                variant.parts = range.clone();
                variant.bounds = part_bounds(&vertices, &parts[range]);
            }
        }
        let mut mins = [f32::INFINITY; 3];
        let mut maxs = [f32::NEG_INFINITY; 3];
        let mut triangles = 0;
        for part in &parts {
            triangles += part.indices.len() / 3;
            for &index in &part.indices {
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
        Ok(result)
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

fn part_bounds(vertices: &[Vertex], parts: &[Part]) -> Option<[[f32; 3]; 2]> {
    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    let mut present = false;
    for part in parts {
        for &index in &part.indices {
            present = true;
            let point = vertices[index as usize].position;
            for axis in 0..3 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
    }
    present.then_some([low, high])
}
