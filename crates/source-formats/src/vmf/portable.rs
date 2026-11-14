use std::collections::BTreeSet;
use std::io::{self, Write};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use sha2::{Digest, Sha256};

use crate::bsp::{PakArchive, PakEntry, PakLimits};
use crate::gamefs::asset_relative;
use crate::keyvalues::Node;

pub const BLOCK: &str = "hammerr_assets";
pub const MAX_VMF_BYTES: usize = 160 * 1024 * 1024;
const CHUNK: usize = 192;
const MAX_CHUNK_TEXT: usize = 4096;
const BUFFER: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAssets {
    archive: std::sync::Arc<PakArchive>,
    compressed: std::sync::Arc<[u8]>,
    digest: String,
    archive_bytes: usize,
    decoded_bytes: usize,
    base: BTreeSet<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn asset_path(path: &str) -> io::Result<String> {
    let canonical = asset_relative(path)?;
    if canonical.is_empty()
        || canonical.len() > PakLimits::default().max_name_bytes
        || !canonical
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"<>:\"|?*".contains(&b) || b == b' ')
        || (!canonical.ends_with(".vbsp")
            && !canonical.ends_with(".rad")
            && ![
                "materials/",
                "models/",
                "sound/",
                "scripts/",
                "particles/",
                "resource/",
                "maps/",
            ]
            .iter()
            .any(|prefix| canonical.starts_with(prefix)))
    {
        return Err(invalid(format!(
            "Unsupported portable resource path: {path}"
        )));
    }
    for component in canonical.split('/') {
        let stem = component.split('.').next().unwrap_or_default();
        if component.is_empty()
            || component.ends_with(['.', ' '])
            || ["con", "prn", "aux", "nul"].contains(&stem)
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid(format!("Nonportable resource path: {path}")));
        }
    }
    let extension = canonical.rsplit('.').next().unwrap_or_default();
    if ![
        "vmt", "vtf", "mdl", "vvd", "vtx", "phy", "ani", "wav", "mp3", "pcf", "txt", "vbsp", "rad",
        "res", "vcd", "nav", "raw",
    ]
    .contains(&extension)
    {
        return Err(invalid(format!(
            "Unsupported portable resource type: {path}"
        )));
    }
    Ok(canonical)
}

fn validate_tree(names: &BTreeSet<String>) -> io::Result<()> {
    for path in names {
        for (separator, _) in path.match_indices('/') {
            if names.contains(&path[..separator]) {
                return Err(invalid(format!(
                    "Portable asset is also a directory: {}",
                    &path[..separator]
                )));
            }
        }
    }
    Ok(())
}

impl MapAssets {
    pub fn entries(&self) -> &[PakEntry] {
        &self.archive.entries
    }
    pub fn base_dependencies(&self) -> &BTreeSet<String> {
        &self.base
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn compressed_bytes(&self) -> usize {
        self.compressed.len()
    }
    pub fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }

