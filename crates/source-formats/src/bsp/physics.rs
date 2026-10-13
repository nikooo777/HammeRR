use std::collections::BTreeSet;
use std::io;

use super::{Bsp, Lump, Model, invalid, lump};

pub const MAX_PHYSICS_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_COLLISION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PHYSICS_MODELS: usize = 1024;
pub const MAX_PHYSICS_SOLIDS: usize = 32_767;
pub const MAX_PHYSICS_KEY_BYTES: usize = 65_535;
const MODEL_HEADER_BYTES: usize = 16;
const SENTINEL: [i32; 4] = [-1, -1, 0, 0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CollisionHeader {
    Convex {
        orthographic_areas: [f32; 3],
        reserved: u32,
    },
    Null,
    Unsupported {
        version: u16,
        kind: u16,
    },
}

/// Checked framing does not establish the safety of compact-body internals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsSolid<'a> {
    data: &'a [u8],
    header: CollisionHeader,
}

impl<'a> PhysicsSolid<'a> {
    pub fn parse(data: &'a [u8]) -> io::Result<Self> {
        if data.len() < 12 || data.len() > MAX_COLLISION_BYTES {
            return Err(invalid("physics solid size is outside supported bounds"));
        }
        if &data[..4] != b"VPHY" {
            return Err(invalid("physics solid has invalid VPHY magic"));
        }
        let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
        let kind = u16::from_le_bytes(data[6..8].try_into().unwrap());
        let header = match (version, kind) {
            (0x100, 0) => {
                if data.len() < 28 {
                    return Err(invalid("convex physics header is truncated"));
                }
                let orthographic_areas = std::array::from_fn(|index| {
                    f32::from_le_bytes(data[12 + index * 4..16 + index * 4].try_into().unwrap())
                });
                CollisionHeader::Convex {
                    orthographic_areas,
                    reserved: u32::from_le_bytes(data[24..28].try_into().unwrap()),
                }
            }
            (0x100, 1) => CollisionHeader::Null,
            _ => CollisionHeader::Unsupported { version, kind },
        };
        let header_size = match header {
            CollisionHeader::Convex { .. } => Some(28),
            CollisionHeader::Null => Some(12),
            CollisionHeader::Unsupported { .. } => None,
        };
        if let Some(header_size) = header_size {
            let size = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
            if size != data.len() - header_size {
                return Err(invalid(
                    "physics embedded body size does not match its record",
                ));
            }
        }
        Ok(Self { data, header })
    }

    pub fn bytes(self) -> &'a [u8] {
        self.data
    }

    pub fn header(self) -> CollisionHeader {
        self.header
    }

    pub fn body(self) -> Option<&'a [u8]> {
        match self.header {
            CollisionHeader::Convex { .. } => Some(&self.data[28..]),
            CollisionHeader::Null => Some(&self.data[12..]),
            CollisionHeader::Unsupported { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsModel<'a> {
    pub index: usize,
    pub solids: Vec<PhysicsSolid<'a>>,
    pub key_data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsCollision<'a> {
    pub models: Vec<PhysicsModel<'a>>,
}

fn take<'a>(data: &mut &'a [u8], count: usize) -> io::Result<&'a [u8]> {
    let (head, tail) = data
        .split_at_checked(count)
        .ok_or_else(|| invalid("physics model payload is truncated"))?;
    *data = tail;
    Ok(head)
}

fn integer(data: &mut &[u8]) -> io::Result<i32> {
    Ok(i32::from_le_bytes(take(data, 4)?.try_into().unwrap()))
}

fn bounded(value: i32, maximum: usize, label: &str) -> io::Result<usize> {
    if value < 0 || value as usize > maximum {
        return Err(invalid(label));
    }
    Ok(value as usize)
}

fn check_keys(data: &[u8]) -> io::Result<()> {
    if data.is_empty()
        || data.len() > MAX_PHYSICS_KEY_BYTES
        || data.last() != Some(&0)
        || data[..data.len() - 1].contains(&0)
    {
        return Err(invalid(
            "physics key data needs bounded, singly terminated text",
        ));
    }
    Ok(())
}

