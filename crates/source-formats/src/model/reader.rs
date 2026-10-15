use std::io;

use super::invalid;

pub struct Reader<'a> {
    data: &'a [u8],
}

pub struct Table {
    pub start: usize,
    pub len: usize,
    stride: usize,
}

impl Table {
    pub fn at(&self, index: usize) -> io::Result<usize> {
        if index >= self.len {
            return Err(invalid("model table index is out of range"));
        }
        Ok(self.start + index * self.stride)
    }
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], minimum: usize) -> io::Result<Self> {
        if data.len() < minimum || data.len() > crate::mdl::MAX_MODEL_BYTES {
            return Err(invalid("model companion file size is out of bounds"));
        }
        Ok(Self { data })
    }

    pub fn bytes(&self, start: usize, size: usize) -> io::Result<&'a [u8]> {
        let end = start
            .checked_add(size)
            .ok_or_else(|| invalid("model range overflow"))?;
        self.data
            .get(start..end)
            .ok_or_else(|| invalid("model range exceeds the file"))
    }

    pub fn int(&self, offset: usize) -> io::Result<i32> {
        Ok(i32::from_le_bytes(
            self.bytes(offset, 4)?.try_into().unwrap(),
        ))
    }

    pub fn short(&self, offset: usize) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(offset, 2)?.try_into().unwrap(),
        ))
    }

    pub fn byte(&self, offset: usize) -> io::Result<u8> {
        Ok(self.bytes(offset, 1)?[0])
    }

    pub fn count(&self, offset: usize, maximum: usize) -> io::Result<usize> {
        usize::try_from(self.int(offset)?)
            .ok()
            .filter(|&count| count <= maximum)
            .ok_or_else(|| invalid("model count is negative or exceeds its resource limit"))
    }

    pub fn relative(&self, base: usize, field: usize) -> io::Result<usize> {
        base.checked_add_signed(self.int(base + field)? as isize)
            .ok_or_else(|| invalid("model relative offset is out of range"))
    }

    pub fn table(
        &self,
        base: usize,
        count: usize,
        offset: usize,
        stride: usize,
        maximum: usize,
    ) -> io::Result<Table> {
        let len = self.count(base + count, maximum)?;
        if len == 0 {
            return Ok(Table {
                start: 0,
                len: 0,
                stride,
            });
        }
        let start = self.relative(base, offset)?;
        self.bytes(
            start,
            len.checked_mul(stride)
                .ok_or_else(|| invalid("model table size overflow"))?,
        )?;
        Ok(Table { start, len, stride })
    }

    pub fn float(&self, offset: usize) -> io::Result<f32> {
        let value = f32::from_le_bytes(self.bytes(offset, 4)?.try_into().unwrap());
        if !value.is_finite() {
            return Err(invalid("model vertex values must be finite"));
        }
        Ok(value)
    }

    pub fn vector(&self, offset: usize) -> io::Result<[f32; 3]> {
        Ok([
            self.float(offset)?,
            self.float(offset + 4)?,
            self.float(offset + 8)?,
        ])
    }

    pub fn affine(&self, offset: usize) -> io::Result<glam::DAffine3> {
        let mut rows = [[0.0; 4]; 3];
        for (row, values) in rows.iter_mut().enumerate() {
            for (column, value) in values.iter_mut().enumerate() {
                *value = f64::from(self.float(offset + row * 16 + column * 4)?);
            }
        }
        Ok(glam::DAffine3::from_mat3_translation(
            glam::DMat3::from_cols(
                glam::DVec3::new(rows[0][0], rows[1][0], rows[2][0]),
                glam::DVec3::new(rows[0][1], rows[1][1], rows[2][1]),
                glam::DVec3::new(rows[0][2], rows[1][2], rows[2][2]),
            ),
            glam::DVec3::new(rows[0][3], rows[1][3], rows[2][3]),
        ))
    }

    pub fn string(&self, start: usize) -> io::Result<String> {
        let available = self.data.len().saturating_sub(start).min(1025);
        let bytes = self.bytes(start, available)?;
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| invalid("model string is missing its bounded terminator"))?;
        std::str::from_utf8(&bytes[..end])
            .map(str::to_owned)
            .map_err(|_| invalid("model strings must be UTF-8"))
    }
}
