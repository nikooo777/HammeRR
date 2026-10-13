use std::io;

use bytemuck::{Pod, Zeroable};

use super::{Bsp, Leaf, LightingKind, Lump, Record, RgbExp32, invalid, lump};

pub const MAX_LEAF_AMBIENT_SAMPLES: usize = u16::MAX as usize * 2;
const MAX_LEAVES: usize = 65_536;
const LEGACY_LEAF_BYTES: usize = 56;
const LEGACY_CUBE_OFFSET: usize = 30;

/// Radiance in +X, -X, +Y, -Y, +Z, -Z order.
pub type LightCube = [RgbExp32; 6];

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct LeafAmbientIndex {
    pub sample_count: u16,
    /// A sample offset, or a leaf reference when sample_count is zero.
    pub first_sample: u16,
}

impl Record for LeafAmbientIndex {
    const LUMP: usize = lump::LEAF_AMBIENT_INDEX;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct LeafAmbientSample {
    pub cube: LightCube,
    pub position: [u8; 3],
    pub padding: u8,
}

impl Record for LeafAmbientSample {
    const LUMP: usize = lump::LEAF_AMBIENT_LIGHTING;
    const VERSION: i32 = 1;
}

impl LeafAmbientSample {
    pub fn world_position(&self, leaf: &Leaf) -> io::Result<[f64; 3]> {
        if leaf.mins.iter().zip(leaf.maxs).any(|(&min, max)| min > max) {
            return Err(invalid("ambient sample has reversed leaf bounds"));
        }
        Ok(std::array::from_fn(|axis| {
            f64::from(leaf.mins[axis])
                + (f64::from(leaf.maxs[axis]) - f64::from(leaf.mins[axis]))
                    * f64::from(self.position[axis])
                    / 255.0
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafAmbient {
    Embedded(Vec<LightCube>),
    Dense {
        cubes: Vec<LightCube>,
        version: i32,
    },
    Indexed {
        indices: Vec<LeafAmbientIndex>,
        samples: Vec<LeafAmbientSample>,
    },
}

impl LeafAmbient {
    pub fn sample_leaf(&self, leaf: usize) -> io::Result<Option<usize>> {
        match self {
            Self::Embedded(cubes) | Self::Dense { cubes, .. } => {
                if leaf >= cubes.len() {
                    return Err(invalid("ambient leaf index is outside the map"));
                }
                Ok(Some(leaf))
            }
            Self::Indexed { indices, .. } => {
                if leaf >= indices.len() {
                    return Err(invalid("ambient leaf index is outside the map"));
                }
                let mut current = leaf;
                for _ in 0..indices.len() {
                    let index = indices
                        .get(current)
                        .ok_or_else(|| invalid("ambient leaf reference is outside the map"))?;
                    if index.sample_count != 0 {
                        return Ok(Some(current));
                    }
                    let next = usize::from(index.first_sample);
                    if next == current {
                        return Ok(None);
                    }
                    current = next;
                }
                Err(invalid("ambient leaf aliases contain a cycle"))
            }
        }
    }

    fn validate(&self, leaves: usize) -> io::Result<()> {
        if leaves > MAX_LEAVES {
            return Err(invalid("ambient leaf count exceeds Source limits"));
        }
        match self {
            Self::Embedded(cubes) | Self::Dense { cubes, .. } => {
                if cubes.len() != leaves {
                    return Err(invalid("ambient cubes must match the leaf count"));
                }
            }
            Self::Indexed { indices, samples } => {
                if indices.len() != leaves || samples.len() > MAX_LEAF_AMBIENT_SAMPLES {
                    return Err(invalid(
                        "ambient indices or sample count exceed leaf limits",
                    ));
                }
                for index in indices {
                    let start = usize::from(index.first_sample);
                    if index.sample_count == 0 {
                        if start >= leaves {
                            return Err(invalid("ambient leaf alias is outside the map"));
                        }
                    } else if start + usize::from(index.sample_count) > samples.len() {
                        return Err(invalid("ambient sample range is outside its lump"));
                    }
                }
                let mut visited = vec![0u8; leaves];
                let mut path = Vec::new();
                for start in 0..leaves {
                    if visited[start] != 0 {
                        continue;
                    }
                    path.clear();
                    let mut current = start;
                    loop {
                        if visited[current] == 2 {
                            break;
                        }
                        if visited[current] == 1 {
                            return Err(invalid("ambient leaf aliases contain a cycle"));
                        }
                        visited[current] = 1;
                        path.push(current);
                        let index = indices[current];
                        if index.sample_count != 0 || usize::from(index.first_sample) == current {
                            break;
                        }
                        current = usize::from(index.first_sample);
                    }
                    for &index in &path {
                        visited[index] = 2;
                    }
                }
            }
        }
        Ok(())
    }
}

fn slots(kind: LightingKind) -> (usize, usize) {
    match kind {
        LightingKind::Ldr => (lump::LEAF_AMBIENT_INDEX, lump::LEAF_AMBIENT_LIGHTING),
        LightingKind::Hdr => (
            lump::LEAF_AMBIENT_INDEX_HDR,
            lump::LEAF_AMBIENT_LIGHTING_HDR,
        ),
    }
}

fn leaf_count(bsp: &Bsp) -> io::Result<usize> {
    let stride = match bsp.lumps[lump::LEAFS].version {
        0 => LEGACY_LEAF_BYTES,
        1 => size_of::<Leaf>(),
        _ => return Err(invalid("unsupported ambient leaf version")),
    };
    let leaves = bsp.checked(lump::LEAFS, stride)?.len() / stride;
    if leaves > MAX_LEAVES {
        return Err(invalid("ambient leaf count exceeds Source limits"));
    }
    Ok(leaves)
}

impl Bsp {
    pub fn leaf_ambient(&self, kind: LightingKind) -> io::Result<Option<LeafAmbient>> {
        let leaf_version = self.lumps[lump::LEAFS].version;
        let leaves = leaf_count(self)?;
        let (index_slot, sample_slot) = slots(kind);
        let data = if !self.lumps[index_slot].data.is_empty() {
            if leaf_version != 1
                || self.lumps[index_slot].version != 0
                || (!self.lumps[sample_slot].data.is_empty()
                    && self.lumps[sample_slot].version != 1)
            {
                return Err(invalid("unsupported indexed ambient lump version"));
            }
            let index_bytes = self.checked(index_slot, size_of::<LeafAmbientIndex>())?;
            let sample_bytes = self.checked(sample_slot, size_of::<LeafAmbientSample>())?;
            if index_bytes.len() / size_of::<LeafAmbientIndex>() != leaves
                || sample_bytes.len() / size_of::<LeafAmbientSample>() > MAX_LEAF_AMBIENT_SAMPLES
            {
                return Err(invalid(
                    "ambient index or sample count exceeds Source limits",
                ));
            }
            LeafAmbient::Indexed {
                indices: bytemuck::pod_collect_to_vec(index_bytes),
                samples: bytemuck::pod_collect_to_vec(sample_bytes),
            }
        } else if !self.lumps[sample_slot].data.is_empty() {
            let bytes = self.checked(sample_slot, size_of::<LightCube>())?;
            let version = self.lumps[sample_slot].version;
            if leaf_version != 1
                || bytes.len() / size_of::<LightCube>() != leaves
                || (version != 0 && usize::try_from(version).ok() != Some(bytes.len()))
            {
                return Err(invalid("unsupported dense legacy ambient layout"));
            }
            LeafAmbient::Dense {
                cubes: bytemuck::pod_collect_to_vec(bytes),
                version,
            }
        } else if kind == LightingKind::Ldr && leaf_version == 0 && leaves != 0 {
            let bytes = self.checked(lump::LEAFS, LEGACY_LEAF_BYTES)?;
            LeafAmbient::Embedded(
                bytes
                    .as_chunks::<LEGACY_LEAF_BYTES>()
                    .0
                    .iter()
                    .map(|leaf| {
                        bytemuck::pod_read_unaligned(
                            &leaf[LEGACY_CUBE_OFFSET..LEGACY_CUBE_OFFSET + size_of::<LightCube>()],
                        )
                    })
                    .collect(),
            )
        } else {
            return Ok(None);
        };
        data.validate(leaves)?;
        Ok(Some(data))
    }

    pub fn set_leaf_ambient(&mut self, kind: LightingKind, data: &LeafAmbient) -> io::Result<()> {
        let leaves = leaf_count(self)?;
        data.validate(leaves)?;
        let (index_slot, sample_slot) = slots(kind);
        let (indices, samples) = match data {
            LeafAmbient::Embedded(cubes) => {
                if kind != LightingKind::Ldr
                    || self.lumps[lump::LEAFS].version != 0
                    || !self.lumps[index_slot].data.is_empty()
                    || !self.lumps[sample_slot].data.is_empty()
                {
                    return Err(invalid("embedded ambient needs LDR version-0 leaves"));
                }
                let mut bytes = self.checked(lump::LEAFS, LEGACY_LEAF_BYTES)?.to_vec();
                for (leaf, cube) in bytes
                    .as_chunks_mut::<LEGACY_LEAF_BYTES>()
                    .0
                    .iter_mut()
                    .zip(cubes)
                {
                    leaf[LEGACY_CUBE_OFFSET..LEGACY_CUBE_OFFSET + size_of::<LightCube>()]
                        .copy_from_slice(bytemuck::bytes_of(cube));
                }
                self.lumps[lump::LEAFS].data = bytes;
                return Ok(());
            }
            LeafAmbient::Dense { cubes, version } => {
                if self.lumps[lump::LEAFS].version != 1 {
                    return Err(invalid("separate ambient lumps need version-1 leaves"));
                }
                let bytes = bytemuck::cast_slice(cubes).to_vec();
                if *version != 0 && usize::try_from(*version).ok() != Some(bytes.len()) {
                    return Err(invalid("unsupported dense legacy ambient version"));
                }
                let indices = if self.lumps[index_slot].data.is_empty() {
                    self.lumps[index_slot].clone()
                } else {
                    Lump::default()
                };
                (
                    indices,
                    Lump {
                        version: *version,
                        data: bytes,
                        ..Lump::default()
                    },
                )
            }
            LeafAmbient::Indexed { indices, samples } => {
                if self.lumps[lump::LEAFS].version != 1 {
                    return Err(invalid("separate ambient lumps need version-1 leaves"));
                }
                (
                    Lump {
                        data: bytemuck::cast_slice(indices).to_vec(),
                        ..Lump::default()
                    },
                    Lump {
                        version: 1,
                        data: bytemuck::cast_slice(samples).to_vec(),
                        ..Lump::default()
                    },
                )
            }
        };
        self.lumps[index_slot] = indices;
        self.lumps[sample_slot] = samples;
        Ok(())
    }
}
