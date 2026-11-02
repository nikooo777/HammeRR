use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::build_settings::BuildSettings;
use crate::preference_file::{CoordinatedFile, ObservedSource, Source};

pub const FILE_NAME: &str = "content-profiles.json";
pub const MAX_PROFILES: usize = 32;
pub const MAX_NAME_BYTES: usize = 80;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_ERROR_BYTES: usize = 512;
const LOCK_FILE_NAME: &str = ".content-profiles.lock";
const VERSION: u32 = 1;
const ID_PREFIX: &str = "profile:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub gameinfo: PathBuf,
    pub fgd: Option<PathBuf>,
    pub engine_root: Option<PathBuf>,
    pub steam_root: Option<PathBuf>,
    pub build: BuildSettings,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        validate_path(&self.gameinfo, "Gameinfo")?;
        for (label, path) in [
            ("FGD", &self.fgd),
            ("Engine root", &self.engine_root),
            ("Steam root", &self.steam_root),
        ] {
            if let Some(path) = path {
                validate_path(path, label)?;
            }
        }
        self.build.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub config: Config,
}

impl Profile {
    pub fn identity(&self) -> Identity {
        Identity {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    pub fn json(&self) -> Value {
        let mut profile = json!(self);
        profile["config"]["build"] = self.config.build.report();
        profile
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    next_id: u64,
    profiles: Vec<Profile>,
}

impl Default for File {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            profiles: Vec::new(),
        }
    }
}

struct Registry {
    file: File,
    revision: u64,
    path: Option<PathBuf>,
    disk: Option<ObservedSource>,
    blocked: bool,
    error: Option<String>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            file: File::default(),
            revision: 1,
            path: None,
            disk: None,
            blocked: true,
            error: Some("Content profile storage is not configured".into()),
        }
    }
}

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<Registry>>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub profiles: Vec<Profile>,
    pub revision: u64,
    pub path: Option<PathBuf>,
    pub blocked: bool,
    pub error: Option<String>,
}

impl Snapshot {
    pub fn find(&self, id: &str) -> Result<Profile, String> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .ok_or_else(|| "Content profile no longer exists; refresh the list".into())
    }

    pub fn persistence(&self) -> &'static str {
        if self.blocked {
            "blocked"
        } else if self.path.is_some() {
            "persistent"
        } else {
            "session"
        }
    }
}

pub(crate) enum Edit<'a> {
    Save {
        name: &'a str,
        replace: Option<&'a str>,
        config: &'a Config,
    },
    Delete {
        id: &'a str,
    },
}

pub(crate) fn configured_path(directory: Result<PathBuf, String>) -> Result<PathBuf, String> {
    configured_path_with_override(directory, std::env::var_os("HAMMERR_CONTENT_PROFILES"))
}

fn configured_path_with_override(
    directory: Result<PathBuf, String>,
    override_path: Option<OsString>,
) -> Result<PathBuf, String> {
    let path = match override_path {
        Some(path) if path.is_empty() => {
            return Err("HAMMERR_CONTENT_PROFILES must be a file path".into());
        }
        Some(path) => PathBuf::from(path),
        None => directory?.join(FILE_NAME),
    };
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    validate_path(&path, "Content profile storage")?;
    if path.file_name().is_none() {
        return Err("Content profile storage must have a file name".into());
    }
    Ok(path)
}

fn validate_path(path: &Path, label: &str) -> Result<(), String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("{label} path must be valid UTF-8"))?;
    if !path.is_absolute() || text.len() > MAX_PATH_BYTES || text.chars().any(char::is_control) {
        return Err(format!(
            "{label} path must be absolute, contain no control characters and fit in {MAX_PATH_BYTES} bytes"
        ));
    }
    Ok(())
}

fn profile_name(name: &str) -> Result<String, String> {
    crate::preference_file::named_preference(name, MAX_NAME_BYTES, "Profile")
}

