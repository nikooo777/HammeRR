use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const READ_BYTES: usize = 64 * 1024;

pub(crate) struct Source {
    requested: PathBuf,
    target: PathBuf,
    metadata: Metadata,
    limit: u64,
}

fn same_metadata(before: &Metadata, after: &Metadata) -> bool {
    let same = after.is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && (
            before.dev(),
            before.ino(),
            before.ctime(),
            before.ctime_nsec(),
        ) == (after.dev(), after.ino(), after.ctime(), after.ctime_nsec())
    }
    #[cfg(not(unix))]
    same
}

impl Source {
    pub(crate) fn capture(path: &Path, limit: u64) -> io::Result<Self> {
        let requested = std::path::absolute(path)?;
        let target = requested.canonicalize()?;
        let metadata = fs::symlink_metadata(&target)?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err(io::Error::other(format!(
                "Choose a regular file no larger than {limit} bytes"
            )));
        }
        Ok(Self {
            requested,
            target,
            metadata,
            limit,
        })
    }

    pub(crate) fn read(self) -> io::Result<Vec<u8>> {
        self.read_controlled(|| Ok(()))
    }

    pub(crate) fn read_controlled(
        self,
        mut checkpoint: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Vec<u8>> {
        checkpoint()?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::OFlags;
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags((OFlags::NONBLOCK | OFlags::NOFOLLOW).bits() as i32);
        }
        let file = options.open(&self.target)?;
        if !same_metadata(&self.metadata, &file.metadata()?) {
            return Err(io::Error::other("File changed while opening"));
        }
        let maximum = self
            .limit
            .checked_add(1)
            .ok_or_else(|| io::Error::other("File byte limit overflow"))?;
        let mut bytes = Vec::new();
        let mut reader = (&file).take(maximum);
        let mut buffer = [0; READ_BYTES];
        loop {
            checkpoint()?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        if bytes.len() as u64 > self.limit {
            return Err(io::Error::other(format!(
                "File exceeds the {}-byte limit",
                self.limit
            )));
        }
        if bytes.len() as u64 != self.metadata.len()
            || !same_metadata(&self.metadata, &file.metadata()?)
            || !same_metadata(&self.metadata, &fs::symlink_metadata(&self.target)?)
            || self.requested.canonicalize()? != self.target
        {
            return Err(io::Error::other("File changed while reading"));
        }
        checkpoint()?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;
