//! Complete observed client arrays matched to an explicitly selected BSP channel.
//! The caller binds the exact BSP bytes and native profile/process/readiness receipt.

use std::io;

use super::{
    Bsp, DisplacementInfo, Face, FaceGeometry, LUMP_COUNT, LightingKind, NativeDisplacementProfile,
    NativeDisplacementRounding, TexInfo, invalid, lump,
};

pub const NATIVE_TERRAIN_VERTEX_STRIDE: usize = 64;
pub const NATIVE_TERRAIN_POSITION_OFFSET: usize = 0;
pub const NATIVE_TERRAIN_NORMAL_OFFSET: usize = 12;
pub const NATIVE_TERRAIN_TANGENT_S_OFFSET: usize = 24;
pub const NATIVE_TERRAIN_TANGENT_T_OFFSET: usize = 36;
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

    pub const fn texinfo(self) -> TexInfo {
        self.texinfo
    }
}

pub struct NativeTerrainReadback {
    pub ordinal: u16,
    pub power: i32,
    pub vertices: Vec<u8>,
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
    bsp_version: i32,
    map_revision: i32,
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
                return Err(invalid("Native terrain inspection requires Model0 terrain"));
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
            bsp_version: bsp.version,
            map_revision: bsp.map_revision,
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

    pub fn decode_controlled(
        &self,
        readbacks: &[NativeTerrainReadback],
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<NativeTerrainSnapshot> {
        check()?;
        if readbacks.len() != self.displacements.len() {
            return Err(invalid(
                "Native terrain readback is not the complete BSP catalog",
            ));
        }
        let mut matching = [true; 4];
        let mut displacements = Vec::with_capacity(self.displacements.len());
        for ((source, candidates), readback) in self
            .displacements
            .iter()
            .zip(&self.position_candidates)
            .zip(readbacks)
        {
            check()?;
            if readback.ordinal != source.ordinal() || readback.power != source.power() {
                return Err(invalid(
                    "Native terrain catalog order, ordinal or power mismatch",
                ));
            }
            let expected_bytes = source.vertex_count() * NATIVE_TERRAIN_VERTEX_STRIDE;
            if readback.vertices.len() != expected_bytes {
                return Err(invalid(
                    "Native terrain vertex readback is incomplete or oversized",
                ));
            }
            let mut samples = Vec::with_capacity(source.vertex_count());
            for (index, bytes) in readback
                .vertices
                .as_chunks::<NATIVE_TERRAIN_VERTEX_STRIDE>()
                .0
                .iter()
                .enumerate()
            {
                let position = vector_bits(bytes, NATIVE_TERRAIN_POSITION_OFFSET);
                if position
                    .iter()
                    .any(|bits| bits & FLOAT_EXPONENT == FLOAT_EXPONENT)
                {
                    return Err(invalid("Native terrain observed position must be finite"));
                }
                for (mode, expected) in candidates.iter().enumerate() {
                    if matching[mode]
                        && !position
                            .into_iter()
                            .zip(expected[index])
                            .all(|(observed, expected)| same_coordinate(observed, expected))
                    {
                        matching[mode] = false;
                    }
                }
                samples.push(NativeTerrainSample {
                    position,
                    normal: NativeTerrainVector::new(vector_bits(
                        bytes,
                        NATIVE_TERRAIN_NORMAL_OFFSET,
                    )),
                    tangent_s: NativeTerrainVector::new(vector_bits(
                        bytes,
                        NATIVE_TERRAIN_TANGENT_S_OFFSET,
                    )),
                    tangent_t: NativeTerrainVector::new(vector_bits(
                        bytes,
                        NATIVE_TERRAIN_TANGENT_T_OFFSET,
                    )),
                });
            }
            if !matching.iter().any(|matches| *matches) {
                return Err(invalid(
                    "Native terrain positions do not match one complete catalog profile",
                ));
            }
            displacements.push(NativeTerrainDisplacement {
                source: *source,
                samples,
            });
        }
        check()?;
        let mut position_rounding_mask = 0;
        let mut position_roundings = Vec::with_capacity(NativeDisplacementRounding::ALL.len());
        for (index, rounding) in NativeDisplacementRounding::ALL.into_iter().enumerate() {
            if matching[index] {
                position_rounding_mask |= 1 << index;
                position_roundings.push(rounding);
            }
        }
        Ok(NativeTerrainSnapshot {
            channel: self.channel,
            bsp_version: self.bsp_version,
            map_revision: self.map_revision,
            position_rounding_mask,
            position_roundings,
            displacements,
        })
    }
}

fn vector_bits(bytes: &[u8], offset: usize) -> [u32; 3] {
    std::array::from_fn(|axis| {
        let start = offset + axis * size_of::<u32>();
        u32::from_le_bytes(bytes[start..start + size_of::<u32>()].try_into().unwrap())
    })
}

fn same_coordinate(observed: u32, expected: f32) -> bool {
    let expected = expected.to_bits();
    observed == expected || (observed | expected) & !FLOAT_SIGN == 0
}
