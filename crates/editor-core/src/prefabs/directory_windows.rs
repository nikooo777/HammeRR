use super::*;
use crate::preference_file::DirectoryBinding;
use source_formats::file_publication::{
    FileIdentity,
    windows::{open_directory, open_file, rename_noreplace},
};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
    sync::Arc,
};

#[derive(Clone, Debug)]
pub(super) struct SourceDirectory {
    pub binding: DirectoryBinding,
    _retained: Arc<File>,
}

impl PartialEq for SourceDirectory {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
}

impl Eq for SourceDirectory {}

impl SourceDirectory {
    pub fn verify(&self) -> Result<(), String> {
        self.binding.verify()
    }
}

pub(super) struct Directory {
    pub binding: DirectoryBinding,
    requested: PathBuf,
    _ancestor: File,
    file: Option<Arc<File>>,
}

impl Directory {
    pub fn capture(requested: &Path) -> Result<Self, String> {
        let binding =
            DirectoryBinding::capture(requested, "Prefab library", "refresh it before continuing")?;
        let ancestor =
            open_directory(binding.physical_ancestor()).map_err(|error| error.to_string())?;
        let file = if binding.suffix()?.as_os_str().is_empty() {
            Some(Arc::new(
                ancestor.try_clone().map_err(|error| error.to_string())?,
            ))
        } else {
            None
        };
        let directory = Self {
            requested: binding.requested().to_owned(),
            binding,
            _ancestor: ancestor,
            file,
        };
        directory.verify()?;
        Ok(directory)
    }

    pub fn verify(&self) -> Result<(), String> {
        self.binding.verify()?;
        match (&self.file, open_directory(&self.requested)) {
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Some(expected), Ok(actual))
                if FileIdentity::from_file(expected).map_err(|error| error.to_string())?
                    == FileIdentity::from_file(&actual).map_err(|error| error.to_string())? =>
            {
                Ok(())
            }
            _ => Err("Prefab library directory changed; refresh it before continuing".into()),
        }
    }

    pub fn is_present(&self) -> bool {
        self.file.is_some()
    }

    pub fn create(&mut self) -> Result<(), String> {
        self.verify()?;
        if self.file.is_none() {
            fs::create_dir_all(&self.requested).map_err(|error| error.to_string())?;
            self.binding.verify()?;
            self.file = Some(Arc::new(
                open_directory(&self.requested).map_err(|error| error.to_string())?,
            ));
        }
        self.verify()
    }

    pub fn source(&self) -> Result<SourceDirectory, String> {
        self.verify()?;
        Ok(SourceDirectory {
            binding: self.binding.clone(),
            _retained: self.file.clone().ok_or("Prefab library no longer exists")?,
        })
    }

    fn entry(&self, name: &OsStr) -> Result<PathBuf, String> {
        self.verify()?;
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err("Prefab entry must be a basename".into());
        }
        Ok(self.requested.join(name))
    }

    pub fn names(&self) -> Result<Vec<OsString>, String> {
        self.verify()?;
        if !self.is_present() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.requested).map_err(|error| error.to_string())? {
            if names.len() >= MAX_SCAN {
                return Err("Prefab storage exceeds 512 directory entries".into());
            }
            names.push(entry.map_err(|error| error.to_string())?.file_name());
        }
        self.verify()?;
        Ok(names)
    }

    pub fn metadata(&self, name: &OsStr) -> Result<fs::Metadata, String> {
        fs::symlink_metadata(self.entry(name)?).map_err(|error| error.to_string())
    }

    pub fn open(&self, name: &OsStr) -> Result<File, String> {
        open_file(&self.entry(name)?).map_err(|error| error.to_string())
    }

    pub fn rename(&self, source: &str, destination: &str) -> Result<(), String> {
        rename_noreplace(
            &self.entry(OsStr::new(source))?,
            &self.entry(OsStr::new(destination))?,
        )
        .map_err(|error| format!("Cannot rename prefab without replacing another entry: {error}"))
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        fs::remove_file(self.entry(OsStr::new(name))?).map_err(|error| error.to_string())
    }

    pub fn publication(&self, action: &str) -> Mutation {
        Mutation { warning: self.verify().err().map(|error| format!("Prefab was {action}, but the library could not be verified; refresh it before continuing: {error}")) }
    }

    pub fn stage(&self) -> Result<Stage<'_>, String> {
        self.verify()?;
        let temporary = tempfile::Builder::new()
            .prefix(".hammerr-prefab-")
            .disable_cleanup(true)
            .tempfile_in(&self.requested)
            .map_err(|error| error.to_string())?;
        Ok(Stage {
            temporary,
            directory: self,
        })
    }
}

pub(super) struct Stage<'a> {
    temporary: tempfile::NamedTempFile,
    directory: &'a Directory,
}

impl Stage<'_> {
    pub fn write(&mut self, text: &str) -> Result<(), String> {
        self.temporary
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        self.temporary
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())
    }

    fn owned(&self) -> Result<bool, String> {
        match open_file(self.temporary.path()) {
            Ok(file) => Ok(
                FileIdentity::from_file(&file).map_err(|error| error.to_string())?
                    == FileIdentity::from_file(self.temporary.as_file())
                        .map_err(|error| error.to_string())?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn publish(&self, destination: &str) -> Result<(), String> {
        if !self.owned()? {
            return Err("Prefab staging name was replaced; unrelated file preserved".into());
        }
        fs::hard_link(
            self.temporary.path(),
            self.directory.entry(OsStr::new(destination))?,
        )
        .map_err(|error| error.to_string())
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if matches!(self.owned(), Ok(true)) {
            let _ = fs::remove_file(self.temporary.path());
        }
    }
}
