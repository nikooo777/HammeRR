//! Terrain arrays generated from an explicitly selected BSP channel and native arithmetic profile.

use std::io;

pub mod basis;
use super::{
    Bsp, DisplacementInfo, Face, FaceGeometry, LUMP_COUNT, LightingKind, NativeDisplacementProfile,
    NativeDisplacementRounding, TexInfo, invalid, lump,
};

const MAX_TEXINFO_BYTES: usize = 16 * 1024 * 1024;
const FLOAT_SIGN: u32 = 1 << 31;
const FLOAT_EXPONENT: u32 = 0x7f80_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTerrainVectorStatus {
    FiniteNonzero,
    Zero,
    NonFinite,
}

impl NativeTerrainVectorStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiniteNonzero => "finite_nonzero",
            Self::Zero => "zero",
            Self::NonFinite => "nonfinite",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTerrainVector {
    bits: [u32; 3],
    status: NativeTerrainVectorStatus,
}

impl NativeTerrainVector {
    fn new(bits: [u32; 3]) -> Self {
        let status = if bits
            .iter()
            .any(|bits| bits & FLOAT_EXPONENT == FLOAT_EXPONENT)
        {
            NativeTerrainVectorStatus::NonFinite
        } else if bits.iter().all(|bits| bits & !FLOAT_SIGN == 0) {
            NativeTerrainVectorStatus::Zero
        } else {
            NativeTerrainVectorStatus::FiniteNonzero
        };
        Self { bits, status }
    }

    pub const fn bits(self) -> [u32; 3] {
        self.bits
    }

