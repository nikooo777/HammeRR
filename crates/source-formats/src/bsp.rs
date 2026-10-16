//! Compiled maps (BSP), versions 19 and 20 as used by Counter-Strike: Source.
//!
//! A BSP is a header with 64 lump slots followed by the lump data. Most
//! lumps are arrays of fixed-size little-endian records; [`Bsp::records`]
//! and [`Bsp::set_records`] convert them to the structs in this module.
//! The game lump holds its own directory with absolute file offsets, so it
//! is kept split into [`GameLump`]s and reassembled on write.

use std::io;
use std::path::Path;

use bytemuck::{Pod, Zeroable};
mod ambient;
pub use ambient::{
    LeafAmbient, LeafAmbientIndex, LeafAmbientSample, LightCube, MAX_LEAF_AMBIENT_SAMPLES,
};
mod areas;
pub use areas::{AreaData, MAX_AREA_PORTALS, MAX_AREAS, MAX_CLIP_PORTAL_VERTICES};
mod displacements;
mod lighting;
mod overlays;
pub use overlays::{MAX_OVERLAY_FACES, MAX_OVERLAYS, Overlay, OverlayData, OverlayFade};
mod pak;
pub use pak::{PakArchive, PakEntry, PakLimits, replace_bsp_pak, replace_bsp_pak_controlled};
mod water_overlays;
pub use water_overlays::{
    MAX_WATER_OVERLAY_FACES, MAX_WATER_OVERLAYS, WaterOverlay, WaterOverlayData,
};
mod occlusion;
pub use occlusion::{
    MAX_OCCLUDERS, MAX_OCCLUSION_POLYGONS, MAX_OCCLUSION_VERTEX_INDICES, OCCLUDER_INACTIVE,
    Occluder, OccluderPolygon, Occlusion,
};
mod physics;
mod static_props;
mod visibility;
pub use displacements::{
    DisplacementCorner, DisplacementGeometricSample, DisplacementInfo, DisplacementLightmapSample,
    DisplacementNeighbor, DisplacementSurfaceSample, DisplacementSurfaceSampler,
    DisplacementTriangle, DisplacementVertex, Displacements, INVALID_DISPLACEMENT_NEIGHBOR,
    MAX_DISPLACEMENT_TRIANGLES, MAX_DISPLACEMENT_VERTICES, MAX_DISPLACEMENTS,
};
pub use lighting::{
    FaceLighting, Lighting, LightingKind, MAX_LIGHTING_BYTES, MAX_LIGHTING_FACES, MAX_WORLD_LIGHTS,
    RgbExp32, WorldLight, world_light_kind,
};
pub use physics::{
    CollisionHeader, MAX_COLLISION_BYTES, MAX_PHYSICS_BYTES, MAX_PHYSICS_KEY_BYTES,
    MAX_PHYSICS_MODELS, MAX_PHYSICS_SOLIDS, PhysicsCollision, PhysicsModel, PhysicsSolid,
};
pub use static_props::{
    MAX_STATIC_PROP_BYTES, MAX_STATIC_PROP_LEAVES, MAX_STATIC_PROP_MODELS, MAX_STATIC_PROPS,
    STATIC_PROP_NAME_BYTES, StaticProp, StaticProps,
};
pub use visibility::{MAX_VISIBILITY_BYTES, MAX_VISIBILITY_CLUSTERS, Visibility, VisibilityKind};

pub const IDENT: [u8; 4] = *b"VBSP";
pub const LUMP_COUNT: usize = 64;
const HEADER_SIZE: usize = 8 + LUMP_COUNT * 16 + 4;

