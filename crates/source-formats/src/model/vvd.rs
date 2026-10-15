use std::io;

use super::{MAX_VERTICES, Vertex, invalid, reader::Reader};

pub fn parse(data: &[u8], checksum: i32) -> io::Result<Vec<Vertex>> {
    let reader = Reader::new(data, 64)?;
    if reader.bytes(0, 4)? != b"IDSV" || reader.int(4)? != 4 || reader.int(8)? != checksum {
        return Err(invalid(
            "VVD magic, version or model checksum does not match",
        ));
    }
    let lods = reader.count(12, 8)?;
    if lods == 0 {
        return Err(invalid("VVD requires at least one LOD"));
    }
    let count = reader.count(16, MAX_VERTICES)?;
    if count == 0 {
        return Err(invalid("VVD has no vertices"));
    }
    let mut previous = count;
    for lod in 0..lods {
        let value = reader.count(16 + lod * 4, MAX_VERTICES)?;
        if value > previous {
            return Err(invalid("VVD LOD vertex counts must decrease"));
        }
        previous = value;
    }
    let start = reader.count(56, data.len())?;
    reader.bytes(start, count * 48)?;
    let fixups = reader.table(0, 48, 52, 12, MAX_VERTICES)?;
    let mut order = Vec::with_capacity(count);
    if fixups.len == 0 {
        order.extend(0..count);
    } else {
        for index in 0..fixups.len {
            let fixup = fixups.at(index)?;
            let lod = reader.count(fixup, 7)?;
            if lod >= lods {
                return Err(invalid("VVD fixup references an unavailable LOD"));
            }
            let source = reader.count(fixup + 4, count)?;
            let size = reader.count(fixup + 8, count)?;
            let end = source
                .checked_add(size)
                .filter(|&end| end <= count)
                .ok_or_else(|| invalid("VVD fixup exceeds the vertex stream"))?;
            if order.len() + size > count {
                return Err(invalid("VVD fixups exceed the root vertex count"));
            }
            order.extend(source..end);
        }
        if order.len() != count {
            return Err(invalid("VVD fixups do not cover the root vertex count"));
        }
    }
    order
        .into_iter()
        .map(|index| {
            let offset = start + index * 48;
            let position = reader.vector(offset + 16)?;
            if position.iter().any(|value| value.abs() > 65536.0) {
                return Err(invalid("model vertices must fit ±65536 units"));
            }
            let normal = reader.vector(offset + 28)?;
            let length = normal.iter().map(|value| value * value).sum::<f32>();
            if !length.is_finite() || length < 1e-12 {
                return Err(invalid("model vertex normals must be nonzero"));
            }
            Ok(Vertex {
                position,
                normal,
                uv: [reader.float(offset + 40)?, reader.float(offset + 44)?],
            })
        })
        .collect()
}
