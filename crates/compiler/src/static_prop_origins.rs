use sha2::{Digest, Sha256};
use source_formats::bsp::{self, Bsp, GameLump};

use crate::{Control, entity_origins::Origin, instances::Prepared};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct StaticPropOrigins {
    entries: Vec<Origin>,
    digest: [u8; 32],
    version: u16,
}

fn table(bsp: &Bsp) -> Result<&GameLump, &'static str> {
    let mut tables = bsp.game_lumps.iter().filter(|table| table.id == *b"sprp");
    let table = tables
        .next()
        .ok_or("Native static-prop table is unavailable")?;
    if tables.next().is_some() || table.flags != 0 {
        return Err("Native static-prop table is duplicated or compressed");
    }
    Ok(table)
}

impl StaticPropOrigins {
    pub(crate) fn prepare(
        sources: &[usize],
        instances: &Prepared<'_>,
        control: &Control,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        let mut bytes = sources
            .len()
            .checked_mul(size_of::<Origin>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .and_then(|bytes| bytes.checked_add(2 * size_of::<usize>()))
            .and_then(|bytes| bytes.checked_add(instances.occurrence_bytes()))
            .filter(|bytes| sources.len() <= bsp::MAX_STATIC_PROPS && *bytes <= MAX_BYTES)
            .ok_or("Compiled prop origins exceed their storage budget")?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(sources.len())
            .map_err(|_| "Compiled prop origins cannot be allocated")?;
        for &source in sources {
            control.checkpoint()?;
            let entity =
                u32::try_from(source).map_err(|_| "Compiled prop source entity ID is invalid")?;
            entries.push(Origin::capture(entity, instances, &mut bytes, MAX_BYTES)?);
        }
        Ok(Self {
            entries,
            digest: [0; 32],
            version: 0,
        })
    }

    pub(crate) fn finish(mut self, bsp: &Bsp, control: &Control) -> Result<Self, String> {
        control.checkpoint()?;
        let props = bsp
            .static_props()
            .map_err(|error| error.to_string())?
            .ok_or("Native static-prop table is unavailable")?;
        if self.entries.len() != props.props.len() {
            return Err("Compiled prop source count differs from the native table".into());
        }
        let table = table(bsp)?;
        self.version = table.version;
        self.digest = Sha256::digest(&table.data).into();
        control.checkpoint()?;
        Ok(self)
    }

    pub fn associations(&self, bsp: &Bsp, count: usize) -> Result<&[Origin], &'static str> {
        let table = table(bsp)?;
        if self.entries.len() != count {
            return Err("Prop source association count differs from the native table");
        }
        let digest: [u8; 32] = Sha256::digest(&table.data).into();
        if self.version != table.version || self.digest != digest {
            return Err("Prop source association seal differs from the native table");
        }
        Ok(&self.entries)
    }
}
