use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::util::{hash, invalid};

const MAX_PREFIX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_APPENDED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    device: u64,
    inode: u64,
    start: u64,
    start_sha256: String,
    length: u64,
    sha256: String,
}

#[derive(Debug)]
pub(super) struct Appended {
    pub lines: Vec<String>,
    pub uploads: Vec<String>,
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    b.is_file() && (a.dev(), a.ino()) == (b.dev(), b.ino())
}

fn snapshot(path: &Path) -> io::Result<(Metadata, Vec<u8>)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_PREFIX_BYTES {
        return Err(invalid(
            "Steam Cloud log requires a regular file of at most 64 MiB",
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(before.len() as usize)
        .map_err(io::Error::other)?;
    (&file).take(before.len()).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if bytes.len() as u64 != before.len()
        || !same_file(&before, &after)
        || !same_file(&before, &current)
        || after.len() < before.len()
        || current.len() < before.len()
    {
        return Err(invalid(
            "Steam Cloud log changed or truncated while observing it",
        ));
    }
    Ok((before, bytes))
}

fn require_complete_records(bytes: &[u8]) -> io::Result<()> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(invalid("Steam Cloud log ends with an incomplete record"));
    }
    Ok(())
}

impl Observation {
    pub(super) fn capture(steam: &Path) -> io::Result<Self> {
        let (metadata, bytes) = snapshot(&steam.join("logs/cloud_log.txt"))?;
        require_complete_records(&bytes)?;
        let sha256 = hash(&bytes);
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            start: bytes.len() as u64,
            start_sha256: sha256.clone(),
            length: bytes.len() as u64,
            sha256,
        })
    }

    pub(super) fn length(&self) -> u64 {
        self.length
    }

    pub(super) fn resume(&self, next: Self) -> io::Result<Self> {
        if self.device != next.device
            || self.inode != next.inode
            || self.start != next.start
            || self.start_sha256 != next.start_sha256
            || next.length < self.length
        {
            return Err(invalid(
                "Steam Cloud log observation does not continue the original baseline",
            ));
        }
        Ok(next)
    }

    pub(super) fn read(&mut self, steam: &Path) -> io::Result<Appended> {
        if self.start > self.length
            || self.length > MAX_PREFIX_BYTES
            || [&self.sha256, &self.start_sha256]
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(invalid("Invalid Steam Cloud log observation"));
        }
        let (metadata, bytes) = snapshot(&steam.join("logs/cloud_log.txt"))?;
        if (metadata.dev(), metadata.ino()) != (self.device, self.inode)
            || (bytes.len() as u64) < self.length
            || hash(&bytes[..self.start as usize]) != self.start_sha256
            || hash(&bytes[..self.length as usize]) != self.sha256
        {
            return Err(invalid(
                "Steam Cloud log rotated, truncated or its observed prefix changed",
            ));
        }
        if self.start > 0 && bytes[self.start as usize - 1] != b'\n' {
            return Err(invalid(
                "Steam Cloud log observation starts inside a record",
            ));
        }
        require_complete_records(&bytes)?;
        let appended = &bytes[self.start as usize..];
        if appended.len() > MAX_APPENDED_BYTES {
            return Err(invalid(
                "Steam Cloud log exceeded the 8 MiB appended audit budget",
            ));
        }
        let lines = String::from_utf8_lossy(appended)
            .lines()
            .filter(|line| line.contains("[AppID 240]"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let uploads = lines
            .iter()
            .filter(|line| {
                [
                    "Need to upload file",
                    "HTTP upload",
                    "Upload batch initiated",
                    "Upload OK",
                ]
                .iter()
                .any(|term| line.contains(term))
            })
            .cloned()
            .collect();
        self.length = bytes.len() as u64;
        self.sha256 = hash(&bytes);
        Ok(Appended { lines, uploads })
    }
}