fn validate(file: &File) -> Result<(), String> {
    if file.version != VERSION || file.profiles.len() > MAX_PROFILES || file.next_id == 0 {
        return Err("Unsupported or oversized content profile file".into());
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for profile in &file.profiles {
        let id = profile
            .id
            .strip_prefix(ID_PREFIX)
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|id| *id > 0 && *id < file.next_id)
            .ok_or("Invalid content profile identifier")?;
        if profile.id != format!("{ID_PREFIX}{id}") {
            return Err("Invalid content profile identifier".into());
        }
        if !ids.insert(id) || !names.insert(profile.name.to_lowercase()) {
            return Err("Duplicate content profile identifier or name".into());
        }
        if profile_name(&profile.name)? != profile.name {
            return Err("Stored profile name must not have surrounding whitespace".into());
        }
        profile.config.validate()?;
    }
    Ok(())
}

fn target(path: &Path) -> Result<CoordinatedFile<'_, &OsStr>, String> {
    Ok(CoordinatedFile {
        directory: path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
        file_name: path
            .file_name()
            .ok_or("Content profile storage must have a file name")?,
        lock_name: LOCK_FILE_NAME,
        label: "Content profiles",
        reload: "Reload profiles",
        saved_description: "saved profiles",
        max_bytes: MAX_FILE_BYTES as u64,
    })
}

fn load(path: &Path) -> Result<(File, ObservedSource), String> {
    let disk = target(path)?.observe_following()?;
    let file = match &disk.source {
        Source::File { bytes, length, .. } => {
            if *length > MAX_FILE_BYTES as u64 {
                return Err(format!(
                    "Content profile file exceeds {MAX_FILE_BYTES} bytes"
                ));
            }
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?
        }
        Source::Missing => File::default(),
    };
    validate(&file)?;
    Ok((file, disk))
}

impl Authority {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure_at(configured_path(directory));
        authority
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.state
            .lock()
            .expect("Content profiles authority was poisoned")
    }

    pub(crate) fn configure_at(&self, path: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        let loaded = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| load(path));
        state.path = path.ok();
        state.revision += 1;
        match loaded {
            Ok((file, disk)) => {
                state.accept(file, disk);
                Ok(())
            }
            Err(error) => Err(state.block(format!("Content profiles could not load: {error}"))),
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.lock().snapshot()
    }

    pub(crate) fn find(
        &self,
        id: &str,
        expected: Option<u64>,
    ) -> Result<(Profile, Snapshot), String> {
        self.use_profile(id, expected, |profile, snapshot| Ok((profile, snapshot)))
    }

    pub(crate) fn use_profile<T>(
        &self,
        id: &str,
        expected: Option<u64>,
        use_profile: impl FnOnce(Profile, Snapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.lock();
        state.check_revision(expected)?;
        state.check_current()?;
        let snapshot = state.snapshot();
        let profile = snapshot.find(id)?;
        use_profile(profile, snapshot)
    }

    pub(crate) fn reload(&self, expected: Option<u64>) -> Result<Snapshot, String> {
        let mut state = self.lock();
        state.check_revision(expected)?;
        let loaded = state
            .path
            .as_deref()
            .ok_or_else(|| "Content profile storage is unavailable".to_owned())
            .and_then(load);
        state.revision += 1;
        match loaded {
            Ok((file, disk)) => {
                state.accept(file, disk);
                Ok(state.snapshot())
            }
            Err(error) => Err(state.block(format!("Content profiles could not reload: {error}"))),
        }
    }

    pub(crate) fn edit(
        &self,
        expected: Option<u64>,
        edit: Edit<'_>,
    ) -> Result<(Profile, Snapshot), String> {
        let mut state = self.lock();
        state.check_revision(expected)?;
        state.ensure_available()?;
        let mut file = state.file.clone();
        let profile = match edit {
            Edit::Save {
                name,
                replace,
                config,
            } => {
                let name = profile_name(name)?;
                config.validate()?;
                match replace {
                    Some(id) => {
                        let profile = file
                            .profiles
                            .iter_mut()
                            .find(|profile| profile.id == id)
                            .ok_or("Choose an existing content profile to replace")?;
                        profile.name = name;
                        profile.config = config.clone();
                        profile.clone()
                    }
                    None => {
                        if file.profiles.len() == MAX_PROFILES {
                            return Err(format!("Keep at most {MAX_PROFILES} content profiles"));
                        }
                        let profile = Profile {
                            id: format!("{ID_PREFIX}{}", file.next_id),
                            name,
                            config: config.clone(),
                        };
                        file.next_id = file
                            .next_id
                            .checked_add(1)
                            .ok_or("Content profile identifiers exhausted")?;
                        file.profiles.push(profile.clone());
                        profile
                    }
                }
            }
            Edit::Delete { id } => {
                let index = file
                    .profiles
                    .iter()
                    .position(|profile| profile.id == id)
                    .ok_or("Choose an existing content profile to delete")?;
                file.profiles.remove(index)
            }
        };
        state.publish(file)?;
        Ok((profile, state.snapshot()))
    }
}

