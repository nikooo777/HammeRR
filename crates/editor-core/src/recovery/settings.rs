use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{Settings, Store};

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
    revision: u64,
}

impl State {
    fn snapshot(&self) -> SettingsSnapshot {
        SettingsSnapshot {
            settings: self.settings.clone(),
            revision: self.revision,
            error: self.error.clone(),
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
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                state.error = Some(error.clone());
                return Err(error);
            }
        };
        let store = Store { directory };
        let loaded = store.load_settings();
        state.store = Some(store);
        state.advance();
        match loaded {
            Ok(settings) => {
                state.settings = settings;
                state.error = None;
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
        let mut state = self.lock();
        state.check_revision(expected_revision)?;
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
            .and_then(|store| store.save_settings(&settings));
        if let Err(error) = saved {
            state.error = Some(error.clone());
            return Err(error);
        }
        state.settings = settings;
        state.error = None;
        state.advance();
        Ok(state.snapshot())
    }

    pub(crate) fn reload(
        &self,
        expected_revision: Option<u64>,
    ) -> Result<SettingsSnapshot, ReloadError> {
        let mut state = self.lock();
        state
            .check_revision(expected_revision)
            .map_err(ReloadError::Stale)?;
        let loaded = state
            .store
            .as_ref()
            .ok_or_else(|| "Recovery storage is unavailable".to_owned())
            .and_then(Store::load_settings);
        match loaded {
            Ok(settings) => {
                state.settings = settings;
                state.error = None;
                state.advance();
                Ok(state.snapshot())
            }
            Err(error) => {
                let error = format!("Recovery settings could not reload: {error}");
                state.error = Some(error.clone());
                Err(ReloadError::Storage(error))
            }
        }
    }
}
