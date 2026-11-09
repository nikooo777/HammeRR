use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::terrain_snapshot::process::{Mapping, mappings};
use crate::util::{invalid, read};

pub(super) struct Memory {
    file: File,
    pub maps: Vec<Mapping>,
}

impl Memory {
    pub fn open() -> io::Result<Self> {
        let bytes = read(Path::new("/proc/self/maps"), 4 * 1024 * 1024)?;
        let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
        Ok(Self {
            file: File::open("/proc/self/mem")?,
            maps: mappings(text)?,
        })
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        self.maps = Self::open()?.maps;
        Ok(())
    }

    pub fn range(&self, address: u64, length: usize) -> io::Result<()> {
        if address == 0 || length == 0 || length > 1024 * 1024 {
            return Err(invalid("Observer native memory range is invalid"));
        }
        let end = address
            .checked_add(length as u64)
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| invalid("Observer native memory range overflows"))?;
        let mut cursor = address;
        for map in &self.maps {
            if map.end <= cursor {
                continue;
            }
            if map.start > cursor || !map.readable || !map.private {
                break;
            }
            cursor = map.end.min(end);
            if cursor == end {
                return Ok(());
            }
        }
        Err(invalid(
            "Observer native memory range is not mapped readable/private",
        ))
    }

    pub fn bytes(&self, address: u64, length: usize) -> io::Result<Vec<u8>> {
        self.range(address, length)?;
        let mut bytes = vec![0; length];
        self.file.read_exact_at(&mut bytes, address)?;
        Ok(bytes)
    }

    fn array<const N: usize>(&self, address: u64) -> io::Result<[u8; N]> {
        self.range(address, N)?;
        let mut bytes = [0; N];
        self.file.read_exact_at(&mut bytes, address)?;
        Ok(bytes)
    }

    pub fn u8(&self, address: u64) -> io::Result<u8> {
        Ok(self.bytes(address, 1)?[0])
    }
    pub fn u32(&self, address: u64) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.array(address)?))
    }
    pub fn i32(&self, address: u64) -> io::Result<i32> {
        Ok(self.u32(address)? as i32)
    }
    pub fn u64(&self, address: u64) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.array(address)?))
    }
    pub fn f32(&self, address: u64) -> io::Result<f32> {
        let value = f32::from_bits(self.u32(address)?);
        if !value.is_finite() {
            return Err(invalid("Observer native value is nonfinite"));
        }
        Ok(value)
    }
    pub fn vector(&self, address: u64) -> io::Result<[f32; 3]> {
        Ok([
            self.f32(address)?,
            self.f32(address + 4)?,
            self.f32(address + 8)?,
        ])
    }
    pub fn string(&self, address: u64, maximum: usize) -> io::Result<String> {
        if !(1..=4096).contains(&maximum) {
            return Err(invalid("Observer string bound invalid"));
        }
        let mut bytes = Vec::new();
        for offset in 0..maximum {
            let byte = self.u8(address + offset as u64)?;
            if byte == 0 {
                return String::from_utf8(bytes).map_err(io::Error::other);
            }
            bytes.push(byte);
        }
        Err(invalid("Observer native string has no bounded terminator"))
    }
    pub fn function(&self, address: u64, expected: u64) -> io::Result<usize> {
        if address != expected
            || !self.maps.iter().any(|map| {
                map.start <= address
                    && address < map.end
                    && map.executable
                    && !map.writable
                    && map.private
            })
        {
            return Err(invalid("Observer native function identity differs"));
        }
        usize::try_from(address).map_err(io::Error::other)
    }
}
