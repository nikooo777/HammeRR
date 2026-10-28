//! Bounded VHV v2 color files with the SDK producer's contiguous, sector-aligned layout.
//! Colors are already encoded BGRA bytes; this codec does not convert lighting units.

use std::io;

use bytemuck::{Pod, Zeroable};

pub const VERSION: u32 = 2;
pub const VERTEX_COLOR: u32 = 0x0004;
pub const HEADER_BYTES: usize = 40;
pub const MESH_HEADER_BYTES: usize = 28;
pub const VERTEX_BYTES: usize = 4;
pub const ALIGNMENT: usize = 512;
pub const MAX_LODS: u32 = 8;

/// HammeRR resource budgets, not native-engine format limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_meshes: usize,
    pub max_vertices: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_meshes: 65_536,
            max_vertices: 4 * 1024 * 1024,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Bgra8 {
    pub blue: u8,
    pub green: u8,
    pub red: u8,
    pub alpha: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshLayout {
    pub lod: u32,
    pub vertex_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mesh {
    pub layout: MeshLayout,
    pub colors: Vec<Bgra8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub checksum: u32,
    pub meshes: Vec<Mesh>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub checksum: u32,
    pub mesh_count: usize,
    pub vertex_count: usize,
    /// File, mesh elements and color elements; excludes allocator bookkeeping.
    pub decoded_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    data: &'a [u8],
    metadata: Metadata,
}

#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    pub index: usize,
    pub layout: MeshLayout,
    pub hardware_start: usize,
    pub payload_offset: usize,
    colors: &'a [Bgra8],
}

impl<'a> View<'a> {
    pub fn parse(data: &'a [u8], limits: Limits) -> io::Result<Self> {
        Ok(Self {
            data,
            metadata: inspect(data, limits)?,
        })
    }

    pub fn metadata(self) -> Metadata {
        self.metadata
    }

    pub fn section(self, index: usize) -> io::Result<Section<'a>> {
        if index >= self.metadata.mesh_count {
            return Err(invalid("VHV section index exceeds its descriptor count"));
        }
        let mut hardware_start = 0_usize;
        for previous in 0..index {
            hardware_start = hardware_start
                .checked_add(
                    mesh_layout(self.data, HEADER_BYTES + previous * MESH_HEADER_BYTES)?
                        .vertex_count as usize,
                )
                .ok_or_else(|| invalid("VHV hardware prefix overflow"))?;
        }
        let base = HEADER_BYTES + index * MESH_HEADER_BYTES;
        let layout = mesh_layout(self.data, base)?;
        let payload_offset = word(self.data, base + 8)? as usize;
        let colors = bytemuck::cast_slice(bytes(
            self.data,
            payload_offset,
            vertex_bytes(layout.vertex_count as usize)?,
        )?);
        Ok(Section {
            index,
            layout,
            hardware_start,
            payload_offset,
            colors,
        })
    }
}

impl<'a> Section<'a> {
    pub fn page(self, offset: usize, count: usize) -> io::Result<&'a [Bgra8]> {
        let end = offset
            .checked_add(count)
            .ok_or_else(|| invalid("VHV vertex page range overflow"))?;
        self.colors
            .get(offset..end)
            .ok_or_else(|| invalid("VHV vertex page exceeds its section"))
    }
}

struct Encoding {
    payload_start: usize,
    encoded_bytes: usize,
    vertex_count: usize,
}

impl Limits {
    fn validate(self) -> io::Result<()> {
        if self.max_bytes > u32::MAX as usize
            || self.max_meshes > i32::MAX as usize
            || self.max_vertices > u32::MAX as usize
        {
            return Err(invalid("VHV limits exceed the format's field sizes"));
        }
        Ok(())
    }
}

