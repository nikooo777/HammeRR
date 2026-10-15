use std::io;

use crate::mdl::ModelHeader;

mod reader;
#[cfg(test)]
mod tests;
mod vtx;
mod vvd;

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
pub struct Model {
    pub header: ModelHeader,
    pub vertices: Vec<Vertex>,
    pub parts: Vec<Part>,
    pub materials: Vec<String>,
    pub material_directories: Vec<String>,
    pub skins: Vec<Vec<usize>>,
    pub bounds: [[f32; 3]; 2],
    pub body_parts: usize,
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
        if !header.is_static_prop() {
            return Err(invalid(
                "viewport geometry currently requires a static-prop model",
            ));
        }
        let reader = reader::Reader::new(mdl, 240)?;
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
        let vertices = vvd::parse(vvd, header.checksum)?;
        let bodies = reader.table(0, 232, 236, 16, MAX_BODY_PARTS)?;
        let mut meshes = Vec::with_capacity(bodies.len);
        let mut model_counts = Vec::with_capacity(bodies.len);
        let mut total_meshes = 0;
        for index in 0..bodies.len {
            let models = reader.table(bodies.at(index)?, 4, 12, 148, MAX_MODELS)?;
            model_counts.push(models.len);
            let model = models.at(0)?;
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
            meshes.push(model_meshes);
        }
        let parts = vtx::parse(vtx, header.checksum, &meshes, &model_counts)?;
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
            return Err(invalid("MDL default body has no displayable triangles"));
        }
        Ok(Self {
            header,
            vertices,
            parts,
            materials,
            material_directories,
            skins,
            bounds: [mins, maxs],
            body_parts: bodies.len,
        })
    }

    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.vertices.capacity() * std::mem::size_of::<Vertex>()
            + self.parts.capacity() * std::mem::size_of::<Part>()
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
