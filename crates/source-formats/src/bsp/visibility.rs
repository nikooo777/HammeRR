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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(clusters: usize, payload: &[u8]) -> Vec<u8> {
        let header = 4 + clusters * 8;
        let mut data = Vec::new();
        data.extend_from_slice(&(clusters as i32).to_le_bytes());
        for _ in 0..clusters * 2 {
            data.extend_from_slice(&(header as i32).to_le_bytes());
        }
        data.extend_from_slice(payload);
        data
    }

    #[test]
    fn decodes_shared_rows_with_bounded_zero_runs() {
        let data = fixture(24, &[1, 0, 2]);
        let visibility = Visibility::parse(&data).unwrap();
        assert_eq!(visibility.clusters(), 24);
        for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
            assert_eq!(visibility.row(0, kind).unwrap(), vec![1, 0, 0]);
            assert_eq!(visibility.row(23, kind).unwrap(), vec![1, 0, 0]);
        }
        assert!(visibility.row(24, VisibilityKind::Visible).is_err());
    }

    #[test]
    fn rejects_malformed_headers_offsets_and_compressed_rows() {
        for data in [
            vec![],
            vec![1, 0, 0],
            (-1_i32).to_le_bytes().to_vec(),
            (65_537_i32).to_le_bytes().to_vec(),
            vec![1, 0, 0, 0],
        ] {
            assert!(Visibility::parse(&data).is_err());
        }
        for offset in [-2_i32, 0, 4, 13, i32::MAX] {
            let mut data = fixture(1, &[1]);
            data[4..8].copy_from_slice(&offset.to_le_bytes());
            assert!(Visibility::parse(&data).is_err());
        }
        for payload in [vec![0], vec![0, 0], vec![0, 2], vec![0xff]] {
            let data = fixture(1, &payload);
            let visibility = Visibility::parse(&data).unwrap();
            assert!(visibility.row(0, VisibilityKind::Visible).is_err());
        }
        let mut data = fixture(1, &[1]);
        data[4..8].copy_from_slice(&(-1_i32).to_le_bytes());
        assert!(
            Visibility::parse(&data)
                .unwrap()
                .row(0, VisibilityKind::Visible)
                .is_err()
        );
    }

    #[test]
    fn bsp_without_visibility_reports_none() {
        assert!(Bsp::new().visibility().unwrap().is_none());
        let mut bsp = Bsp::new();
        bsp.lumps[lump::VISIBILITY].data = fixture(1, &[1]);
        bsp.lumps[lump::VISIBILITY].version = 1;
        assert!(bsp.visibility().is_err());
    }
}
