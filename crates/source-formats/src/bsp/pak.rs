//! Bounded ZIP32 PAK files: STORE/DEFLATE input and deterministic STORE output.
//! Asset names and decoded bytes are retained; archive metadata is canonicalized.
//! Controlled operations return callback failures without publishing files.

use std::cell::RefCell;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use zip::read::read_zipfile_from_stream_with_options;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipReadOptions, ZipWriter};

use super::{Bsp, Lump, invalid, lump};

mod bsp;
mod deflate;
mod headers;
pub use bsp::{replace_bsp_pak, replace_bsp_pak_controlled};

const CHUNK_BYTES: usize = 32 * 1024;

/// HammeRR resource budgets, below the ZIP32 and signed BSP field limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PakLimits {
    /// Encoded ZIP bytes, including headers and comments.
    pub max_archive_bytes: usize,
    pub max_entries: usize,
    pub max_entry_bytes: usize,
    /// Sum of decoded file bytes.
    pub max_total_bytes: usize,
    pub max_name_bytes: usize,
    /// Entire BSP, including old unreachable PAK bytes retained by append-only replacement.
    pub max_bsp_bytes: usize,
}

impl Default for PakLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 64 * 1024 * 1024,
            max_entries: 4096,
            max_entry_bytes: 16 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_name_bytes: 259,
            max_bsp_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PakEntry {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PakArchive {
    pub entries: Vec<PakEntry>,
}

impl PakLimits {
    fn validate(self) -> io::Result<()> {
        if self.max_archive_bytes >= u32::MAX as usize
            || self.max_entry_bytes >= u32::MAX as usize
            || self.max_total_bytes >= u32::MAX as usize
            || self.max_entries >= u16::MAX as usize
            || self.max_name_bytes > u16::MAX as usize
            || self.max_bsp_bytes > i32::MAX as usize
        {
            return Err(invalid("PAK limits exceed ZIP32 or signed BSP field sizes"));
        }
        Ok(())
    }
}

impl PakArchive {
    pub fn parse(data: &[u8], limits: PakLimits) -> io::Result<Self> {
        Self::parse_controlled(data, limits, &mut || Ok(()))
    }

    pub fn parse_controlled(
        data: &[u8],
        limits: PakLimits,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        let directory = headers::preflight(data, limits, check)?;
        controlled(check, |checkpoint| {
            let mut entries = Vec::new();
            reserve(&mut entries, directory.entries.len())?;
            let mut buffer = [0; CHUNK_BYTES];
            for expected in directory.entries {
                checkpoint()?;
                if expected.method == 8 {
                    deflate::validate(
                        &data[expected.data_start..expected.data_start + expected.compressed_size],
                        expected.size,
                        checkpoint,
                    )?;
                }
                let mut reader = CheckedIo {
                    inner: &data[expected.header_start..expected.local_end],
                    check: checkpoint,
                };
                let options = ZipReadOptions::default()
                    .override_compressed_size(expected.compressed_size as u64)
                    .override_uncompressed_size(expected.size as u64)
                    .override_crc(expected.crc32);
                // A checked entry slice prevents alternate-footer scanning and directory allocation.
                let mut file = read_zipfile_from_stream_with_options(&mut reader, options)
                    .map_err(io::Error::from)?
                    .ok_or_else(|| invalid("PAK ZIP local entry is missing"))?;
                let method = if expected.method == 0 {
                    CompressionMethod::Stored
                } else {
                    CompressionMethod::Deflated
                };
                if file.name_raw() != expected.name.as_bytes()
                    || file.size() != expected.size as u64
                    || file.compressed_size() != expected.compressed_size as u64
                    || file.crc32() != expected.crc32
                    || file.compression() != method
                    || file.encrypted()
                {
                    return Err(invalid("PAK ZIP entry differs from its checked headers"));
                }
                let mut bytes = Vec::new();
                reserve(&mut bytes, expected.size)?;
                let mut crc = crc32fast::Hasher::new();
                loop {
                    checkpoint()?;
                    let remaining = expected.size - bytes.len();
                    let read_limit = buffer.len().min(remaining.saturating_add(1));
                    let count = file.read(&mut buffer[..read_limit])?;
                    if count == 0 {
                        break;
                    }
                    if count > remaining {
                        return Err(invalid(
                            "PAK entry exceeds its advertised decompressed size",
                        ));
                    }
                    crc.update(&buffer[..count]);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                if bytes.len() != expected.size || crc.finalize() != expected.crc32 {
                    return Err(invalid("PAK entry decompressed length or CRC32 is invalid"));
                }
                entries.push(PakEntry {
                    name: expected.name.to_owned(),
                    data: bytes,
                });
            }
            checkpoint()?;
            Ok(Self { entries })
        })
    }

    pub fn to_bytes(&self, limits: PakLimits) -> io::Result<Vec<u8>> {
        self.to_bytes_controlled(limits, &mut || Ok(()))
    }

    pub fn to_bytes_controlled(
        &self,
        limits: PakLimits,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Vec<u8>> {
        let (entries, encoded_size) = headers::writer_preflight(&self.entries, limits, check)?;
        let mut bytes = Vec::new();
        reserve(&mut bytes, encoded_size)?;
        controlled(check, |checkpoint| {
            let writer = CheckedIo {
                inner: BoundedWriter {
                    inner: Cursor::new(bytes),
                    limit: encoded_size,
                },
                check: checkpoint,
            };
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .system(System::Unix)
                .last_modified_time(DateTime::default())
                .unix_permissions(0o644)
                .large_file(false);
            let mut archive = ZipWriter::new(writer);
            for entry in entries {
                checkpoint()?;
                archive
                    .start_file(&entry.name, options)
                    .map_err(io::Error::from)?;
                for chunk in entry.data.chunks(CHUNK_BYTES) {
                    checkpoint()?;
                    archive.write_all(chunk)?;
                }
            }
            checkpoint()?;
            let writer = archive.finish().map_err(io::Error::from)?;
            let bytes = writer.inner.inner.into_inner();
            if bytes.len() != encoded_size {
                return Err(invalid("PAK stored ZIP size differs from its preflight"));
            }
            checkpoint()?;
            Ok(bytes)
        })
    }
}

impl Bsp {
    pub fn pak_data(&self, limits: PakLimits) -> io::Result<PakArchive> {
        self.pak_data_controlled(limits, &mut || Ok(()))
    }

    pub fn pak_data_controlled(
        &self,
        limits: PakLimits,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<PakArchive> {
        check()?;
        let lump = self
            .lumps
            .get(lump::PAKFILE)
            .ok_or_else(|| invalid("BSP has no PAK lump slot"))?;
        if lump.version != 0 || lump.fourcc != [0; 4] {
            return Err(invalid("BSP PAK lump requires uncompressed version 0"));
        }
        if lump.data.is_empty() {
            limits.validate()?;
            return Ok(PakArchive::default());
        }
        PakArchive::parse_controlled(&lump.data, limits, check)
    }

    pub fn set_pak_data(&mut self, archive: &PakArchive, limits: PakLimits) -> io::Result<()> {
        self.set_pak_data_controlled(archive, limits, &mut || Ok(()))
    }

    pub fn set_pak_data_controlled(
        &mut self,
        archive: &PakArchive,
        limits: PakLimits,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        if self.lumps.len() != super::LUMP_COUNT {
            return Err(invalid("BSP must have exactly 64 lump slots"));
        }
        let data = archive.to_bytes_controlled(limits, check)?;
        check()?;
        self.lumps[lump::PAKFILE] = Lump {
            data,
            ..Lump::default()
        };
        Ok(())
    }
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> io::Result<()> {
    values
        .try_reserve_exact(count)
        .map_err(|error| io::Error::other(format!("PAK allocation failed: {error}")))
}

fn controlled<T>(
    check: &mut dyn FnMut() -> io::Result<()>,
    operation: impl FnOnce(&dyn Fn() -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    let callback = RefCell::new(check);
    let failure = RefCell::new(None);
    let checkpoint = || {
        if failure.borrow().is_none() {
            if let Err(error) = (*callback.borrow_mut())() {
                *failure.borrow_mut() = Some(error);
            }
        }
        if failure.borrow().is_some() {
            // read_exact/write_all retry Interrupted; restore the original error outside ZIP I/O.
            Err(io::Error::other(
                "PAK control callback stopped the operation",
            ))
        } else {
            Ok(())
        }
    };
    let result = operation(&checkpoint);
    match failure.into_inner() {
        Some(error) => Err(error),
        None => result,
    }
}

struct CheckedIo<'a, T> {
    inner: T,
    check: &'a dyn Fn() -> io::Result<()>,
}

impl<T: Read> Read for CheckedIo<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (self.check)()?;
        self.inner.read(buffer)
    }
}

impl<T: Write> Write for CheckedIo<'_, T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        (self.check)()?;
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        (self.check)()?;
        self.inner.flush()
    }
}

impl<T: Seek> Seek for CheckedIo<'_, T> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        (self.check)()?;
        self.inner.seek(position)
    }
}

struct BoundedWriter {
    inner: Cursor<Vec<u8>>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let end = self
            .inner
            .position()
            .checked_add(buffer.len() as u64)
            .ok_or_else(|| invalid("PAK ZIP output offset overflow"))?;
        if end > self.limit as u64 {
            return Err(invalid("PAK ZIP writer exceeded its checked output size"));
        }
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for BoundedWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let offset = self.inner.seek(position)?;
        if offset > self.limit as u64 {
            return Err(invalid(
                "PAK ZIP writer sought beyond its checked output size",
            ));
        }
        Ok(offset)
    }
}