    pub fn new(mut archive: PakArchive, base: BTreeSet<String>) -> io::Result<Self> {
        let limits = PakLimits::default();
        let mut identities = BTreeSet::new();
        for entry in &mut archive.entries {
            let path = asset_path(&entry.name)?;
            if !identities.insert(path.clone()) {
                return Err(invalid(format!(
                    "Case-colliding portable asset identity: {}",
                    entry.name
                )));
            }
            entry.name = path;
        }
        validate_tree(&identities)?;
        for path in &base {
            if asset_path(path)? != *path || identities.contains(path) {
                return Err(invalid(format!(
                    "Invalid or overlapping base-game dependency: {path}"
                )));
            }
        }
        if base.len() > limits.max_entries {
            return Err(invalid("Too many base-game dependencies"));
        }
        archive.entries.sort_by(|a, b| a.name.cmp(&b.name));
        let bytes = archive.to_bytes(limits)?;
        let mut compressed = Vec::new();
        let mut encoder = Compress::new(Compression::default(), false);
        let mut buffer = [0; BUFFER];
        loop {
            let before_in = encoder.total_in() as usize;
            let before_out = encoder.total_out();
            let status = encoder
                .compress(&bytes[before_in..], &mut buffer, FlushCompress::Finish)
                .map_err(|error| invalid(format!("Portable compression failed: {error}")))?;
            let count = (encoder.total_out() - before_out) as usize;
            if compressed.len() + count > limits.max_archive_bytes {
                return Err(invalid("Compressed portable assets exceed 64 MiB"));
            }
            compressed.extend_from_slice(&buffer[..count]);
            if status == Status::StreamEnd {
                break;
            }
            if encoder.total_in() as usize == before_in && count == 0 {
                return Err(invalid("Portable compression made no progress"));
            }
        }
        let digest = format!("{:x}", Sha256::digest(&compressed));
        let decoded_bytes = archive.entries.iter().map(|entry| entry.data.len()).sum();
        Ok(Self {
            archive: std::sync::Arc::new(archive),
            compressed: compressed.into(),
            digest,
            archive_bytes: bytes.len(),
            decoded_bytes,
            base,
        })
    }

    pub fn with_base_dependencies(&self, base: BTreeSet<String>) -> io::Result<Self> {
        if base.len() > PakLimits::default().max_entries
            || base.iter().any(|path| {
                !asset_path(path).is_ok_and(|canonical| canonical == *path)
                    || self.entries().iter().any(|entry| entry.name == *path)
            })
        {
            return Err(invalid("Invalid base-game dependencies"));
        }
        Ok(Self {
            base,
            ..self.clone()
        })
    }

    pub fn same_content(&self, archive: &PakArchive, base: &BTreeSet<String>) -> bool {
        self.base == *base && self.same_assets(archive)
    }

    pub fn same_assets(&self, archive: &PakArchive) -> bool {
        self.archive.entries.len() == archive.entries.len()
            && archive
                .entries
                .iter()
                .all(|entry| self.archive.entries.iter().any(|old| old == entry))
    }

