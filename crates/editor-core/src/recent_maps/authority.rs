use super::*;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    store: Option<Store>,
    load_failed: bool,
    error: Option<String>,
    serial: u64,
    source: Option<ObservedSource>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub directory: Option<PathBuf>,
    pub blocked: bool,
    pub error: Option<String>,
    pub serial: u64,
}

impl Snapshot {
    pub fn persistence(&self) -> &'static str {
        if self.blocked {
            "blocked"
        } else if self.directory.is_some() {
            "persistent"
        } else {
            "session"
        }
    }
}

impl Authority {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure(directory);
        authority
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("Recent maps authority poisoned")
    }

    pub(crate) fn configure(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        let store = directory.map(|directory| Store { directory });
        let loaded = match &store {
            Ok(store) => store.load_snapshot(),
            Err(error) => Err(error.clone()),
        };
        state.store = store.ok();
        match loaded {
            Ok((entries, source)) => {
                state.entries = entries;
                state.source = Some(source);
                state.load_failed = false;
                state.error = None;
                state.serial += 1;
                Ok(())
            }
            Err(error) => {
                state.source = None;
                state.load_failed = true;
                Err(state.error(format!("Recent maps could not load: {error}")))
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.lock().snapshot()
    }

    pub(crate) fn serial(&self) -> u64 {
        self.lock().serial
    }

    pub(crate) fn blocked(&self) -> bool {
        self.lock().load_failed
    }

    pub(crate) fn entry(&self, path: &Path) -> Option<Entry> {
        self.lock()
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
    }

    pub(crate) fn report_error(&self, error: String) -> String {
        self.lock().error(error)
    }

    pub(crate) fn reload(&self) -> Result<Snapshot, String> {
        let mut state = self.lock();
        let loaded = state
            .store
            .as_ref()
            .ok_or_else(|| "Recent maps storage is unavailable".to_owned())
            .and_then(Store::load_snapshot);
        match loaded {
            Ok((entries, source)) => {
                state.entries = entries;
                state.source = Some(source);
                state.load_failed = false;
                state.error = None;
                state.serial += 1;
                Ok(state.snapshot())
            }
            Err(error) => {
                state.load_failed = true;
                Err(state.error(format!("Recent maps could not reload: {error}")))
            }
        }
    }

    pub(crate) fn remember(
        &self,
        mut entry: Entry,
        reorder: bool,
        preserve_incompatible: bool,
        compiling: bool,
    ) -> Result<(), String> {
        let mut state = self.lock();
        if state.load_failed {
            return Ok(());
        }
        let existing = state
            .entries
            .iter()
            .find(|existing| existing.path == entry.path);
        if !reorder && existing.is_none() {
            return Ok(());
        }
        if let Some(existing) = existing.filter(|_| !reorder) {
            entry.reopen_path = existing.reopen_path.clone();
        }
        if let Some(existing) = existing.filter(|existing| {
            compiling || (preserve_incompatible && existing.game_directory != entry.game_directory)
        }) {
            entry.game_directory = existing.game_directory.clone();
            entry.build_settings = existing.build_settings.clone();
        }
        let mut entries = state.entries.clone();
        if reorder {
            entries.retain(|existing| existing.path != entry.path);
            entries.insert(0, entry);
            entries.truncate(MAX_RECENT_MAPS);
        } else if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.path == entry.path)
        {
            *existing = entry;
        }
        if entries == state.entries && state.error.is_none() {
            return Ok(());
        }
        state.publish(entries)
    }

    pub(crate) fn remove(&self, path: &Path) -> Result<(), String> {
        let mut state = self.lock();
        state.ensure_writable()?;
        let mut entries = state.entries.clone();
        entries.retain(|entry| entry.path != path);
        if entries.len() == state.entries.len() {
            return Err("This map is no longer in Recent maps; refresh the list".into());
        }
        state.publish(entries)
    }

    pub(crate) fn clear(&self) -> Result<(), String> {
        let mut state = self.lock();
        state.ensure_writable()?;
        if state.entries.is_empty() && state.error.is_none() {
            return Ok(());
        }
        state.publish(Vec::new())
    }
}

impl State {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            entries: self.entries.clone(),
            directory: self.store.as_ref().map(|store| store.directory.clone()),
            blocked: self.load_failed,
            error: self.error.clone(),
            serial: self.serial,
        }
    }

    fn error(&mut self, error: String) -> String {
        let error = map_core::entity_helpers::bounded_preview(&error, MAX_ERROR_BYTES);
        self.error = Some(error.clone());
        self.serial += 1;
        error
    }

    fn ensure_writable(&self) -> Result<(), String> {
        if self.load_failed {
            return Err(map_core::entity_helpers::bounded_preview(
                &format!(
                    "{}; fix storage and Reload saved list before changing recent maps",
                    self.error.as_deref().unwrap_or("Recent maps did not load")
                ),
                MAX_ERROR_BYTES,
            ));
        }
        Ok(())
    }

    fn publish(&mut self, entries: Vec<Entry>) -> Result<(), String> {
        self.ensure_writable()?;
        if let Some(store) = &self.store {
            let expected = self
                .source
                .as_ref()
                .ok_or("Reload saved list before changing recent maps")?;
            match store.save_checked(&entries, expected) {
                Ok(publication) => {
                    self.entries = entries;
                    match publication.source {
                        Ok(source) => self.source = Some(source),
                        Err(error) => {
                            self.source = None;
                            self.load_failed = true;
                            return Err(self.error(error));
                        }
                    }
                }
                Err(error) => {
                    if error.conflict {
                        self.load_failed = true;
                    }
                    return Err(
                        self.error(format!("Recent maps could not save: {}", error.message))
                    );
                }
            }
        } else {
            self.entries = entries;
        }
        self.error = None;
        self.serial += 1;
        Ok(())
    }
}
