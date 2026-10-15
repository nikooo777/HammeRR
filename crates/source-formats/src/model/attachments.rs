use std::io;

use glam::DAffine3;

use super::{Attachment, invalid, reader::Reader};

pub const MAX_ATTACHMENTS: usize = 1024;
pub const WORLD_ALIGN: u32 = 0x10000;

pub fn parse(reader: &Reader<'_>, bones: &[DAffine3]) -> io::Result<Vec<Attachment>> {
    let table = reader.table(0, 240, 244, 92, MAX_ATTACHMENTS)?;
    let mut result = Vec::with_capacity(table.len);
    for index in 0..table.len {
        let base = table.at(index)?;
        let name = reader.string(reader.relative(base, 0)?)?;
        if name.is_empty() {
            return Err(invalid("MDL attachment name cannot be empty"));
        }
        let bone = reader.count(base + 8, bones.len())?;
        let pose = bones
            .get(bone)
            .ok_or_else(|| invalid("MDL attachment bone is outside its hierarchy"))?;
        let transform = *pose * reader.affine(base + 12)?;
        let flags = reader.int(base + 4)? as u32;
        if !transform.is_finite()
            || transform.translation.abs().max_element() > 65536.0
            || transform
                .matrix3
                .to_cols_array()
                .iter()
                .any(|value| value.abs() > 65536.0)
            || (flags & WORLD_ALIGN == 0 && transform.matrix3.determinant().abs() < 1e-9)
        {
            return Err(invalid(
                "MDL reference attachment transform exceeds its bounds",
            ));
        }
        result.push(Attachment {
            name,
            bone,
            flags,
            transform,
        });
    }
    Ok(result)
}