    pub(crate) fn extract(&self, root: &std::path::Path) -> io::Result<()> {
        for entry in self.entries() {
            let path = root.join(&entry.name);
            std::fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| invalid("Asset has no parent"))?,
            )?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(&entry.data)?;
        }
        Ok(())
    }

    pub fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("version", "1"),
            Node::string("encoding", "deflate-zip32-base64"),
            Node::string("sha256", &self.digest),
            Node::string("archive_bytes", self.archive_bytes.to_string()),
            Node::string("decoded_bytes", self.decoded_bytes.to_string()),
            Node::string("file_count", self.entries().len().to_string()),
        ];
        fields.extend(self.base.iter().map(|path| Node::string("base", path)));
        fields.extend(
            self.compressed
                .chunks(CHUNK)
                .map(|chunk| Node::string("chunk", STANDARD.encode(chunk))),
        );
        Node::block(BLOCK, fields)
    }

    pub fn from_node(node: &Node) -> io::Result<Self> {
        let fields = node
            .children()
            .ok_or_else(|| invalid("Portable asset package must be a block"))?;
        let limits = PakLimits::default();
        if fields.len() > limits.max_entries + limits.max_archive_bytes.div_ceil(CHUNK) + 6 {
            return Err(invalid("Portable asset metadata exceeds its limit"));
        }
        let mut values = std::collections::BTreeMap::new();
        let mut base = BTreeSet::new();
        let mut compressed = Vec::new();
        for field in fields {
            let value = field
                .as_str()
                .ok_or_else(|| invalid("Nested portable asset metadata is unsupported"))?;
            match field.key.as_str() {
                "base" => {
                    if asset_path(value)? != value
                        || !base.insert(value.to_owned())
                        || base.len() > limits.max_entries
                    {
                        return Err(invalid("Duplicate or invalid base-game dependency"));
                    }
                }
                "chunk" => {
                    if value.len() > MAX_CHUNK_TEXT || value.is_empty() {
                        return Err(invalid("Invalid portable asset chunk size"));
                    }
                    let chunk = STANDARD
                        .decode(value)
                        .map_err(|error| invalid(format!("Portable asset base64: {error}")))?;
                    if compressed.len() + chunk.len() > limits.max_archive_bytes {
                        return Err(invalid("Compressed portable assets exceed 64 MiB"));
                    }
                    compressed.extend(chunk);
                }
                "version" | "encoding" | "sha256" | "archive_bytes" | "decoded_bytes"
                | "file_count" => {
                    if values.insert(field.key.as_str(), value).is_some() {
                        return Err(invalid(format!("Duplicate portable field: {}", field.key)));
                    }
                }
                _ => {
                    return Err(invalid(format!(
                        "Unsupported portable field: {}",
                        field.key
                    )));
                }
            }
        }
        if values.get("version") != Some(&"1")
            || values.get("encoding") != Some(&"deflate-zip32-base64")
        {
            return Err(invalid(
                "Unsupported portable VMF package version or encoding",
            ));
        }
        let number = |key| {
            values
                .get(key)
                .ok_or_else(|| invalid(format!("Missing portable field: {key}")))?
                .parse::<usize>()
                .map_err(|_| invalid(format!("Invalid portable field: {key}")))
        };
        let archive_bytes = number("archive_bytes")?;
        let decoded_bytes = number("decoded_bytes")?;
        let file_count = number("file_count")?;
        if archive_bytes > limits.max_archive_bytes
            || decoded_bytes > limits.max_total_bytes
            || file_count > limits.max_entries
        {
            return Err(invalid("Portable package exceeds decoded resource limits"));
        }
        let digest = format!("{:x}", Sha256::digest(&compressed));
        if values.get("sha256") != Some(&digest.as_str()) {
            return Err(invalid("Portable asset SHA-256 mismatch"));
        }
        let mut decoder = Decompress::new(false);
        let mut archive = Vec::new();
        let mut buffer = [0; BUFFER];
        loop {
            let before_in = decoder.total_in() as usize;
            let before_out = decoder.total_out();
            let status = decoder
                .decompress(
                    &compressed[before_in..],
                    &mut buffer,
                    FlushDecompress::Finish,
                )
                .map_err(|error| invalid(format!("Portable DEFLATE failed: {error}")))?;
            let count = (decoder.total_out() - before_out) as usize;
            if archive.len() + count > archive_bytes {
                return Err(invalid("Portable package exceeds advertised archive bytes"));
            }
            archive.extend_from_slice(&buffer[..count]);
            if status == Status::StreamEnd {
                if decoder.total_in() as usize != compressed.len() || archive.len() != archive_bytes
                {
                    return Err(invalid(
                        "Portable DEFLATE has trailing bytes or an invalid length",
                    ));
                }
                break;
            }
            if decoder.total_in() as usize == before_in && count == 0 {
                return Err(invalid("Portable DEFLATE is truncated"));
            }
        }
        let archive = PakArchive::parse(&archive, limits)?;
        let mut names = BTreeSet::new();
        for entry in &archive.entries {
            if asset_path(&entry.name)? != entry.name
                || !names.insert(entry.name.clone())
                || base.contains(&entry.name)
            {
                return Err(invalid(format!(
                    "Invalid or colliding portable asset: {}",
                    entry.name
                )));
            }
        }
        validate_tree(&names)?;
        if archive.entries.len() != file_count
            || archive
                .entries
                .iter()
                .map(|entry| entry.data.len())
                .sum::<usize>()
                != decoded_bytes
        {
            return Err(invalid(
                "Portable package manifest sizes disagree with its PAK",
            ));
        }
        Ok(Self {
            archive: std::sync::Arc::new(archive),
            compressed: compressed.into(),
            digest,
            archive_bytes,
            decoded_bytes,
            base,
        })
    }
}
