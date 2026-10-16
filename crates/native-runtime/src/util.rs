use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

pub(crate) const MAX_BSP: usize = 128 * 1024 * 1024;
pub(crate) const MAX_LOG: usize = 2 * 1024 * 1024;
pub(crate) const MAX_CONFIG: usize = 16 * 1024 * 1024;
pub(crate) const ENGINE_SHA: &str =
    "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a";
pub(crate) const STEAM_SHA: &str =
    "c0cc3d2802e5f2463bfa0046c41d2f65a6335baaeefbba6c7dbd5681d5ca7c46";
pub(crate) const VTEX_SHA: &str =
    "923e9723cbd925c63d20fa035c95ada879c4d2fb27b7b133b7306dedef6d8c6e";
pub(crate) const BSPPACK_SHA: &str =
    "294b689c698888d3b1d12d7a26440a9aed977275f120a784fc7a8be39a73425c";

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(crate) fn read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(invalid(format!(
            "{} exceeds its read budget",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid(format!(
            "{} grew beyond its read budget",
            path.display()
        )));
    }
    Ok(bytes)
}

pub(crate) fn text(path: &Path, limit: usize) -> io::Result<String> {
    String::from_utf8(read(path, limit)?).map_err(|e| invalid(e.to_string()))
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn hash_file(path: &Path, limit: usize) -> io::Result<String> {
    Ok(hash(&read(path, limit)?))
}

pub(crate) fn json_write(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub(crate) fn json_read<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: usize,
) -> io::Result<T> {
    serde_json::from_slice(&read(path, limit)?).map_err(io::Error::other)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| invalid("File has no parent"))?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    pending.write_all(bytes)?;
    pending.as_file().sync_all()?;
    pending.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub(crate) fn validate_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(invalid(
            "Native map name requires 1–80 lowercase letters, digits or underscores",
        ));
    }
    Ok(())
}

pub(crate) fn private_budget(root: &Path, engine: &Path) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let mut pending = vec![root.to_owned()];
    let mut count = 0usize;
    let mut bytes = 0u64;
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            count += 1;
            if count > 16384 {
                return Err(invalid("Native private entry budget exceeded"));
            }
            if metadata.file_type().is_symlink() {
                if entry.path() == root.join("bin")
                    && entry.path().canonicalize()? == engine.join("bin").canonicalize()?
                {
                    continue;
                }
                return Err(invalid("Unexpected symlink in private native output"));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| invalid("Native private byte count overflow"))?;
            } else if !(entry.path().starts_with(root.join("wayland"))
                && metadata.file_type().is_socket())
            {
                return Err(invalid("Unexpected special file in private native output"));
            }
            if bytes > 256 * 1024 * 1024 {
                return Err(invalid(
                    "Native private output exceeds its shared 256 MiB budget",
                ));
            }
        }
    }
    Ok(())
}
