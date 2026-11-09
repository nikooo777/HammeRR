use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source_formats::bsp::LightingKind;
use source_formats::bsp::native_terrain::{
    NATIVE_TERRAIN_VERTEX_STRIDE, NativeTerrainCarrier, NativeTerrainReadback,
    NativeTerrainSnapshot,
};
use source_formats::file_publication::FileIdentity;
use source_formats::vpk::Vpk;

use crate::process::WorkerRequest;
use crate::util::{self, invalid};
use crate::{
    CaptureChannel, CaptureEnvironment, CaptureFailure, CleanupDisposition,
    TerrainObservationReceipt,
};

pub(crate) mod process;
mod readiness;

pub(crate) const PROFILE: &str = "css_linux64_build25338318_observed_terrain_v1";
const READBACK_FILE: &str = "terrain-readback.bin";
const OBSERVATION_FILE: &str = "terrain-observation.json";
const READBACK_MAGIC: &[u8; 8] = b"HMTRN001";
const MAX_READBACK_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_ENTRIES: usize = 2_000_000;
const MAX_PROVIDER_ARCHIVES: usize = 512;
const MAX_PROVIDER_DIRECTORY_ENTRIES: usize = 32768;
const VPK_SIGNATURE: u32 = 0x55aa_1234;
const MAX_ELF_SEGMENTS: usize = 32;
const ELF_HEADER_BYTES: usize = 64;
const ELF_PROGRAM_HEADER_BYTES: usize = 56;
const ELF_LOAD: u32 = 1;
const ELF_EXECUTABLE: u32 = 1;
const ELF_WRITABLE: u32 = 2;
const ELF_READABLE: u32 = 4;
const IMAGE_READ_CHUNK_BYTES: usize = 1024 * 1024;
const WORLD_SURFACES: u64 = 0xe8;
const SURFACE_STRIDE: usize = 0x40;
const SURFACE_DISPLACEMENT: u64 = 0x1c;
const DISP_VERTICES: usize = 0x178;
const DISP_VERTEX_COUNT: usize = 0x188;
const DISP_VERTEX_MIRROR: usize = 0x190;
const DISP_PARENT: usize = 0x1f0;
const DISP_POWER: usize = 0x20c;
const DISP_POWER_INFO: usize = 0x268;
const DISP_ORDINAL: usize = 0x302;
const DISP_HANDLE: usize = 0x308;
const POWER_VERTEX_COUNT: u64 = 0x48;
const MAX_VERTICES: usize = 289;
const PREMAP: &str = "HAMMERR_TERRAIN_PREMAP";
const POSTMAP: &str = "HAMMERR_TERRAIN_POSTMAP";
const CONSOLE_FILE: &str = "hammerr-terrain.log";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TerrainSnapshotProfile {
    pub map_name: String,
    pub bsp_sha256: String,
    pub channel: CaptureChannel,
}

impl TerrainSnapshotProfile {
    pub(crate) fn new(map_name: String, bsp: &[u8], channel: CaptureChannel) -> io::Result<Self> {
        if bsp.len() > util::MAX_BSP {
            return Err(invalid("Native terrain BSP exceeds 128 MiB"));
        }
        let profile = Self {
            map_name,
            bsp_sha256: util::hash(bsp),
            channel,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        util::validate_name(&self.map_name)?;
        if !self.map_name.starts_with("ht_")
            || self.bsp_sha256.len() != 64
            || !self.bsp_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("Invalid private native terrain snapshot profile"));
        }
        Ok(())
    }
}

pub(crate) struct PinnedFile {
    file: File,
    path: PathBuf,
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
    sha256: String,
}

impl PinnedFile {
    pub(crate) fn open(path: &Path, expected: &str) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > util::MAX_BSP as u64 {
            return Err(invalid(
                "Native terrain input file is not a bounded regular file",
            ));
        }
        let input = Self {
            identity: FileIdentity::from_file(&file)?,
            length: metadata.len(),
            modified: metadata.modified()?,
            sha256: expected.to_owned(),
            path,
            file,
        };
        input.verify()?;
        Ok(input)
    }

    pub(crate) fn verify(&self) -> io::Result<()> {
        let current = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)?;
        let metadata = current.metadata()?;
        if !metadata.is_file()
            || FileIdentity::from_file(&current)? != self.identity
            || metadata.len() != self.length
            || metadata.modified()? != self.modified
            || hash_opened(&self.file, self.length, &mut || Ok(()))? != self.sha256
            || self.file.metadata()?.modified()? != self.modified
            || FileIdentity::from_file(&File::open(&self.path)?)? != self.identity
        {
            return Err(invalid(format!(
                "Native terrain input changed: {}",
                self.path.display()
            )));
        }
        Ok(())
    }

    pub(crate) fn receipt(&self) -> Value {
        json!({"path":self.path,"device":self.identity.device,"inode":self.identity.inode,"length":self.length,"sha256":self.sha256})
    }
}

