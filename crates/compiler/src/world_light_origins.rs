use std::sync::Arc;

use sha2::{Digest, Sha256};
use source_formats::bsp::{Bsp, LightingKind, MAX_WORLD_LIGHTS, WorldLight, lump};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_CHANNEL_BYTES: usize = (MAX_BYTES
    - size_of::<WorldLightOrigins>()
    - 2 * size_of::<usize>()
    - crate::instances::MAX_OCCURRENCE_BYTES)
    / 2;

pub use crate::entity_origins::Origin;

#[derive(Debug)]
pub enum Source {
    Entity(Origin),
    EmittingFace { face: usize, material: Arc<str> },
}

#[derive(Debug, Default)]
pub struct WorldLightOrigins {
    pub(crate) channels: [Option<ChannelOrigins>; 2],
}

#[derive(Debug)]
pub(crate) struct ChannelOrigins {
    entries: Vec<Option<Source>>,
    digest: [u8; 32],
    bytes: usize,
}

impl ChannelOrigins {
    pub(crate) fn new(count: usize) -> Result<Self, String> {
        let bytes = count
            .checked_mul(size_of::<Option<Source>>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .filter(|&bytes| count <= MAX_WORLD_LIGHTS && bytes <= MAX_CHANNEL_BYTES)
            .ok_or("Compiled light origins exceed their channel storage budget")?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| "Compiled light origins cannot be allocated")?;
        Ok(Self {
            entries,
            digest: [0; 32],
            bytes,
        })
    }

    pub(crate) fn push_entity(
        &mut self,
        source: Option<u32>,
        instances: &crate::instances::Prepared<'_>,
    ) -> Result<(), String> {
        if self.entries.len() == self.entries.capacity() {
            return Err("Compiled light origin count exceeds the admitted native table".into());
        }
        let origin = source
            .map(|entity| Origin::capture(entity, instances, &mut self.bytes, MAX_CHANNEL_BYTES))
            .transpose()?;
        self.entries.push(origin.map(Source::Entity));
        Ok(())
    }

    pub(crate) fn push_emitting_face(
        &mut self,
        face: usize,
        material: &Arc<str>,
    ) -> Result<(), String> {
        if self.entries.len() == self.entries.capacity() {
            return Err("Compiled light origin count exceeds the admitted native table".into());
        }
        if face >= crate::face_origins::MAX_FACES || material.is_empty() {
            return Err(
                "Compiled emitting light needs a bounded face and original material".into(),
            );
        }
        self.bytes = material
            .len()
            .checked_add(2 * size_of::<usize>())
            .and_then(|bytes| self.bytes.checked_add(bytes))
            .filter(|&bytes| bytes <= MAX_CHANNEL_BYTES)
            .ok_or("Compiled emitting light materials exceed their channel storage budget")?;
        self.entries.push(Some(Source::EmittingFace {
            face,
            material: Arc::clone(material),
        }));
        Ok(())
    }

    pub(crate) fn finish(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != self.entries.len() * size_of::<WorldLight>() {
            return Err("Compiled light origin count differs from the native table".into());
        }
        self.digest = Sha256::digest(bytes).into();
        Ok(())
    }
}

impl WorldLightOrigins {
    pub fn associations(
        &self,
        bsp: &Bsp,
        kind: LightingKind,
        count: usize,
    ) -> Result<&[Option<Source>], &'static str> {
        let (index, lump) = match kind {
            LightingKind::Ldr => (0, lump::WORLDLIGHTS),
            LightingKind::Hdr => (1, lump::WORLDLIGHTS_HDR),
        };
        let table = self.channels[index]
            .as_ref()
            .ok_or("Source associations were not retained for this channel")?;
        if table.entries.len() != count
            || bsp.lumps[lump].data.len() != count * size_of::<WorldLight>()
        {
            return Err("Source association count differs from the native table");
        }
        let digest: [u8; 32] = Sha256::digest(&bsp.lumps[lump].data).into();
        if digest != table.digest {
            return Err("Source association digest differs from the native table");
        }
        Ok(&table.entries)
    }
}