impl Registry {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            profiles: self.file.profiles.clone(),
            revision: self.revision,
            path: self.path.clone(),
            blocked: self.blocked,
            error: self.error.clone(),
        }
    }

    fn check_revision(&self, expected: Option<u64>) -> Result<(), String> {
        if expected.is_some_and(|revision| revision != self.revision) {
            return Err(
                "Content profiles changed; refresh the list before applying this action".into(),
            );
        }
        Ok(())
    }

    fn ensure_available(&self) -> Result<(), String> {
        if self.blocked {
            return Err("Content profiles are blocked; fix storage and Reload profiles before using saved profiles".into());
        }
        if self.path.is_none() || self.disk.is_none() {
            return Err("Content profile storage is unavailable; Reload profiles before changing saved profiles".into());
        }
        Ok(())
    }

    fn accept(&mut self, file: File, disk: ObservedSource) {
        self.file = file;
        self.disk = Some(disk);
        self.blocked = false;
        self.error = None;
    }

    fn block(&mut self, error: String) -> String {
        let error = map_core::entity_helpers::bounded_preview(&error, MAX_ERROR_BYTES);
        self.blocked = true;
        self.disk = None;
        self.error = Some(error.clone());
        error
    }

    fn check_current(&mut self) -> Result<(), String> {
        self.ensure_available()?;
        let path = self.path.as_deref().expect("storage availability checked");
        let expected = self.disk.as_ref().expect("storage availability checked");
        let current = target(path)?.observe_following_from(expected);
        let error = match current {
            Ok(current) if current.source == expected.source => return Ok(()),
            Ok(_) => {
                "Content profiles changed on disk; Reload profiles before using saved profiles"
                    .into()
            }
            Err(error) => format!(
                "Content profiles could not verify saved profiles; Reload profiles before using saved profiles: {error}"
            ),
        };
        self.revision += 1;
        Err(self.block(error))
    }

    fn publish(&mut self, file: File) -> Result<(), String> {
        validate(&file)?;
        self.ensure_available()?;
        let bytes = serde_json::to_vec_pretty(&file).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("Content profiles exceed the storage limit".into());
        }
        let publication = target(self.path.as_deref().expect("storage availability checked"))?
            .publish_following(
                &bytes,
                self.disk.as_ref().expect("storage availability checked"),
            );
        match publication {
            Ok(publication) => match publication.source {
                Ok(disk) => {
                    self.accept(file, disk);
                    self.revision += 1;
                    Ok(())
                }
                Err(error) => {
                    self.revision += 1;
                    Err(self.block(format!(
                        "The content profile file was written but verification failed; Reload profiles before using saved profiles: {error}"
                    )))
                }
            },
            Err(error) => {
                self.revision += 1;
                let message = if error.conflict {
                    format!(
                        "Content profiles changed on disk; Reload profiles before using saved profiles: {}",
                        error.message
                    )
                } else {
                    format!(
                        "Content profiles could not save; Reload profiles before using saved profiles: {}",
                        error.message
                    )
                };
                Err(self.block(message))
            }
        }
    }
}