impl<'a> PhysicsCollision<'a> {
    pub fn parse(mut data: &'a [u8], model_count: usize) -> io::Result<Self> {
        if data.len() > MAX_PHYSICS_BYTES || model_count > MAX_PHYSICS_MODELS {
            return Err(invalid(
                "physics data or model count exceeds supported bounds",
            ));
        }
        let mut models = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            let mut header = [0; 4];
            for value in &mut header {
                *value = integer(&mut data)?;
            }
            if header == SENTINEL {
                if !data.is_empty() {
                    return Err(invalid("physics data follows its end marker"));
                }
                return Ok(Self { models });
            }
            let index = bounded(
                header[0],
                MAX_PHYSICS_MODELS - 1,
                "physics model index is invalid",
            )?;
            if index >= model_count || !seen.insert(index) {
                return Err(invalid("physics model index is missing or duplicated"));
            }
            let data_size = bounded(
                header[1],
                MAX_PHYSICS_BYTES,
                "physics model data size is invalid",
            )?;
            let key_size = bounded(
                header[2],
                MAX_PHYSICS_KEY_BYTES,
                "physics key size is invalid",
            )?;
            let count = bounded(
                header[3],
                MAX_PHYSICS_SOLIDS,
                "physics solid count is invalid",
            )?;
            if count > data_size / 16 {
                return Err(invalid("physics solid count exceeds its model payload"));
            }
            let mut solids_data = take(&mut data, data_size)?;
            let key_data = take(&mut data, key_size)?;
            check_keys(key_data)?;
            let mut solids = Vec::with_capacity(count);
            for _ in 0..count {
                let size = bounded(
                    integer(&mut solids_data)?,
                    MAX_COLLISION_BYTES,
                    "physics solid size is invalid",
                )?;
                solids.push(PhysicsSolid::parse(take(&mut solids_data, size)?)?);
            }
            if !solids_data.is_empty() {
                return Err(invalid("physics model has unclaimed solid bytes"));
            }
            models.push(PhysicsModel {
                index,
                solids,
                key_data,
            });
        }
    }

    pub fn to_bytes(&self, model_count: usize) -> io::Result<Vec<u8>> {
        if model_count > MAX_PHYSICS_MODELS || self.models.len() > model_count {
            return Err(invalid("physics model count exceeds supported bounds"));
        }
        let mut size = MODEL_HEADER_BYTES;
        let mut sizes = Vec::with_capacity(self.models.len());
        let mut seen = BTreeSet::new();
        for model in &self.models {
            if model.index >= model_count || !seen.insert(model.index) {
                return Err(invalid("physics model index is missing or duplicated"));
            }
            if model.solids.len() > MAX_PHYSICS_SOLIDS {
                return Err(invalid("physics solid count exceeds supported bounds"));
            }
            check_keys(model.key_data)?;
            let data_size = model.solids.iter().try_fold(0_usize, |size, solid| {
                size.checked_add(4 + solid.data.len())
                    .filter(|&size| size <= MAX_PHYSICS_BYTES)
                    .ok_or_else(|| invalid("physics model size overflow"))
            })?;
            size = size
                .checked_add(MODEL_HEADER_BYTES + data_size + model.key_data.len())
                .ok_or_else(|| invalid("physics data size overflow"))?;
            if size > MAX_PHYSICS_BYTES {
                return Err(invalid("physics data exceeds supported bounds"));
            }
            sizes.push(data_size);
        }
        let mut data = Vec::with_capacity(size);
        for (model, data_size) in self.models.iter().zip(sizes) {
            for value in [
                model.index,
                data_size,
                model.key_data.len(),
                model.solids.len(),
            ] {
                data.extend_from_slice(&(value as i32).to_le_bytes());
            }
            for solid in &model.solids {
                data.extend_from_slice(&(solid.data.len() as i32).to_le_bytes());
                data.extend_from_slice(solid.data);
            }
            data.extend_from_slice(model.key_data);
        }
        for value in SENTINEL {
            data.extend_from_slice(&value.to_le_bytes());
        }
        Ok(data)
    }
}

impl Bsp {
    pub fn physics(&self) -> io::Result<Option<PhysicsCollision<'_>>> {
        let record = &self.lumps[lump::PHYSCOLLIDE];
        if record.data.is_empty() {
            return Ok(None);
        }
        if record.version != 0 {
            return Err(invalid("unsupported physics lump version"));
        }
        if record.fourcc != [0; 4] {
            return Err(invalid("compressed physics records are unsupported"));
        }
        let data = self.checked(lump::PHYSCOLLIDE, 1)?;
        let model_count =
            self.checked(lump::MODELS, size_of::<Model>())?.len() / size_of::<Model>();
        PhysicsCollision::parse(data, model_count).map(Some)
    }

    pub fn set_physics(&mut self, physics: &PhysicsCollision<'_>) -> io::Result<()> {
        let model_count =
            self.checked(lump::MODELS, size_of::<Model>())?.len() / size_of::<Model>();
        let data = physics.to_bytes(model_count)?;
        self.lumps[lump::PHYSCOLLIDE] = Lump {
            data,
            ..Lump::default()
        };
        Ok(())
    }
}
