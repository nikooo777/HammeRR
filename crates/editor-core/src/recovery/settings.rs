use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{MAX_SETTINGS_BYTES, Settings, Store};
use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};

const FILE_NAME: &str = "settings.json";
const LOCK_FILE_NAME: &str = ".recovery-settings.lock";

impl Store {
    fn settings_target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: LOCK_FILE_NAME,
            label: "Recovery settings",
            reload: "Reload settings",
            saved_description: "saved settings",
            max_bytes: MAX_SETTINGS_BYTES,
        }
    }

    fn settings_source(&self) -> Result<ObservedSource, String> {
        self.settings_target().observe_following()
    }

    fn parse_settings(&self, source: &ObservedSource) -> Result<Settings, String> {
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok(Settings::default());
        };
        let path = self.directory.join(FILE_NAME);
        if *length > MAX_SETTINGS_BYTES {
            return Err(format!(
                "{} must be a regular file of at most {MAX_SETTINGS_BYTES} bytes",
                path.display()
            ));
        }
        let settings: Settings = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn load_settings(&self) -> Result<Settings, String> {
        self.parse_settings(&self.settings_source()?)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        settings.validate()?;
        let source = self.settings_source()?;
        self.save_settings_checked(settings, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    fn save_settings_checked(
        &self,
        settings: &Settings,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        settings.validate().map_err(SaveError::failure)?;
        let data = serde_json::to_vec_pretty(settings)
            .map_err(|error| SaveError::failure(error.to_string()))?;
        self.settings_target().publish_following(&data, expected)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsSnapshot {
    pub settings: Settings,
    pub revision: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SettingsUpdate {
    pub enabled: Option<bool>,
    pub interval_seconds: Option<u64>,
    pub keep_per_map: Option<usize>,
}

impl From<Settings> for SettingsUpdate {
    fn from(settings: Settings) -> Self {
        Self {
            enabled: Some(settings.enabled),
            interval_seconds: Some(settings.interval_seconds),
            keep_per_map: Some(settings.keep_per_map),
        }
    }
}

#[derive(Default)]
struct State {
    store: Option<Store>,
    settings: Settings,
    error: Option<String>,
    unverified_write: Option<String>,
    source: Option<ObservedSource>,
    reload_required: bool,
    revision: u64,
}

impl State {
    fn snapshot(&self) -> SettingsSnapshot {
        SettingsSnapshot {
            settings: self.settings.clone(),
            revision: self.revision,
            error: self.error(),
        }
    }

    fn error(&self) -> Option<String> {
        match (&self.unverified_write, &self.error) {
            (Some(committed), Some(error)) if committed != error => {
                Some(format!("{committed}; {error}"))
            }
            (Some(error), _) | (None, Some(error)) => Some(error.clone()),
            (None, None) => None,
        }
    }

    fn read_settings(&mut self) -> Result<Settings, String> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| "Recovery storage is unavailable".to_owned())?;
        match store.settings_source() {
            Ok(source) => {
                let settings = store.parse_settings(&source);
                self.source = Some(source);
                self.reload_required = false;
                settings
            }
            Err(error) => {
                self.source = None;
                self.reload_required = true;
                Err(error)
            }
        }
    }

    fn accept(
        &mut self,
        settings: Settings,
        publication: Publication,
    ) -> Result<SettingsSnapshot, String> {
        self.settings = settings;
        self.error = None;
        self.advance();
        match publication.source {
            Ok(source) => {
                self.source = Some(source);
                self.reload_required = false;
                self.unverified_write = None;
                Ok(self.snapshot())
            }
            Err(error) => {
                self.source = None;
                self.reload_required = true;
                self.unverified_write = Some(error.clone());
                Err(error)
            }
        }
    }

    fn advance(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("Recovery settings revision exhausted");
    }

    fn check_revision(&self, expected: Option<u64>) -> Result<(), String> {
        if expected.is_some_and(|revision| revision != self.revision) {
            return Err(
                "Recovery settings changed elsewhere; reload the settings before applying".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct Authority(Arc<Mutex<State>>);

pub(crate) enum ReloadError {
    Stale(String),
    Storage(String),
}

impl Authority {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .expect("Recovery settings authority lock poisoned")
    }

    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure(directory);
        authority
    }

    pub(crate) fn configure(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        let directory = match directory.and_then(super::admit_directory) {
            Ok(directory) => directory,
            Err(error) => {
                state.store = None;
                state.source = None;
                state.reload_required = true;
                state.error = Some(error.clone());
                state.advance();
                return Err(error);
            }
        };
        state.store = Some(Store { directory });
        let loaded = state.read_settings();
        state.advance();
        match loaded {
            Ok(settings) => {
                state.settings = settings;
                state.error = None;
                state.unverified_write = None;
                Ok(())
            }
            Err(error) => {
                state.settings.enabled = false;
                state.error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn snapshot(&self) -> SettingsSnapshot {
        self.lock().snapshot()
    }

    pub(crate) fn update(
        &self,
        update: SettingsUpdate,
        expected_revision: Option<u64>,
    ) -> Result<SettingsSnapshot, String> {
        self.update_with(update, expected_revision, Store::save_settings_checked)
    }

    fn update_with(
        &self,
        update: SettingsUpdate,
        expected_revision: Option<u64>,
        publish: impl FnOnce(&Store, &Settings, &ObservedSource) -> Result<Publication, SaveError>,
    ) -> Result<SettingsSnapshot, String> {
        let mut state = self.lock();
        state.check_revision(expected_revision)?;
        if state.reload_required {
            return Err(state
                .error()
                .unwrap_or_else(|| "Reload settings before applying recovery settings".into()));
        }
        let mut settings = state.settings.clone();
        if let Some(enabled) = update.enabled {
            settings.enabled = enabled;
        }
        if let Some(interval) = update.interval_seconds {
            settings.interval_seconds = interval;
        }
        if let Some(keep) = update.keep_per_map {
            settings.keep_per_map = keep;
        }
        let saved = state
            .store
            .as_ref()
            .ok_or_else(|| "Recovery storage is unavailable".to_owned())
            .map_err(SaveError::failure)
            .and_then(|store| {
                let expected = state.source.as_ref().ok_or_else(|| {
                    SaveError::failure("Reload settings before applying recovery settings")
                })?;
                publish(store, &settings, expected)
            });
        match saved {
            Ok(publication) => state.accept(settings, publication),
            Err(error) => {
                state.reload_required |= error.conflict;
                state.error = Some(error.message);
                Err(state.error().expect("Recovery settings Save error missing"))
            }
        }
    }

    pub(crate) fn reload(
        &self,
        expected_revision: Option<u64>,
    ) -> Result<SettingsSnapshot, ReloadError> {
        let mut state = self.lock();
        state
            .check_revision(expected_revision)
            .map_err(ReloadError::Stale)?;
        let loaded = state.read_settings();
        match loaded {
            Ok(settings) => {
                state.settings = settings;
                state.error = None;
                state.unverified_write = None;
                state.advance();
                Ok(state.snapshot())
            }
            Err(error) => {
                let error = format!("Recovery settings could not reload: {error}");
                state.error = Some(error);
                Err(ReloadError::Storage(
                    state.error().expect("Recovery Reload error missing"),
                ))
            }
        }
    }
}