pub mod lump {
    pub const ENTITIES: usize = 0;
    pub const PLANES: usize = 1;
    pub const TEXDATA: usize = 2;
    pub const VERTEXES: usize = 3;
    pub const VISIBILITY: usize = 4;
    pub const NODES: usize = 5;
    pub const TEXINFO: usize = 6;
    pub const FACES: usize = 7;
    pub const LIGHTING: usize = 8;
    pub const OCCLUSION: usize = 9;
    pub const LEAFS: usize = 10;
    pub const FACEIDS: usize = 11;
    pub const EDGES: usize = 12;
    pub const SURFEDGES: usize = 13;
    pub const MODELS: usize = 14;
    pub const WORLDLIGHTS: usize = 15;
    pub const LEAFFACES: usize = 16;
    pub const LEAFBRUSHES: usize = 17;
    pub const BRUSHES: usize = 18;
    pub const BRUSHSIDES: usize = 19;
    pub const AREAS: usize = 20;
    pub const AREAPORTALS: usize = 21;
    pub const DISPINFO: usize = 26;
    pub const ORIGINALFACES: usize = 27;
    pub const PHYSDISP: usize = 28;
    pub const PHYSCOLLIDE: usize = 29;
    pub const VERTNORMALS: usize = 30;
    pub const VERTNORMALINDICES: usize = 31;
    pub const DISP_LIGHTMAP_ALPHAS: usize = 32;
    pub const DISP_VERTS: usize = 33;
    pub const DISP_LIGHTMAP_SAMPLE_POSITIONS: usize = 34;
    pub const GAME_LUMP: usize = 35;
    pub const LEAFWATERDATA: usize = 36;
    pub const PRIMITIVES: usize = 37;
    pub const PRIMVERTS: usize = 38;
    pub const PRIMINDICES: usize = 39;
    pub const PAKFILE: usize = 40;
    pub const CLIPPORTALVERTS: usize = 41;
    pub const CUBEMAPS: usize = 42;
    pub const TEXDATA_STRING_DATA: usize = 43;
    pub const TEXDATA_STRING_TABLE: usize = 44;
    pub const OVERLAYS: usize = 45;
    pub const LEAFMINDISTTOWATER: usize = 46;
    pub const FACE_MACRO_TEXTURE_INFO: usize = 47;
    pub const DISP_TRIS: usize = 48;
    pub const WATEROVERLAYS: usize = 50;
    pub const LEAF_AMBIENT_INDEX_HDR: usize = 51;
    pub const LEAF_AMBIENT_INDEX: usize = 52;
    pub const LIGHTING_HDR: usize = 53;
    pub const WORLDLIGHTS_HDR: usize = 54;
    pub const LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
    pub const LEAF_AMBIENT_LIGHTING: usize = 56;
    pub const FACES_HDR: usize = 58;
    pub const MAP_FLAGS: usize = 59;
    pub const OVERLAY_FADES: usize = 60;
}

/// Content flags of brushes and leafs.
pub mod contents {
    pub const EMPTY: i32 = 0;
    pub const SOLID: i32 = 0x1;
    pub const WINDOW: i32 = 0x2;
    pub const GRATE: i32 = 0x8;
    pub const SLIME: i32 = 0x10;
    pub const WATER: i32 = 0x20;
    pub const BLOCKLOS: i32 = 0x40;
    pub const OPAQUE: i32 = 0x80;
    pub const TESTFOGVOLUME: i32 = 0x100;
    pub const MOVEABLE: i32 = 0x4000;
    pub const AREAPORTAL: i32 = 0x8000;
    pub const PLAYERCLIP: i32 = 0x10000;
    pub const MONSTERCLIP: i32 = 0x20000;
    pub const ORIGIN: i32 = 0x0100_0000;
    pub const MONSTER: i32 = 0x0200_0000;
    pub const DEBRIS: i32 = 0x0400_0000;
    pub const DETAIL: i32 = 0x0800_0000;
    pub const TRANSLUCENT: i32 = 0x1000_0000;
    pub const LADDER: i32 = 0x2000_0000;
    pub const HITBOX: i32 = 0x4000_0000;
}

