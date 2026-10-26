use sha2::{Digest, Sha256};
use source_formats::bsp::{Bsp, LightingKind, MAX_WORLD_LIGHTS, WorldLight, lump};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_CHANNEL_BYTES: usize = (MAX_BYTES - size_of::<WorldLightOrigins>()) / 2;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Root {
        entity: u32,
    },
    Instance {
        instance: u32,
        file: String,
        entity: u32,
        expanded_entity: u32,
    },
}

#[derive(Debug, Default)]
pub struct WorldLightOrigins {
    pub(crate) channels: [Option<ChannelOrigins>; 2],
}

#[derive(Debug)]
pub(crate) struct ChannelOrigins {
    entries: Vec<Option<Origin>>,
    digest: [u8; 32],
    bytes: usize,
}

impl ChannelOrigins {
    pub(crate) fn new(count: usize) -> Result<Self, String> {
        let bytes = count
            .checked_mul(size_of::<Option<Origin>>())
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

    pub(crate) fn push(
        &mut self,
        source: Option<u32>,
        instances: &crate::instances::Prepared<'_>,
    ) -> Result<(), String> {
        if self.entries.len() == self.entries.capacity() {
            return Err("Compiled light origin count exceeds the admitted native table".into());
        }
        let origin = if let Some(entity) = source {
            if let Some((instance, file, original)) = instances.entity_origin(entity) {
                self.bytes = self
                    .bytes
                    .checked_add(file.len())
                    .filter(|&bytes| bytes <= MAX_CHANNEL_BYTES)
                    .ok_or("Compiled light origins exceed their channel storage budget")?;
                let mut path = String::new();
                path.try_reserve_exact(file.len())
                    .map_err(|_| "Compiled light source path cannot be allocated")?;
                path.push_str(file);
                Some(Origin::Instance {
                    instance,
                    file: path,
                    entity: original,
                    expanded_entity: entity,
                })
            } else {
                Some(Origin::Root { entity })
            }
        } else {
            None
        };
        self.entries.push(origin);
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
    ) -> Result<&[Option<Origin>], &'static str> {
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