/// Validates the whole supported layout without allocating decoded meshes or colors.
pub fn inspect(data: &[u8], limits: Limits) -> io::Result<Metadata> {
    limits.validate()?;
    if data.len() > limits.max_bytes {
        return Err(invalid("VHV file exceeds its byte budget"));
    }
    bytes(data, 0, HEADER_BYTES)?;
    if word(data, 0)? != VERSION {
        return Err(invalid("unsupported VHV version; expected version 2"));
    }
    if word(data, 8)? != VERTEX_COLOR || word(data, 12)? != VERTEX_BYTES as u32 {
        return Err(invalid("unsupported VHV vertex flags or stride"));
    }
    zeroes(
        bytes(data, 24, 16)?,
        "VHV header reserved words must be zero",
    )?;
    let vertex_count = count(word(data, 16)?, limits.max_vertices, "VHV vertex")?;
    let mesh_word = word(data, 20)?;
    if mesh_word > i32::MAX as u32 {
        return Err(invalid("VHV mesh count must be nonnegative"));
    }
    let mesh_count = count(mesh_word, limits.max_meshes, "VHV mesh")?;
    let directory_end = directory_end(mesh_count)?;
    let payload_start = aligned(directory_end)?;
    bytes(data, 0, payload_start)?;
    zeroes(
        bytes(data, directory_end, payload_start - directory_end)?,
        "VHV header padding must be zero",
    )?;

    let mut offset = payload_start;
    let mut total = 0_usize;
    for index in 0..mesh_count {
        let base = HEADER_BYTES + index * MESH_HEADER_BYTES;
        let layout = mesh_layout(data, base)?;
        let vertices = count(layout.vertex_count, limits.max_vertices, "VHV mesh vertex")?;
        total = total
            .checked_add(vertices)
            .filter(|&value| value <= limits.max_vertices)
            .ok_or_else(|| invalid("VHV aggregate vertex count exceeds its budget"))?;
        if usize::try_from(word(data, base + 8)?).ok() != Some(offset) {
            return Err(invalid("VHV mesh payloads must be contiguous and ordered"));
        }
        zeroes(
            bytes(data, base + 12, 16)?,
            "VHV mesh reserved words must be zero",
        )?;
        let size = vertex_bytes(vertices)?;
        bytes(data, offset, size)?;
        offset = offset
            .checked_add(size)
            .ok_or_else(|| invalid("VHV payload offset overflow"))?;
    }
    if total != vertex_count {
        return Err(invalid(
            "VHV total vertex count differs from its mesh records",
        ));
    }
    if data.len() != aligned(offset)? {
        return Err(invalid("VHV file must end at the next 512-byte boundary"));
    }
    zeroes(
        bytes(data, offset, data.len() - offset)?,
        "VHV trailing padding must be zero",
    )?;
    let color_bytes = vertex_bytes(vertex_count)?;
    let decoded_bytes = mesh_count
        .checked_mul(size_of::<Mesh>())
        .and_then(|value| value.checked_add(color_bytes))
        .and_then(|value| value.checked_add(size_of::<File>()))
        .ok_or_else(|| invalid("VHV decoded allocation size overflow"))?;
    Ok(Metadata {
        checksum: word(data, 4)?,
        mesh_count,
        vertex_count,
        decoded_bytes,
    })
}

impl File {
    pub fn parse(data: &[u8], limits: Limits) -> io::Result<Self> {
        let metadata = inspect(data, limits)?;
        let mut meshes = Vec::new();
        reserve(&mut meshes, metadata.mesh_count)?;
        for index in 0..metadata.mesh_count {
            let base = HEADER_BYTES + index * MESH_HEADER_BYTES;
            let layout = mesh_layout(data, base)?;
            let vertices = usize::try_from(layout.vertex_count)
                .map_err(|_| invalid("VHV vertex count exceeds the address space"))?;
            let offset = usize::try_from(word(data, base + 8)?)
                .map_err(|_| invalid("VHV payload offset exceeds the address space"))?;
            let payload = bytes(data, offset, vertex_bytes(vertices)?)?;
            let mut colors = Vec::new();
            reserve(&mut colors, vertices)?;
            colors.extend_from_slice(bytemuck::cast_slice(payload));
            meshes.push(Mesh { layout, colors });
        }
        Ok(Self {
            checksum: metadata.checksum,
            meshes,
        })
    }

    /// Checks exact ordered hardware metadata supplied independently by the caller.
    pub fn validate_layout(&self, checksum: u32, expected: &[MeshLayout]) -> io::Result<()> {
        if self.checksum != checksum {
            return Err(invalid(
                "VHV checksum differs from the expected model checksum",
            ));
        }
        if self.meshes.len() != expected.len() {
            return Err(invalid(
                "VHV mesh count differs from the expected hardware layout",
            ));
        }
        for (mesh, layout) in self.meshes.iter().zip(expected) {
            validate_lod(mesh.layout.lod)?;
            if mesh.layout != *layout || mesh.colors.len() != layout.vertex_count as usize {
                return Err(invalid(
                    "VHV mesh differs from the expected ordered hardware layout",
                ));
            }
        }
        Ok(())
    }

    /// Validates all writer input before the caller admits an output allocation.
    pub fn encoded_len(&self, limits: Limits) -> io::Result<usize> {
        Ok(self.encoding(limits)?.encoded_bytes)
    }