    pub const fn status(self) -> NativeTerrainVectorStatus {
        self.status
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTerrainSample {
    position: [u32; 3],
    normal: NativeTerrainVector,
    tangent_s: NativeTerrainVector,
    tangent_t: NativeTerrainVector,
}

impl NativeTerrainSample {
    pub const fn position_bits(self) -> [u32; 3] {
        self.position
    }

    pub const fn normal(self) -> NativeTerrainVector {
        self.normal
    }

    pub const fn tangent_s(self) -> NativeTerrainVector {
        self.tangent_s
    }

    pub const fn tangent_t(self) -> NativeTerrainVector {
        self.tangent_t
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NativeTerrainSource {
    ordinal: u16,
    info: DisplacementInfo,
    face_record: Face,
    base_quad: [[f32; 3]; 4],
    native_base_quad: [[f32; 3]; 4],
    texinfo: TexInfo,
}

impl NativeTerrainSource {
    pub const fn ordinal(self) -> u16 {
        self.ordinal
    }

    pub const fn face(self) -> u16 {
        self.info.face
    }

    pub const fn power(self) -> i32 {
        self.info.power
    }

    pub fn grid_size(self) -> usize {
        (1 << self.info.power) + 1
    }

    pub fn vertex_count(self) -> usize {
        self.grid_size().pow(2)
    }

    pub const fn info(self) -> DisplacementInfo {
        self.info
    }

    pub const fn face_record(self) -> Face {
        self.face_record
    }

    pub const fn base_quad(self) -> [[f32; 3]; 4] {
        self.base_quad
    }

    pub const fn native_base_quad(self) -> [[f32; 3]; 4] {
        self.native_base_quad
    }

    pub const fn texinfo(self) -> TexInfo {
        self.texinfo
    }
}

pub struct NativeTerrainDisplacement {
    source: NativeTerrainSource,
    samples: Vec<NativeTerrainSample>,
}

impl NativeTerrainDisplacement {
    pub const fn source(&self) -> &NativeTerrainSource {
        &self.source
    }

    pub const fn ordinal(&self) -> u16 {
        self.source.ordinal()
    }

    pub const fn face(&self) -> u16 {
        self.source.face()
    }

    pub const fn power(&self) -> i32 {
        self.source.power()
    }

    pub fn grid_size(&self) -> usize {
        self.source.grid_size()
    }

    pub fn samples(&self) -> &[NativeTerrainSample] {
        &self.samples
    }
}

pub struct NativeTerrainSnapshot {
    channel: LightingKind,
    bsp_version: i32,
    map_revision: i32,
    position_rounding_mask: u8,
    position_roundings: Vec<NativeDisplacementRounding>,
    displacements: Vec<NativeTerrainDisplacement>,
}

impl NativeTerrainSnapshot {
    pub const fn channel(&self) -> LightingKind {
        self.channel
    }

    pub const fn bsp_version(&self) -> i32 {
        self.bsp_version
    }

    pub const fn map_revision(&self) -> i32 {
        self.map_revision
    }

    /// Bit indices follow `NativeDisplacementRounding::ALL`. These are position
    /// correspondence candidates, not observations of generator MXCSR.
    pub const fn position_rounding_mask(&self) -> u8 {
        self.position_rounding_mask
    }

    pub fn position_roundings(&self) -> &[NativeDisplacementRounding] {
        &self.position_roundings
    }

    pub fn displacements(&self) -> &[NativeTerrainDisplacement] {
        &self.displacements
    }

    pub fn sample_count(&self) -> usize {
        self.displacements
            .iter()
            .map(|disp| disp.samples.len())
            .sum()
    }

    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.displacements.capacity() * size_of::<NativeTerrainDisplacement>()
            + self.position_roundings.capacity() * size_of::<NativeDisplacementRounding>()
            + self
                .displacements
                .iter()
                .map(|disp| disp.samples.capacity() * size_of::<NativeTerrainSample>())
                .sum::<usize>()
    }
}

pub struct NativeTerrainCarrier {
    channel: LightingKind,
    displacements: Vec<NativeTerrainSource>,
    position_candidates: Vec<[Vec<[f32; 3]>; 4]>,
}

impl NativeTerrainCarrier {
    pub fn new_controlled(
        bsp: &Bsp,
        channel: LightingKind,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        check()?;
        if bsp.lumps.len() != LUMP_COUNT || ![19, 20].contains(&bsp.version) {
            return Err(invalid(
                "Unsupported native terrain BSP version or lump count",
            ));
        }
        let geometry = FaceGeometry::new_controlled(bsp, channel, check)?;
        let data = bsp
            .displacements()?
            .ok_or_else(|| invalid("Compiled BSP has no displacement catalog"))?;
        let texinfo_lump = bsp.lump(lump::TEXINFO);
        if texinfo_lump.version != 0
            || texinfo_lump.fourcc != [0; 4]
            || texinfo_lump.data.len() > MAX_TEXINFO_BYTES
        {
            return Err(invalid(
                "Unsupported native terrain texinfo metadata or size",
            ));
        }
        let texinfos = bsp.records::<TexInfo>()?;
        let mut displacements = Vec::with_capacity(data.infos.len());
        let mut position_candidates = Vec::with_capacity(data.infos.len());
        let mut vertex_start = 0usize;
        let mut triangle_start = 0usize;
        for (ordinal, info) in data.infos.iter().copied().enumerate() {
            check()?;
            if usize::try_from(info.vertex_start).ok() != Some(vertex_start)
                || usize::try_from(info.triangle_start).ok() != Some(triangle_start)
            {
                return Err(invalid(
                    "Native terrain requires cumulative catalog vertex and triangle ranges",
                ));
            }
            vertex_start += info.vertex_count()?;
            triangle_start += info.triangle_count()?;
            let face_ordinal = usize::from(info.face);
            let face_record = *geometry
                .faces()
                .get(face_ordinal)
                .ok_or_else(|| invalid("Native terrain FACE is out of bounds"))?;
            if face_record.dispinfo != ordinal as i16 {
                return Err(invalid("Native terrain FACE disagrees with DISPINFO order"));
            }
            let winding = geometry.base_winding(face_ordinal)?;
            if winding.model != 0 {
                return Err(invalid("Native terrain basis requires Model0 terrain"));
            }
            let base_quad = winding
                .positions
                .try_into()
                .map_err(|_| invalid("Native terrain FACE must be a BASE quad"))?;
            let texinfo = usize::try_from(face_record.texinfo)
                .ok()
                .and_then(|index| texinfos.get(index))
                .copied()
                .ok_or_else(|| invalid("Native terrain FACE texinfo is out of bounds"))?;
            if texinfo
                .texture_vecs
                .iter()
                .chain(&texinfo.lightmap_vecs)
                .flatten()
                .any(|value| !value.is_finite())
            {
                return Err(invalid("Native terrain texinfo vectors must be finite"));
            }
            let mut candidates = std::array::from_fn(|_| Vec::new());
            for (index, rounding) in NativeDisplacementRounding::ALL.into_iter().enumerate() {
                check()?;
                candidates[index] = data.native_positions(
                    ordinal,
                    base_quad,
                    NativeDisplacementProfile::CssLinux64Build25338318 { rounding },
                )?;
            }
            displacements.push(NativeTerrainSource {
                ordinal: ordinal as u16,
                info,
                face_record,
                base_quad,
                native_base_quad: data.native_base_quad(
                    ordinal,
                    base_quad,
                    NativeDisplacementProfile::CssLinux64Build25338318 {
                        rounding: NativeDisplacementRounding::NearestEven,
                    },
                )?,
                texinfo,
            });
            position_candidates.push(candidates);
        }
        if vertex_start != data.vertices.len() || triangle_start != data.triangles.len() {
            return Err(invalid(
                "Native terrain requires complete catalog vertex and triangle coverage",
            ));
        }
        check()?;
        Ok(Self {
            channel,
            displacements,
            position_candidates,
        })
    }

    pub const fn channel(&self) -> LightingKind {
        self.channel
    }

    pub fn displacements(&self) -> &[NativeTerrainSource] {
        &self.displacements
    }
}