/// Surface flags of texinfos.
pub mod surf {
    pub const LIGHT: i32 = 0x1;
    pub const SKY2D: i32 = 0x2;
    pub const SKY: i32 = 0x4;
    pub const WARP: i32 = 0x8;
    pub const TRANS: i32 = 0x10;
    pub const NOPORTAL: i32 = 0x20;
    pub const TRIGGER: i32 = 0x40;
    pub const NODRAW: i32 = 0x80;
    pub const HINT: i32 = 0x100;
    pub const SKIP: i32 = 0x200;
    pub const NOLIGHT: i32 = 0x400;
    pub const BUMPLIGHT: i32 = 0x800;
    pub const NOSHADOWS: i32 = 0x1000;
    pub const NODECALS: i32 = 0x2000;
    pub const NOCHOP: i32 = 0x4000;
    pub const HITBOX: i32 = 0x8000;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lump {
    pub version: i32,
    /// Uncompressed size for LZMA-compressed lumps, otherwise zero.
    pub fourcc: [u8; 4],
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameLump {
    pub id: [u8; 4],
    pub flags: u16,
    pub version: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bsp {
    pub version: i32,
    pub map_revision: i32,
    /// All lump slots; the game lump's slot is empty, see `game_lumps`.
    pub lumps: Vec<Lump>,
    pub game_lumps: Vec<GameLump>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
    /// 0-2 for planes facing an axis, 3-5 for planes closest to one.
    pub kind: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct TexData {
    pub reflectivity: [f32; 3],
    pub name_index: i32,
    pub width: i32,
    pub height: i32,
    pub view_width: i32,
    pub view_height: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Node {
    pub plane: i32,
    /// Negative values are leafs: `-1 - leaf`.
    pub children: [i32; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub face_count: u16,
    pub area: i16,
    pub padding: i16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct TexInfo {
    /// `u = dot(xyz, s.xyz) + s.w`, in texels.
    pub texture_vecs: [[f32; 4]; 2],
    /// The same in luxels.
    pub lightmap_vecs: [[f32; 4]; 2],
    pub flags: i32,
    pub texdata: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Face {
    pub plane: u16,
    pub side: u8,
    pub on_node: u8,
    pub first_edge: i32,
    pub edge_count: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub fog_volume: i16,
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub area: f32,
    pub lightmap_mins: [i32; 2],
    pub lightmap_size: [i32; 2],
    pub original_face: i32,
    pub primitive_count: u16,
    pub first_primitive: u16,
    pub smoothing_groups: u32,
}

/// A leaf as stored in version 1 leaf lumps.
pub mod leaf_flags {
    pub const SKY: i16 = 1;
    pub const RADIAL: i16 = 2;
    pub const SKY2D: i16 = 4;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Leaf {
    pub contents: i32,
    pub cluster: i16,
    /// Area in the low 9 bits, flags in the high 7.
    pub area_flags: i16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_leaf_face: u16,
    pub leaf_face_count: u16,
    pub first_leaf_brush: u16,
    pub leaf_brush_count: u16,
    pub water_data: i16,
    pub padding: i16,
}

impl Leaf {
    pub fn area(&self) -> i16 {
        self.area_flags & 0x1ff
    }

    pub fn flags(&self) -> i16 {
        (self.area_flags >> 9) & 0x7f
    }
}

/// Version 0 leafs embed a 24-byte ambient light cube after `water_data`.
const LEAF_V0_SIZE: usize = 56;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Edge(pub [u16; 2]);

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Model {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub origin: [f32; 3],
    pub head_node: i32,
    pub first_face: i32,
    pub face_count: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Brush {
    pub first_side: i32,
    pub side_count: i32,
    pub contents: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct BrushSide {
    pub plane: u16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub bevel: u8,
    pub thin: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Area {
    pub portal_count: i32,
    pub first_portal: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct AreaPortal {
    pub portal_key: u16,
    pub other_area: u16,
    pub first_clip_vert: u16,
    pub clip_vert_count: u16,
    pub plane: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct Cubemap {
    pub origin: [i32; 3],
    pub size: u8,
    pub padding: [u8; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct LeafWaterData {
    pub surface_z: f32,
    pub min_z: f32,
    pub surface_texinfo: i16,
    pub padding: i16,
}

pub const MAX_LEAF_WATER_RECORDS: usize = i16::MAX as usize + 1;

pub const MAX_CUBEMAP_SAMPLES: usize = 1024;

/// Lumps of fixed-size records and the struct each one holds.
pub trait Record: Pod {
    const LUMP: usize;
    /// Lump version written by `set_records`.
    const VERSION: i32 = 0;
}

macro_rules! records {
    ($($ty:ty => $lump:ident $(v $version:literal)?),* $(,)?) => {
        $(impl Record for $ty {
            const LUMP: usize = lump::$lump;
            $(const VERSION: i32 = $version;)?
        })*
    };
}

records! {
    Plane => PLANES,
    TexData => TEXDATA,
    Node => NODES,
    TexInfo => TEXINFO,
    Face => FACES,
    Leaf => LEAFS v 1,
    Edge => EDGES,
    Model => MODELS,
    Brush => BRUSHES,
    BrushSide => BRUSHSIDES,
    Area => AREAS,
    AreaPortal => AREAPORTALS,
    Cubemap => CUBEMAPS,
    LeafWaterData => LEAFWATERDATA,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn i32_at(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn file_range(data: &[u8], offset: i32, length: i32) -> io::Result<&[u8]> {
    let offset = usize::try_from(offset).map_err(|_| invalid("negative lump offset"))?;
    let length = usize::try_from(length).map_err(|_| invalid("negative lump length"))?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("lump range overflow"))?;
    data.get(offset..end)
        .ok_or_else(|| invalid("lump lies outside the file"))
}

fn align4(out: &mut Vec<u8>) {
    out.resize(out.len().next_multiple_of(4), 0);
}

impl Default for Bsp {
    fn default() -> Self {
        Self::new()
    }
}

impl Bsp {
    /// An empty version 20 map.
    pub fn new() -> Self {
        Self {
            version: 20,
            map_revision: 0,
            lumps: vec![Lump::default(); LUMP_COUNT],
            game_lumps: Vec::new(),
        }
    }

    pub fn read(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn write(&self, path: impl AsRef<Path>) -> io::Result<()> {
        std::fs::write(path, self.to_bytes())
    }

    pub fn parse(data: &[u8]) -> io::Result<Self> {
        if data.len() < HEADER_SIZE || data[..4] != IDENT {
            return Err(invalid("not a VBSP file"));
        }
        let version = i32_at(data, 4);
        if !(19..=21).contains(&version) {
            return Err(invalid(format!("unsupported BSP version {version}")));
        }
        let mut lumps = Vec::with_capacity(LUMP_COUNT);
        for index in 0..LUMP_COUNT {
            let entry = 8 + index * 16;
            let bytes = file_range(data, i32_at(data, entry), i32_at(data, entry + 4))
                .map_err(|error| invalid(format!("lump {index}: {error}")))?;
            lumps.push(Lump {
                version: i32_at(data, entry + 8),
                fourcc: data[entry + 12..entry + 16].try_into().unwrap(),
                data: bytes.to_vec(),
            });
        }
        let map_revision = i32_at(data, 8 + LUMP_COUNT * 16);
        let game_lumps = parse_game_lumps(data, &lumps[lump::GAME_LUMP].data)?;
        lumps[lump::GAME_LUMP].data.clear();
        Ok(Self {
            version,
            map_revision,
            lumps,
            game_lumps,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0; HEADER_SIZE];
        out[..4].copy_from_slice(&IDENT);
        out[4..8].copy_from_slice(&self.version.to_le_bytes());
        let revision_at = 8 + LUMP_COUNT * 16;
        out[revision_at..revision_at + 4].copy_from_slice(&self.map_revision.to_le_bytes());
        for (index, lump) in self.lumps.iter().enumerate() {
            align4(&mut out);
            let offset = out.len();
            if index == lump::GAME_LUMP {
                write_game_lumps(&mut out, &self.game_lumps);
            } else {
                out.extend_from_slice(&lump.data);
            }
            let length = out.len() - offset;
            let entry = 8 + index * 16;
            let offset = if length == 0 { 0 } else { offset };
            out[entry..entry + 4].copy_from_slice(&(offset as i32).to_le_bytes());
            out[entry + 4..entry + 8].copy_from_slice(&(length as i32).to_le_bytes());
            out[entry + 8..entry + 12].copy_from_slice(&lump.version.to_le_bytes());
            out[entry + 12..entry + 16].copy_from_slice(&lump.fourcc);
        }
        out
    }

    pub fn lump(&self, index: usize) -> &Lump {
        &self.lumps[index]
    }

    pub fn is_compressed(&self, index: usize) -> bool {
        self.lumps[index].data.starts_with(b"LZMA")
    }

    fn checked(&self, index: usize, record_size: usize) -> io::Result<&[u8]> {
        if self.is_compressed(index) {
            return Err(invalid(format!("lump {index} is LZMA-compressed")));
        }
        let data = &self.lumps[index].data;
        if !data.len().is_multiple_of(record_size) {
            return Err(invalid(format!(
                "lump {index} is {} bytes, not a multiple of {record_size}",
                data.len()
            )));
        }
        Ok(data)
    }

    pub fn records<T: Record>(&self) -> io::Result<Vec<T>> {
        if T::LUMP == lump::LEAFS && self.lumps[lump::LEAFS].version == 0 {
            return self
                .leafs_v0()
                .map(|leafs| bytemuck::pod_collect_to_vec(&leafs));
        }
        let data = self.checked(T::LUMP, size_of::<T>())?;
        Ok(bytemuck::pod_collect_to_vec(data))
    }

    fn leafs_v0(&self) -> io::Result<Vec<Leaf>> {
        let data = self.checked(lump::LEAFS, LEAF_V0_SIZE)?;
        Ok(data
            .as_chunks::<LEAF_V0_SIZE>()
            .0
            .iter()
            .map(|chunk| {
                let mut leaf: Leaf = bytemuck::pod_read_unaligned(&chunk[..size_of::<Leaf>()]);
                leaf.padding = 0;
                leaf
            })
            .collect())
    }

    pub fn set_records<T: Record>(&mut self, records: &[T]) {
        self.lumps[T::LUMP] = Lump {
            version: T::VERSION,
            fourcc: [0; 4],
            data: bytemuck::cast_slice(records).to_vec(),
        };
    }

    /// Arrays of plain numbers: vertices, surfedges, leaf faces, ...
    pub fn values<T: Pod>(&self, index: usize) -> io::Result<Vec<T>> {
        Ok(bytemuck::pod_collect_to_vec(
            self.checked(index, size_of::<T>())?,
        ))
    }

    pub fn set_values<T: Pod>(&mut self, index: usize, values: &[T]) {
        self.lumps[index] = Lump {
            version: 0,
            fourcc: [0; 4],
            data: bytemuck::cast_slice(values).to_vec(),
        };
    }

    pub fn vertices(&self) -> io::Result<Vec<[f32; 3]>> {
        self.values(lump::VERTEXES)
    }

    pub fn surfedges(&self) -> io::Result<Vec<i32>> {
        self.values(lump::SURFEDGES)
    }

    pub fn leaf_faces(&self) -> io::Result<Vec<u16>> {
        self.values(lump::LEAFFACES)
    }

    pub fn leaf_brushes(&self) -> io::Result<Vec<u16>> {
        self.values(lump::LEAFBRUSHES)
    }

    /// The entity lump's KeyValues text.
    pub fn entities(&self) -> io::Result<&str> {
        let data = &self.lumps[lump::ENTITIES].data;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        std::str::from_utf8(&data[..end]).map_err(|_| invalid("entity lump is not UTF-8"))
    }

    pub fn set_entities(&mut self, text: &str) {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        self.lumps[lump::ENTITIES] = Lump {
            data,
            ..Lump::default()
        };
    }

    /// Material names referenced by texdata, in string table order.
    pub fn texdata_strings(&self) -> io::Result<Vec<String>> {
        let table: Vec<i32> = self.values(lump::TEXDATA_STRING_TABLE)?;
        let data = &self.lumps[lump::TEXDATA_STRING_DATA].data;
        table
            .iter()
            .map(|&offset| {
                let rest = data
                    .get(offset as usize..)
                    .ok_or_else(|| invalid("texdata string offset out of range"))?;
                let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
                Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
            })
            .collect()
    }

    pub fn set_texdata_strings<S: AsRef<str>>(&mut self, names: &[S]) {
        let mut data = Vec::new();
        let mut table = Vec::with_capacity(names.len());
        for name in names {
            table.push(data.len() as i32);
            data.extend_from_slice(name.as_ref().as_bytes());
            data.push(0);
        }
        self.lumps[lump::TEXDATA_STRING_DATA] = Lump {
            data,
            ..Lump::default()
        };
        self.set_values(lump::TEXDATA_STRING_TABLE, &table);
    }

    pub fn game_lump(&self, id: [u8; 4]) -> Option<&GameLump> {
        self.game_lumps.iter().find(|g| g.id == id)
    }
}

/// Game lump ids are stored as little-endian integers, so `sprp` reads
/// back as the bytes `prps`; `GameLump::id` holds the readable order.
fn game_lump_id(raw: i32) -> [u8; 4] {
    raw.to_be_bytes()
}

fn parse_game_lumps(file: &[u8], directory: &[u8]) -> io::Result<Vec<GameLump>> {
    if directory.is_empty() {
        return Ok(Vec::new());
    }
    if directory.len() < 4 {
        return Err(invalid("game lump directory is truncated"));
    }
    let count =
        usize::try_from(i32_at(directory, 0)).map_err(|_| invalid("negative game lump count"))?;
    if count > (directory.len() - 4) / 16 {
        return Err(invalid("game lump count exceeds its directory"));
    }
    let mut lumps = Vec::with_capacity(count);
    for i in 0..count {
        let entry = 4 + i * 16;
        let header = directory
            .get(entry..entry + 16)
            .ok_or_else(|| invalid("game lump directory is truncated"))?;
        let data = file_range(file, i32_at(header, 8), i32_at(header, 12))
            .map_err(|error| invalid(format!("game lump {i}: {error}")))?;
        lumps.push(GameLump {
            id: game_lump_id(i32_at(header, 0)),
            flags: u16_at(header, 4),
            version: u16_at(header, 6),
            data: data.to_vec(),
        });
    }
    Ok(lumps)
}

fn write_game_lumps(out: &mut Vec<u8>, lumps: &[GameLump]) {
    let start = out.len();
    let mut data_offset = start + 4 + lumps.len() * 16;
    out.extend_from_slice(&(lumps.len() as i32).to_le_bytes());
    for lump in lumps {
        out.extend_from_slice(&i32::from_be_bytes(lump.id).to_le_bytes());
        out.extend_from_slice(&lump.flags.to_le_bytes());
        out.extend_from_slice(&lump.version.to_le_bytes());
        out.extend_from_slice(&(data_offset as i32).to_le_bytes());
        out.extend_from_slice(&(lump.data.len() as i32).to_le_bytes());
        data_offset += lump.data.len();
    }
    for lump in lumps {
        out.extend_from_slice(&lump.data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_sizes_match_the_format() {
        assert_eq!(size_of::<Plane>(), 20);
        assert_eq!(size_of::<TexData>(), 32);
        assert_eq!(size_of::<Node>(), 32);
        assert_eq!(size_of::<TexInfo>(), 72);
        assert_eq!(size_of::<Face>(), 56);
        assert_eq!(size_of::<Leaf>(), 32);
        assert_eq!(size_of::<Edge>(), 4);
        assert_eq!(size_of::<Model>(), 48);
        assert_eq!(size_of::<Brush>(), 12);
        assert_eq!(size_of::<BrushSide>(), 8);
        assert_eq!(size_of::<Area>(), 8);
        assert_eq!(size_of::<AreaPortal>(), 12);
        assert_eq!(size_of::<Cubemap>(), 16);
        assert_eq!(size_of::<LeafWaterData>(), 12);
    }

    #[test]
    fn leaf_water_wire_padding_survives_read_write() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&64_f32.to_le_bytes());
        bytes.extend_from_slice(&(-32_f32).to_le_bytes());
        bytes.extend_from_slice(&17_i16.to_le_bytes());
        bytes.extend_from_slice(&0x1234_i16.to_le_bytes());
        let mut bsp = Bsp::new();
        bsp.lumps[lump::LEAFWATERDATA].data = bytes.clone();
        let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
        let records = parsed.records::<LeafWaterData>().unwrap();
        assert_eq!(
            records[0],
            LeafWaterData {
                surface_z: 64.0,
                min_z: -32.0,
                surface_texinfo: 17,
                padding: 0x1234,
            }
        );
        bsp.set_records(&records);
        assert_eq!(bsp.lumps[lump::LEAFWATERDATA].data, bytes);
    }

    #[test]
    fn cubemap_size_is_one_byte_and_padding_round_trips() {
        let mut bsp = Bsp::new();
        let mut bytes = Vec::new();
        for origin in [-12_i32, 34, -56] {
            bytes.extend_from_slice(&origin.to_le_bytes());
        }
        bytes.extend_from_slice(&[9, 0xab, 0xcd, 0xef]);
        bsp.lumps[lump::CUBEMAPS].data = bytes.clone();
        let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
        let samples = parsed.records::<Cubemap>().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].origin, [-12, 34, -56]);
        assert_eq!(samples[0].size, 9);
        assert_eq!(samples[0].padding, [0xab, 0xcd, 0xef]);
        bsp.set_records(&samples);
        assert_eq!(bsp.lumps[lump::CUBEMAPS].data, bytes);
    }

    #[test]
    fn round_trips_lumps() {
        let mut bsp = Bsp::new();
        bsp.set_entities("{\n\"classname\" \"worldspawn\"\n}\n");
        bsp.set_records(&[Plane {
            normal: [0.0, 0.0, 1.0],
            dist: 64.0,
            kind: 2,
        }]);
        bsp.set_values(lump::VERTEXES, &[[1.0f32, 2.0, 3.0]]);
        bsp.set_texdata_strings(&["TOOLS/TOOLSNODRAW", "BRICK/BRICKWALL001A"]);
        bsp.game_lumps.push(GameLump {
            id: *b"sprp",
            flags: 0,
            version: 10,
            data: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        });
        let parsed = Bsp::parse(&bsp.to_bytes()).unwrap();
        assert_eq!(parsed, bsp);
        assert_eq!(parsed.texdata_strings().unwrap()[1], "BRICK/BRICKWALL001A");
        assert_eq!(parsed.records::<Plane>().unwrap()[0].dist, 64.0);
        assert!(parsed.game_lump(*b"sprp").is_some());
    }
}
