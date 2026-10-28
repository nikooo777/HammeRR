use std::io;

use super::{Bsp, invalid, lump};

pub const MAX_VISIBILITY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_VISIBILITY_CLUSTERS: usize = 65_536;

#[derive(Debug, Clone, Copy)]
pub enum VisibilityKind {
    Visible,
    Audible,
}

#[derive(Debug, Clone, Copy)]
pub struct Visibility<'a> {
    data: &'a [u8],
    clusters: usize,
    header: usize,
}

impl Bsp {
    pub fn visibility(&self) -> io::Result<Option<Visibility<'_>>> {
        let data = &self.lumps[lump::VISIBILITY].data;
        if data.is_empty() {
            return Ok(None);
        }
        if self.lumps[lump::VISIBILITY].version != 0 {
            return Err(invalid("unsupported visibility lump version"));
        }
        Visibility::parse(data).map(Some)
    }
}

impl<'a> Visibility<'a> {
    pub fn clusters(&self) -> usize {
        self.clusters
    }

    pub fn parse(data: &'a [u8]) -> io::Result<Self> {
        let count = data
            .get(..4)
            .ok_or_else(|| invalid("visibility header is truncated"))?;
        let count = i32::from_le_bytes(count.try_into().unwrap());
        if count < 0 || count as usize > MAX_VISIBILITY_CLUSTERS {
            return Err(invalid("visibility cluster count exceeds Source limits"));
        }
        let clusters = count as usize;
        let header = 4 + clusters * 8;
        let table = data
            .get(4..header)
            .ok_or_else(|| invalid("visibility offset table is truncated"))?;
        for offset in table.as_chunks::<4>().0 {
            let offset = i32::from_le_bytes(*offset);
            if offset != -1 && (offset < header as i32 || offset as usize >= data.len()) {
                return Err(invalid("visibility row offset is outside the payload"));
            }
        }
        Ok(Self {
            data,
            clusters,
            header,
        })
    }

    pub fn row(&self, cluster: usize, kind: VisibilityKind) -> io::Result<Vec<u8>> {
        if cluster >= self.clusters {
            return Err(invalid("visibility cluster index is out of range"));
        }
        let kind = match kind {
            VisibilityKind::Visible => 0,
            VisibilityKind::Audible => 1,
        };
        let table = 4 + cluster * 8 + kind * 4;
        let offset = i32::from_le_bytes(self.data[table..table + 4].try_into().unwrap());
        if offset < self.header as i32 {
            return Err(invalid("visibility row is missing"));
        }
        let mut cursor = offset as usize;
        let width = self.clusters.div_ceil(8);
        let mut row = Vec::with_capacity(width);
        while row.len() < width {
            let value = *self
                .data
                .get(cursor)
                .ok_or_else(|| invalid("visibility row is truncated"))?;
            cursor += 1;
            if value != 0 {
                row.push(value);
                continue;
            }
            let count = *self
                .data
                .get(cursor)
                .ok_or_else(|| invalid("visibility zero run is truncated"))?
                as usize;
            cursor += 1;
            if count == 0 || count > width - row.len() {
                return Err(invalid("visibility zero run exceeds its row"));
            }
            row.resize(row.len() + count, 0);
        }
        if !self.clusters.is_multiple_of(8) && row[width - 1] >> (self.clusters % 8) != 0 {
            return Err(invalid("visibility padding bits are set"));
        }
        Ok(row)
    }
}