    pub fn to_bytes(&self, limits: Limits) -> io::Result<Vec<u8>> {
        let encoding = self.encoding(limits)?;
        let mut data = Vec::new();
        reserve(&mut data, encoding.encoded_bytes)?;
        data.resize(encoding.encoded_bytes, 0);
        put_word(&mut data, 0, VERSION);
        put_word(&mut data, 4, self.checksum);
        put_word(&mut data, 8, VERTEX_COLOR);
        put_word(&mut data, 12, VERTEX_BYTES as u32);
        put_word(&mut data, 16, encoding.vertex_count as u32);
        put_word(&mut data, 20, self.meshes.len() as u32);
        let mut offset = encoding.payload_start;
        for (index, mesh) in self.meshes.iter().enumerate() {
            let base = HEADER_BYTES + index * MESH_HEADER_BYTES;
            put_word(&mut data, base, mesh.layout.lod);
            put_word(&mut data, base + 4, mesh.layout.vertex_count);
            put_word(&mut data, base + 8, offset as u32);
            let payload: &[u8] = bytemuck::cast_slice(&mesh.colors);
            data[offset..offset + payload.len()].copy_from_slice(payload);
            offset += payload.len();
        }
        Ok(data)
    }

    fn encoding(&self, limits: Limits) -> io::Result<Encoding> {
        limits.validate()?;
        if self.meshes.len() > limits.max_meshes {
            return Err(invalid("VHV mesh count exceeds its budget"));
        }
        let payload_start = aligned(directory_end(self.meshes.len())?)?;
        let mut vertex_count = 0_usize;
        for mesh in &self.meshes {
            validate_lod(mesh.layout.lod)?;
            if mesh.colors.len() != mesh.layout.vertex_count as usize {
                return Err(invalid("VHV mesh color count differs from its descriptor"));
            }
            vertex_count = vertex_count
                .checked_add(mesh.colors.len())
                .filter(|&value| value <= limits.max_vertices)
                .ok_or_else(|| invalid("VHV aggregate vertex count exceeds its budget"))?;
        }
        let payload_end = payload_start
            .checked_add(vertex_bytes(vertex_count)?)
            .ok_or_else(|| invalid("VHV payload size overflow"))?;
        let encoded_bytes = aligned(payload_end)?;
        if encoded_bytes > limits.max_bytes {
            return Err(invalid("VHV encoded file exceeds its byte budget"));
        }
        Ok(Encoding {
            payload_start,
            encoded_bytes,
            vertex_count,
        })
    }
}

fn mesh_layout(data: &[u8], base: usize) -> io::Result<MeshLayout> {
    let lod = word(data, base)?;
    validate_lod(lod)?;
    Ok(MeshLayout {
        lod,
        vertex_count: word(data, base + 4)?,
    })
}

fn validate_lod(lod: u32) -> io::Result<()> {
    if lod >= MAX_LODS {
        return Err(invalid("unsupported VHV LOD index; expected 0 through 7"));
    }
    Ok(())
}

fn count(value: u32, maximum: usize, name: &str) -> io::Result<usize> {
    usize::try_from(value)
        .ok()
        .filter(|&count| count <= maximum)
        .ok_or_else(|| invalid(format!("{name} count exceeds its budget")))
}

fn directory_end(meshes: usize) -> io::Result<usize> {
    meshes
        .checked_mul(MESH_HEADER_BYTES)
        .and_then(|size| size.checked_add(HEADER_BYTES))
        .ok_or_else(|| invalid("VHV mesh directory size overflow"))
}

fn vertex_bytes(vertices: usize) -> io::Result<usize> {
    vertices
        .checked_mul(VERTEX_BYTES)
        .ok_or_else(|| invalid("VHV vertex byte size overflow"))
}

fn aligned(offset: usize) -> io::Result<usize> {
    offset
        .checked_add(ALIGNMENT - 1)
        .map(|value| value & !(ALIGNMENT - 1))
        .ok_or_else(|| invalid("VHV alignment overflow"))
}

fn bytes(data: &[u8], offset: usize, size: usize) -> io::Result<&[u8]> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| invalid("VHV byte range overflow"))?;
    data.get(offset..end)
        .ok_or_else(|| invalid("VHV byte range exceeds the file"))
}

fn word(data: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(
        bytes(data, offset, 4)?.try_into().unwrap(),
    ))
}

fn put_word(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn zeroes(data: &[u8], message: &str) -> io::Result<()> {
    if data.iter().any(|&value| value != 0) {
        return Err(invalid(message));
    }
    Ok(())
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> io::Result<()> {
    values
        .try_reserve_exact(count)
        .map_err(|error| io::Error::other(format!("VHV allocation failed: {error}")))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