fn hash_opened(
    file: &File,
    length: u64,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0;
    while offset < length {
        check()?;
        let remaining = (length - offset).min(buffer.len() as u64) as usize;
        let count = file.read_at(&mut buffer[..remaining], offset)?;
        if count != remaining {
            return Err(invalid("Native terrain input file read was incomplete"));
        }
        digest.update(&buffer[..count]);
        offset += count as u64;
    }
    check()?;
    let mut extra = [0];
    if file.read_at(&mut extra, length)? != 0 || file.metadata()?.len() != length {
        return Err(invalid("Native terrain input grew while hashing"));
    }
    check()?;
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ImageStamp {
    identity: FileIdentity,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl ImageStamp {
    fn capture(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > util::MAX_BSP as u64 {
            return Err(invalid(
                "Native namespace image exceeds its regular-file budget",
            ));
        }
        Ok(Self {
            identity: FileIdentity::from_file(file)?,
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

struct NamespaceImage {
    file: File,
    stamp: ImageStamp,
    sha256: String,
}

impl NamespaceImage {
    fn open(
        process: &mut process::StoppedProcess<'_>,
        path: &Path,
        expected: &str,
    ) -> io::Result<Self> {
        let file = process.open_namespace_file(path)?;
        let stamp = ImageStamp::capture(&file)?;
        let image = Self {
            file,
            stamp,
            sha256: expected.to_owned(),
        };
        image.verify(process, path)?;
        Ok(image)
    }

    fn verify(&self, process: &mut process::StoppedProcess<'_>, path: &Path) -> io::Result<()> {
        process.checkpoint()?;
        let current = process.open_namespace_file(path)?;
        if ImageStamp::capture(&self.file)? != self.stamp
            || ImageStamp::capture(&current)? != self.stamp
        {
            return Err(invalid("Native namespace image identity or stamp changed"));
        }
        let actual = hash_opened(&self.file, self.stamp.length, &mut || process.checkpoint())?;
        if actual != self.sha256 {
            return Err(invalid(format!(
                "Native namespace image SHA-256 differs from the qualified image: {actual}"
            )));
        }
        if ImageStamp::capture(&self.file)? != self.stamp
            || ImageStamp::capture(&current)? != self.stamp
            || ImageStamp::capture(&process.open_namespace_file(path)?)? != self.stamp
        {
            return Err(invalid(
                "Native namespace image changed while hashing or copying",
            ));
        }
        process.checkpoint()
    }

    fn receipt(&self, path: &Path) -> Value {
        json!({"path":path,"device":self.stamp.identity.device,
            "device_major":libc::major(self.stamp.identity.device),
            "device_minor":libc::minor(self.stamp.identity.device),
            "inode":self.stamp.identity.inode,"length":self.stamp.length,
            "modified":self.stamp.modified,"changed":self.stamp.changed,"sha256":self.sha256})
    }
}

pub(crate) struct EngineBinding {
    namespace: NamespaceImage,
    pub(crate) load_bias: u64,
    vma_device_major: u32,
    vma_device_minor: u32,
    vma_inode: u64,
    mappings: usize,
    immutable_segments: usize,
    immutable_bytes: u64,
    immutable_sha256: String,
}

impl EngineBinding {
    pub(crate) fn receipt(&self, host: &PinnedFile) -> Value {
        json!({"binding":"namespace_sha256_complete_immutable_load_segments_v1",
            "host_file":host.receipt(),"namespace_file":self.namespace.receipt(&host.path),
            "load_bias":self.load_bias,"vma":{"device_major":self.vma_device_major,
                "device_minor":self.vma_device_minor,"inode":self.vma_inode,"mappings":self.mappings},
            "immutable":{"segments":self.immutable_segments,"bytes":self.immutable_bytes,
                "sha256":self.immutable_sha256,"matches_namespace_file":true}})
    }
}

struct LoadSegment {
    offset: u64,
    address: u64,
    file_bytes: u64,
    memory_bytes: u64,
    flags: u32,
}

pub(crate) struct EngineImage {
    pub(crate) input: PinnedFile,
    segments: Vec<LoadSegment>,
}

impl EngineImage {
    pub(crate) fn verify_local(&self) -> io::Result<(u64, Value)> {
        self.input.verify()?;
        let maps_text = fs::read_to_string("/proc/self/maps")?;
        if maps_text.len() > 4 * 1024 * 1024 {
            return Err(invalid("Native local image maps exceed four MiB"));
        }
        let maps = process::mappings(&maps_text)?;
        let candidates: Vec<_> = maps
            .iter()
            .filter(|mapping| mapping.path.as_deref() == Some(self.input.path.as_path()))
            .collect();
        let origins: Vec<_> = candidates
            .iter()
            .filter(|mapping| mapping.offset == 0)
            .collect();
        let [origin] = origins.as_slice() else {
            return Err(invalid(
                "Native local image has no unique offset-zero mapping",
            ));
        };
        let bias = origin.start;
        let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
            .map_err(io::Error::other)?;
        if !page.is_power_of_two() {
            return Err(invalid("Native local page size is invalid"));
        }
        for mapping in &candidates {
            if mapping.inode != self.input.identity.inode
                || mapping.device_major != origin.device_major
                || mapping.device_minor != origin.device_minor
            {
                return Err(invalid("Native local image mapping identity changed"));
            }
            self.verify_mapping(mapping, bias, page)?;
        }
        let memory = File::open("/proc/self/mem")?;
        let mut immutable_bytes = 0u64;
        for segment in self
            .segments
            .iter()
            .filter(|segment| segment.flags & ELF_WRITABLE == 0)
        {
            if segment.file_bytes != segment.memory_bytes {
                return Err(invalid("Native local immutable image has a zero-fill tail"));
            }
            let mut offset = 0;
            while offset < segment.file_bytes {
                let length =
                    (segment.file_bytes - offset).min(IMAGE_READ_CHUNK_BYTES as u64) as usize;
                let position = address(address(bias, segment.address)?, offset)?;
                let end = address(position, length as u64)?;
                let mut cursor = position;
                for mapping in &candidates {
                    if mapping.end <= cursor {
                        continue;
                    }
                    if mapping.start > cursor || !mapping.readable || mapping.writable {
                        break;
                    }
                    cursor = mapping.end.min(end);
                    if cursor == end {
                        break;
                    }
                }
                if cursor != end {
                    return Err(invalid("Native local immutable image range is unavailable"));
                }
                let expected =
                    file_range(&self.input.file, address(segment.offset, offset)?, length)?;
                if file_range(&memory, position, length)? != expected {
                    return Err(invalid("Native local immutable image bytes changed"));
                }
                immutable_bytes = address(immutable_bytes, length as u64)?;
                offset = address(offset, length as u64)?;
            }
        }
        self.input.verify()?;
        Ok((
            bias,
            json!({"file":self.input.receipt(),"load_bias":bias,"immutable_bytes":immutable_bytes,"complete_immutable_match":true}),
        ))
    }

    fn open(root: &Path) -> io::Result<Self> {
        Self::open_at(&root.join("bin/linux64/engine.so"), util::ENGINE_SHA)
    }

    pub(crate) fn open_at(path: &Path, expected: &str) -> io::Result<Self> {
        let input = PinnedFile::open(path, expected)?;
        let header = file_range(&input.file, 0, ELF_HEADER_BYTES)?;
        if &header[..4] != b"\x7fELF"
            || header[4] != 2
            || header[5] != 1
            || header[6] != 1
            || u16_at(&header, 16)? != 3
            || u16_at(&header, 18)? != 62
            || u16_at(&header, 52)? != ELF_HEADER_BYTES as u16
            || u16_at(&header, 54)? != ELF_PROGRAM_HEADER_BYTES as u16
        {
            return Err(invalid(
                "Pinned terrain engine is not the qualified Linux64 ELF image",
            ));
        }
        let count = usize::from(u16_at(&header, 56)?);
        let offset = u64_at(&header, 32)?;
        if count == 0 || count > MAX_ELF_SEGMENTS {
            return Err(invalid(
                "Pinned terrain engine program-header count is invalid",
            ));
        }
        let length = count
            .checked_mul(ELF_PROGRAM_HEADER_BYTES)
            .ok_or_else(|| invalid("Terrain engine program-header length overflow"))?;
        if address(offset, length as u64)? > input.length {
            return Err(invalid("Terrain engine program headers exceed its file"));
        }
        let headers = file_range(&input.file, offset, length)?;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(count)
            .map_err(io::Error::other)?;
        for record in headers.as_chunks::<ELF_PROGRAM_HEADER_BYTES>().0 {
            if u32_at(record, 0)? != ELF_LOAD {
                continue;
            }
            let segment = LoadSegment {
                flags: u32_at(record, 4)?,
                offset: u64_at(record, 8)?,
                address: u64_at(record, 16)?,
                file_bytes: u64_at(record, 32)?,
                memory_bytes: u64_at(record, 40)?,
            };
            if segment.file_bytes > segment.memory_bytes
                || address(segment.offset, segment.file_bytes)? > input.length
                || segment.memory_bytes == 0
                || segment.flags & ELF_READABLE == 0
            {
                return Err(invalid("Pinned terrain engine has an invalid load segment"));
            }
            segments.push(segment);
        }
        if !segments
            .iter()
            .any(|segment| segment.offset == 0 && segment.address == 0)
        {
            return Err(invalid(
                "Pinned terrain engine has no offset-zero load segment",
            ));
        }
        Ok(Self { input, segments })
    }

    pub(crate) fn verify_namespace(
        &self,
        binding: &EngineBinding,
        process: &mut process::StoppedProcess<'_>,
    ) -> io::Result<()> {
        binding.namespace.verify(process, &self.input.path)
    }

    pub(crate) fn bind(
        &self,
        process: &mut process::StoppedProcess<'_>,
    ) -> io::Result<EngineBinding> {
        let namespace = NamespaceImage::open(process, &self.input.path, &self.input.sha256)?;
        let candidates = self.candidate_summary(process, namespace.stamp.identity);
        self.bind_namespace(process, namespace)
            .map_err(|error| io::Error::new(error.kind(), format!("{error}; {candidates}")))
    }

    fn candidate_summary(
        &self,
        process: &process::StoppedProcess<'_>,
        namespace: FileIdentity,
    ) -> String {
        let mut paths = 0;
        let mut inodes = 0;
        let mut offsets = 0;
        let mut identities = BTreeSet::new();
        for mapping in process
            .maps()
            .iter()
            .filter(|mapping| mapping.path.as_deref() == Some(self.input.path.as_path()))
        {
            paths += 1;
            identities.insert((mapping.device_major, mapping.device_minor, mapping.inode));
            if mapping.inode == namespace.inode {
                inodes += 1;
                if mapping.offset == 0 {
                    offsets += 1;
                }
            }
        }
        format!(
            "Engine VMA candidates: exact_path={paths}, namespace_inode={inodes}, offset_zero={offsets}, identities={} {:?}, host={}:{}:{}, namespace={}:{}:{}",
            identities.len(),
            identities.iter().take(8).collect::<Vec<_>>(),
            libc::major(self.input.identity.device),
            libc::minor(self.input.identity.device),
            self.input.identity.inode,
            libc::major(namespace.device),
            libc::minor(namespace.device),
            namespace.inode
        )
    }

    fn bind_namespace(
        &self,
        process: &mut process::StoppedProcess<'_>,
        namespace: NamespaceImage,
    ) -> io::Result<EngineBinding> {
        let candidates: Vec<_> = process
            .maps()
            .iter()
            .filter(|mapping| mapping.path.as_deref() == Some(self.input.path.as_path()))
            .collect();
        let offsets: Vec<_> = candidates
            .iter()
            .filter(|mapping| {
                mapping.inode == namespace.stamp.identity.inode && mapping.offset == 0
            })
            .collect();
        let [origin] = offsets.as_slice() else {
            return Err(invalid(
                "Native terrain engine has no unique namespace-bound offset-zero mapping",
            ));
        };
        let bias = origin.start;
        let device_major = origin.device_major;
        let device_minor = origin.device_minor;
        let inode = origin.inode;
        if !origin.readable || !origin.private || origin.writable {
            return Err(invalid(
                "Native terrain engine ELF header mapping is invalid",
            ));
        }
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page = u64::try_from(page).map_err(io::Error::other)?;
        if !page.is_power_of_two() {
            return Err(invalid("Native terrain engine page size is invalid"));
        }
        for mapping in &candidates {
            if mapping.inode != inode
                || mapping.device_major != device_major
                || mapping.device_minor != device_minor
            {
                return Err(invalid(
                    "Native terrain engine VMAs do not share one namespace inode and VMA device",
                ));
            }
            self.verify_mapping(mapping, bias, page)?;
        }
        let mapping_count = candidates.len();
        let mapped = |mapping: &&process::Mapping| {
            mapping.inode == inode
                && mapping.device_major == device_major
                && mapping.device_minor == device_minor
                && mapping.path.as_deref() == Some(self.input.path.as_path())
        };
        for segment in &self.segments {
            let start = address(bias, segment.address)?;
            let end = address(start, segment.file_bytes)?;
            process.require_range(
                start,
                usize::try_from(segment.memory_bytes).map_err(io::Error::other)?,
                false,
            )?;
            let mut cursor = start;
            while cursor < end {
                let mapping = process
                    .maps()
                    .iter()
                    .filter(mapped)
                    .find(|mapping| mapping.start <= cursor && cursor < mapping.end)
                    .ok_or_else(|| {
                        invalid("Native terrain engine load segment is not file-bound")
                    })?;
                if address(mapping.offset, cursor - mapping.start)?
                    != address(segment.offset, cursor - start)?
                {
                    return Err(invalid(
                        "Native terrain engine file offsets disagree with its ELF segment",
                    ));
                }
                cursor = mapping.end.min(end);
            }
        }
        let mut digest = Sha256::new();
        let mut immutable_bytes = 0u64;
        let mut immutable_segments = 0usize;
        for segment in self
            .segments
            .iter()
            .filter(|segment| segment.flags & ELF_WRITABLE == 0)
        {
            if segment.memory_bytes != segment.file_bytes {
                return Err(invalid(
                    "Qualified immutable terrain engine segment has an unsupported zero-fill tail",
                ));
            }
            immutable_segments += 1;
            let mut offset = 0;
            while offset < segment.file_bytes {
                process.checkpoint()?;
                let length =
                    (segment.file_bytes - offset).min(IMAGE_READ_CHUNK_BYTES as u64) as usize;
                let expected =
                    file_range(&namespace.file, address(segment.offset, offset)?, length)?;
                let actual = process
                    .read_exact(address(address(bias, segment.address)?, offset)?, length)?;
                if actual != expected {
                    return Err(invalid(format!(
                        "Native terrain immutable engine bytes differ at ELF offset {:#x}",
                        address(segment.address, offset)?
                    )));
                }
                digest.update(&actual);
                immutable_bytes = address(immutable_bytes, length as u64)?;
                offset = address(offset, length as u64)?;
                process.checkpoint()?;
            }
        }
        if immutable_bytes == 0 {
            return Err(invalid(
                "Native terrain engine has no verified immutable image bytes",
            ));
        }
        Ok(EngineBinding {
            namespace,
            load_bias: bias,
            vma_device_major: device_major,
            vma_device_minor: device_minor,
            vma_inode: inode,
            mappings: mapping_count,
            immutable_segments,
            immutable_bytes,
            immutable_sha256: format!("{:x}", digest.finalize()),
        })
    }

    fn verify_mapping(&self, mapping: &process::Mapping, bias: u64, page: u64) -> io::Result<()> {
        if !mapping.readable
            || !mapping.private
            || !mapping.start.is_multiple_of(page)
            || !mapping.end.is_multiple_of(page)
            || !mapping.offset.is_multiple_of(page)
        {
            return Err(invalid(
                "Native terrain engine VMA range or protection is invalid",
            ));
        }
        let mut cursor = mapping.start;
        while cursor < mapping.end {
            let mut matched_end = None;
            for segment in &self.segments {
                let segment_start = address(bias, segment.address)?;
                let page_start = segment_start & !(page - 1);
                let page_end =
                    address(address(segment_start, segment.file_bytes)?, page - 1)? & !(page - 1);
                if cursor < page_start || cursor >= page_end {
                    continue;
                }
                if segment.address % page != segment.offset % page
                    || mapping.executable != (segment.flags & ELF_EXECUTABLE != 0)
                    || (mapping.writable && segment.flags & ELF_WRITABLE == 0)
                    || address(mapping.offset, cursor - mapping.start)?
                        != address(segment.offset & !(page - 1), cursor - page_start)?
                {
                    continue;
                }
                matched_end = Some(page_end.min(mapping.end));
                break;
            }
            cursor = matched_end.ok_or_else(|| invalid("Native terrain engine VMA is outside its ELF file ranges or has different offsets/permissions"))?;
        }
        Ok(())
    }
}

fn file_range(file: &File, address: u64, length: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(io::Error::other)?;
    bytes.resize(length, 0);
    if file.read_at(&mut bytes, address)? != length {
        return Err(invalid("Native terrain ELF read was incomplete"));
    }
    Ok(bytes)
}

fn lighting(channel: CaptureChannel) -> LightingKind {
    match channel {
        CaptureChannel::Ldr => LightingKind::Ldr,
        CaptureChannel::Hdr => LightingKind::Hdr,
    }
}

pub(crate) fn carrier(
    bsp: &[u8],
    channel: CaptureChannel,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<NativeTerrainCarrier> {
    let bsp = crate::owned_client::parse_bsp(bsp, check)?;
    let carrier = NativeTerrainCarrier::new_controlled(&bsp, lighting(channel), check)?;
    if carrier.displacements().is_empty() {
        return Err(invalid(
            "Native terrain snapshot requires a nonempty displacement catalog",
        ));
    }
    Ok(carrier)
}

fn is_lump_provider(path: &str, map_name: &str) -> bool {
    let path = path.to_ascii_lowercase().replace('\\', "/");
    let Some(name) = path.strip_prefix("maps/") else {
        return false;
    };
    name.ends_with(".lmp")
        && name
            .strip_prefix(map_name)
            .is_some_and(|suffix| suffix.starts_with('_') || suffix.starts_with('.'))
}

struct ProviderBudget {
    paths: usize,
    archives: usize,
    directory_entries: usize,
}

fn inspect_vpk(path: &Path, map_name: &str, budget: &mut ProviderBudget) -> io::Result<()> {
    budget.archives += 1;
    if budget.archives > MAX_PROVIDER_ARCHIVES {
        return Err(invalid(
            "Native terrain alternate-provider archive budget exceeded",
        ));
    }
    let archive = Vpk::open(path)?;
    for path in archive.paths() {
        budget.paths += 1;
        if budget.paths > MAX_PROVIDER_ENTRIES {
            return Err(invalid(
                "Native terrain alternate-provider path budget exceeded",
            ));
        }
        if is_lump_provider(path, map_name) {
            return Err(invalid(
                "Native terrain private map has an alternate VPK lump provider",
            ));
        }
    }
    archive.verify_index()
}

fn inspect_provider_directory(
    path: &Path,
    map_name: &str,
    budget: &mut ProviderBudget,
) -> io::Result<()> {
    let maps = path.join("maps");
    let entries = match fs::read_dir(&maps) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(entries) = entries {
        for entry in entries {
            budget.directory_entries += 1;
            if budget.directory_entries > MAX_PROVIDER_DIRECTORY_ENTRIES {
                return Err(invalid(
                    "Native terrain alternate-provider directory budget exceeded",
                ));
            }
            let name = entry?.file_name();
            if is_lump_provider(&format!("maps/{}", name.to_string_lossy()), map_name) {
                return Err(invalid(
                    "Native terrain private map has an alternate loose lump provider",
                ));
            }
        }
    }
    for entry in fs::read_dir(path)? {
        budget.directory_entries += 1;
        if budget.directory_entries > MAX_PROVIDER_DIRECTORY_ENTRIES {
            return Err(invalid(
                "Native terrain alternate-provider directory budget exceeded",
            ));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|name| name.strip_suffix(".vpk")) else {
            continue;
        };
        if name.rsplit_once('_').is_some_and(|(_, suffix)| {
            suffix.len() == 3 && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(entry.path())?;
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Err(invalid("Native terrain auto VPK is not a regular file"));
            }
            if metadata.len() < size_of::<u32>() as u64
                || u32_at(&file_range(&file, 0, size_of::<u32>())?, 0)? != VPK_SIGNATURE
            {
                continue;
            }
        }
        inspect_vpk(&entry.path(), map_name, budget)?;
    }
    Ok(())
}

pub(crate) fn verify_providers(
    environment: &CaptureEnvironment,
    game: &Path,
    map_name: &str,
) -> io::Result<()> {
    let mut paths = BTreeSet::from([game.canonicalize()?]);
    for mount in &environment.mounts {
        if mount
            .ids
            .iter()
            .any(|id| matches!(id.as_str(), "game" | "mod" | "platform"))
        {
            paths.insert(mount.path.canonicalize()?);
        }
    }
    let mut budget = ProviderBudget {
        paths: 0,
        archives: 0,
        directory_entries: 0,
    };
    for path in paths {
        if path.is_dir() {
            inspect_provider_directory(&path, map_name, &mut budget)?;
        } else {
            inspect_vpk(&path, map_name, &mut budget)?;
        }
    }
    Ok(())
}

pub(crate) fn configure_game(
    directory: &Path,
    game: &Path,
    profile: &TerrainSnapshotProfile,
) -> io::Result<()> {
    profile.validate()?;
    let staged = PinnedFile::open(&directory.join("staged.bsp"), &profile.bsp_sha256)?;
    let controls = format!(
        "toggle sv_lan 1\ntoggle sv_cheats 1\ntoggle bot_quota 0\ntoggle mat_hdr_level {}\nhelp sv_lan\nhelp mat_hdr_level\n",
        profile.channel.hdr_level()
    );
    let postmap = format!("{controls}echo {POSTMAP}\n");
    for name in [
        "server.cfg",
        "listenserver.cfg",
        "hammerr_capture_postmap.cfg",
    ] {
        util::atomic_write(&game.join("cfg").join(name), postmap.as_bytes())?;
    }
    let launch = format!(
        "toggle con_logfile {CONSOLE_FILE}\ntoggle servercfgfile hammerr_capture_postmap.cfg\ntoggle fps_max 60\n{controls}echo {PREMAP}\nmap {}\n",
        profile.map_name
    );
    crate::guard::reject_writes(&launch, false)?;
    util::atomic_write(
        &game.join("cfg/hammerr_private_launch.cfg"),
        launch.as_bytes(),
    )?;
    util::atomic_write(&game.join("cfg/valve.rc"), b"stuffcmds\n")?;
    util::atomic_write(&game.join("scripts/vscripts/mapspawn.nut"), b"")?;
    util::atomic_write(
        &game.join("motd.txt"),
        b"HammeRR private terrain snapshot\n",
    )?;
    let destination = game.join("maps").join(format!("{}.bsp", profile.map_name));
    fs::copy(&staged.path, &destination)?;
    staged.verify()?;
    PinnedFile::open(&destination, &profile.bsp_sha256)?;
    Ok(())
}

fn checkpoint(directory: &Path, request: &WorkerRequest) -> io::Result<()> {
    if crate::worker::cancelled(directory, request) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Native terrain snapshot cancelled or caller exited",
        ));
    }
    Ok(())
}

pub(crate) fn verify_controls(text: &str, channel: CaptureChannel) -> io::Result<bool> {
    let Some(premap) = text.find(PREMAP) else {
        return Ok(false);
    };
    let Some(postmap) = text.find(POSTMAP) else {
        return Ok(false);
    };
    if postmap <= premap {
        return Err(invalid(
            "Native terrain controls were not applied before map load",
        ));
    }
    for (name, expected) in [
        ("sv_lan", 1.0),
        ("mat_hdr_level", f64::from(channel.hdr_level())),
    ] {
        let pattern = Regex::new(&format!(r#""{name}" = "([^"]+)""#)).map_err(io::Error::other)?;
        let values = pattern
            .captures_iter(text)
            .map(|capture| capture[1].parse::<f64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        if values.len() < 2 || values.iter().any(|value| *value != expected) {
            return Err(invalid(format!(
                "Native terrain controls did not prove {name}={expected} before and after map load"
            )));
        }
    }
    Ok(true)
}

struct Captured {
    readbacks: Vec<NativeTerrainReadback>,
    process: Value,
    readiness: Value,
    vertex_count: usize,
}

enum Attempt {
    Pending(String),
    Captured(Captured),
}

fn extract(
    process: &mut process::StoppedProcess<'_>,
    world: readiness::ReadyWorld,
    carrier: &NativeTerrainCarrier,
) -> io::Result<Captured> {
    if world.displacement_count != carrier.displacements().len() {
        return Err(invalid(
            "Native terrain displacement catalog differs from the exact BSP",
        ));
    }
    let surface_base = process.read_u64(address(world.world, WORLD_SURFACES)?)?;
    if surface_base == 0 {
        return Err(invalid("Native terrain world has no parent FACE array"));
    }
    let mut readbacks = Vec::new();
    readbacks
        .try_reserve_exact(world.displacement_count)
        .map_err(io::Error::other)?;
    let mut vertex_count = 0usize;
    let mut raw_bytes = 0usize;
    for source in carrier.displacements() {
        let displacement = address(
            world.displacement_base,
            usize::from(source.ordinal())
                .checked_mul(readiness::DISPLACEMENT_STRIDE)
                .ok_or_else(|| invalid("Native terrain displacement offset overflow"))?
                as u64,
        )?;
        let bytes = process.read_exact(displacement, readiness::DISPLACEMENT_STRIDE)?;
        let ordinal = u16_at(&bytes, DISP_ORDINAL)?;
        let power = u32_at(&bytes, DISP_POWER)? as i32;
        let count =
            usize::try_from(u32_at(&bytes, DISP_VERTEX_COUNT)?).map_err(io::Error::other)?;
        let vertices = u64_at(&bytes, DISP_VERTICES)?;
        let parent = address(
            surface_base,
            usize::from(source.face())
                .checked_mul(SURFACE_STRIDE)
                .ok_or_else(|| invalid("Native terrain FACE offset overflow"))? as u64,
        )?;
        let power_info = u64_at(&bytes, DISP_POWER_INFO)?;
        if ordinal != source.ordinal()
            || power != source.power()
            || count != source.vertex_count()
            || u64_at(&bytes, DISP_HANDLE)? != world.handle
            || u64_at(&bytes, DISP_VERTEX_MIRROR)? != vertices
            || u64_at(&bytes, DISP_PARENT)? != parent
            || power_info == 0
        {
            return Err(invalid(
                "Native terrain displacement ordinal, power, owner, parent or vertex backing differs from its carrier",
            ));
        }
        process.require_range(parent, SURFACE_STRIDE, true)?;
        if process.read_u64(address(parent, SURFACE_DISPLACEMENT)?)? != displacement {
            return Err(invalid(
                "Native terrain FACE back-reference differs from its displacement",
            ));
        }
        if process.read_u32(address(power_info, POWER_VERTEX_COUNT)?)? as usize != count {
            return Err(invalid(
                "Native terrain power-info maximum vertex count differs from its carrier",
            ));
        }
        let length =
            process.checked_array(vertices, count, NATIVE_TERRAIN_VERTEX_STRIDE, MAX_VERTICES)?;
        raw_bytes = raw_bytes
            .checked_add(length)
            .filter(|bytes| *bytes <= MAX_READBACK_BYTES)
            .ok_or_else(|| invalid("Native terrain raw snapshot exceeds 64 MiB"))?;
        vertex_count = vertex_count
            .checked_add(count)
            .ok_or_else(|| invalid("Native terrain vertex count overflow"))?;
        readbacks.push(NativeTerrainReadback {
            ordinal,
            power,
            vertices: process.read_exact(vertices, length)?,
        });
    }
    Ok(Captured {
        readbacks,
        process: serde_json::to_value(process.receipt()).map_err(io::Error::other)?,
        readiness: serde_json::to_value(world).map_err(io::Error::other)?,
        vertex_count,
    })
}

fn record_capture(
    directory: &Path,
    profile: &TerrainSnapshotProfile,
    captured: Captured,
) -> io::Result<()> {
    let bytes = encode_readbacks(&captured.readbacks)?;
    let observation = TerrainObservationReceipt {
        profile: PROFILE.to_owned(),
        engine_sha256: util::ENGINE_SHA.to_owned(),
        bsp_sha256: profile.bsp_sha256.clone(),
        readback_sha256: util::hash(&bytes),
        map_name: profile.map_name.clone(),
        channel: profile.channel,
        displacement_count: captured.readbacks.len(),
        vertex_count: captured.vertex_count,
        process: captured.process,
        readiness: captured.readiness,
        resumed: true,
    };
    util::atomic_write(&directory.join(READBACK_FILE), &bytes)?;
    util::json_write(&directory.join(OBSERVATION_FILE), &observation)
}

pub(crate) fn execute(
    directory: &Path,
    request: &WorkerRequest,
    profile: &TerrainSnapshotProfile,
) -> io::Result<()> {
    profile.validate()?;
    checkpoint(directory, request)?;
    let root = request.environment.engine_root.canonicalize()?;
    let staged = PinnedFile::open(&directory.join("staged.bsp"), &profile.bsp_sha256)?;
    let carrier = carrier(
        &util::read(&staged.path, util::MAX_BSP)?,
        profile.channel,
        &mut || checkpoint(directory, request),
    )?;
    let engine = EngineImage::open(&root)?;
    let game = crate::worker::game_directory(directory, request)?;
    let consumed = PinnedFile::open(
        &game.join("maps").join(format!("{}.bsp", profile.map_name)),
        &profile.bsp_sha256,
    )?;
    verify_providers(&request.environment, &game, &profile.map_name)?;
    let captured = crate::owned_client::run(
        directory,
        request,
        &game,
        CONSOLE_FILE,
        PROFILE,
        "terrain-process-cleanup.json",
        |client| {
            let checked = (|| {
                if !verify_controls(client.console, profile.channel)? {
                    return Ok(None);
                }
                staged.verify()?;
                consumed.verify()?;
                engine.input.verify()?;
                Ok(Some(()))
            })();
            let checked = checked.map_err(|error| CaptureFailure {
                error,
                cleanup: CleanupDisposition::Confirmed,
            })?;
            if checked.is_none() {
                return Ok(None);
            }
            let mut check = || checkpoint(directory, request);
            let attempt = process::with_stopped(
                client.launch,
                client.child,
                client.executable,
                client.identity,
                &mut check,
                |process| {
                    let binding = engine.bind(process)?;
                    let attempt = match readiness::inspect(
                        process,
                        binding.load_bias,
                        &format!("maps/{}.bsp", profile.map_name),
                    )? {
                        readiness::Readiness::Pending(reason) => {
                            Attempt::Pending(reason.to_string())
                        }
                        readiness::Readiness::Ready(world) => {
                            let mut captured = extract(process, world, &carrier)?;
                            let receipt = captured
                                .process
                                .as_object_mut()
                                .ok_or_else(|| invalid("Invalid native terrain process receipt"))?;
                            receipt.insert("engine".into(), binding.receipt(&engine.input));
                            receipt.insert("bsp".into(), consumed.receipt());
                            Attempt::Captured(captured)
                        }
                    };
                    binding.namespace.verify(process, &engine.input.path)?;
                    Ok(attempt)
                },
            )?;
            let verified = (|| match attempt {
                Attempt::Pending(reason) => {
                    util::json_write(
                        &directory.join("terrain-pending.json"),
                        &json!({"reason":reason}),
                    )?;
                    Ok(None)
                }
                Attempt::Captured(captured) => {
                    staged.verify()?;
                    consumed.verify()?;
                    engine.input.verify()?;
                    verify_providers(&request.environment, &game, &profile.map_name)?;
                    checkpoint(directory, request)?;
                    Ok(Some(captured))
                }
            })();
            verified.map_err(|error| CaptureFailure {
                error,
                cleanup: CleanupDisposition::Confirmed,
            })
        },
    )?;
    checkpoint(directory, request)?;
    record_capture(directory, profile, captured)
}

fn encode_readbacks(readbacks: &[NativeTerrainReadback]) -> io::Result<Vec<u8>> {
    let length = readbacks.iter().try_fold(
        READBACK_MAGIC.len() + size_of::<u32>(),
        |length, readback| {
            length
                .checked_add(10)
                .and_then(|length| length.checked_add(readback.vertices.len()))
                .filter(|length| *length <= MAX_READBACK_BYTES)
                .ok_or_else(|| invalid("Native terrain readback exceeds 64 MiB"))
        },
    )?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(io::Error::other)?;
    bytes.extend_from_slice(READBACK_MAGIC);
    bytes.extend_from_slice(
        &u32::try_from(readbacks.len())
            .map_err(io::Error::other)?
            .to_le_bytes(),
    );
    for readback in readbacks {
        bytes.extend_from_slice(&readback.ordinal.to_le_bytes());
        bytes.extend_from_slice(&readback.power.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(readback.vertices.len())
                .map_err(io::Error::other)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&readback.vertices);
    }
    Ok(bytes)
}

pub(crate) fn decode_output(
    directory: &Path,
    profile: &TerrainSnapshotProfile,
    bsp: &[u8],
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<(NativeTerrainSnapshot, TerrainObservationReceipt)> {
    profile.validate()?;
    check()?;
    if util::hash(bsp) != profile.bsp_sha256 {
        return Err(invalid("Native terrain output BSP identity changed"));
    }
    let carrier = carrier(bsp, profile.channel, check)?;
    let bytes = util::read(&directory.join(READBACK_FILE), MAX_READBACK_BYTES)?;
    let observation: TerrainObservationReceipt =
        util::json_read(&directory.join(OBSERVATION_FILE), MAX_OBSERVATION_BYTES)?;
    if observation.profile != PROFILE
        || observation.engine_sha256 != util::ENGINE_SHA
        || observation.bsp_sha256 != profile.bsp_sha256
        || observation.readback_sha256 != util::hash(&bytes)
        || observation.map_name != profile.map_name
        || observation.channel != profile.channel
        || !observation.resumed
        || observation.displacement_count != carrier.displacements().len()
    {
        return Err(invalid(
            "Native terrain output attestation disagrees with its owned request",
        ));
    }
    if bytes.get(..READBACK_MAGIC.len()) != Some(READBACK_MAGIC.as_slice())
        || u32_at(&bytes, READBACK_MAGIC.len())? as usize != carrier.displacements().len()
    {
        return Err(invalid("Native terrain raw output header is invalid"));
    }
    let mut offset = READBACK_MAGIC.len() + size_of::<u32>();
    let mut readbacks = Vec::new();
    readbacks
        .try_reserve_exact(carrier.displacements().len())
        .map_err(io::Error::other)?;
    for source in carrier.displacements() {
        check()?;
        let ordinal = u16_at(&bytes, offset)?;
        let power = u32_at(&bytes, offset + 2)? as i32;
        let length = u32_at(&bytes, offset + 6)? as usize;
        offset = offset
            .checked_add(10)
            .ok_or_else(|| invalid("Native terrain output offset overflow"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| invalid("Native terrain output length overflow"))?;
        if ordinal != source.ordinal()
            || power != source.power()
            || length != source.vertex_count() * NATIVE_TERRAIN_VERTEX_STRIDE
        {
            return Err(invalid(
                "Native terrain raw output catalog differs from its carrier",
            ));
        }
        let record = bytes
            .get(offset..end)
            .ok_or_else(|| invalid("Native terrain raw output is incomplete"))?;
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(length)
            .map_err(io::Error::other)?;
        vertices.extend_from_slice(record);
        readbacks.push(NativeTerrainReadback {
            ordinal,
            power,
            vertices,
        });
        offset = end;
    }
    if offset != bytes.len() {
        return Err(invalid("Native terrain raw output has trailing bytes"));
    }
    drop(bytes);
    let snapshot = carrier.decode_controlled(&readbacks, check)?;
    if snapshot.sample_count() != observation.vertex_count
        || snapshot.retained_bytes() > MAX_READBACK_BYTES
    {
        return Err(invalid(
            "Native terrain output count or retained byte budget is invalid",
        ));
    }
    check()?;
    Ok((snapshot, observation))
}

fn address(base: u64, offset: u64) -> io::Result<u64> {
    base.checked_add(offset)
        .ok_or_else(|| invalid("Native terrain address overflow"))
}

fn u16_at(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let end = offset
        .checked_add(size_of::<u16>())
        .ok_or_else(|| invalid("Native terrain u16 offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("Incomplete native terrain u16"))?;
    Ok(u16::from_le_bytes(
        bytes.try_into().map_err(io::Error::other)?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset
        .checked_add(size_of::<u32>())
        .ok_or_else(|| invalid("Native terrain u32 offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("Incomplete native terrain u32"))?;
    Ok(u32::from_le_bytes(
        bytes.try_into().map_err(io::Error::other)?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let end = offset
        .checked_add(size_of::<u64>())
        .ok_or_else(|| invalid("Native terrain u64 offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("Incomplete native terrain u64"))?;
    Ok(u64::from_le_bytes(
        bytes.try_into().map_err(io::Error::other)?,
    ))
}
