use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Missing,
    File {
        bytes: Vec<u8>,
        length: u64,
        modified: std::time::SystemTime,
        #[cfg(unix)]
        identity: (u64, u64),
    },
}

pub(crate) fn read_source(path: &Path, max_file_bytes: u64, label: &str) -> Result<Source, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Source::Missing);
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !metadata.is_file() {
        return Err(format!("{} must be a regular file", path.display()));
    }
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
            return Err(format!("{label} were replaced while loading"));
        }
    }
    let modified = metadata
        .modified()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !opened.is_file()
        || opened.len() != metadata.len()
        || opened.modified().map_err(|error| error.to_string())? != modified
    {
        return Err(format!("{label} changed while loading"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(max_file_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    if bytes.len() as u64 != metadata.len().min(max_file_bytes + 1)
        || after.len() != metadata.len()
        || after.modified().map_err(|error| error.to_string())? != modified
    {
        return Err(format!("{label} changed size while loading"));
    }
    Ok(Source::File {
        bytes,
        length: metadata.len(),
        modified,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        },
    })
}

pub(crate) fn ensure_regular_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("{} must be a regular file", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}
