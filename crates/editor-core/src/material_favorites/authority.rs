use super::*;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    materials: BTreeSet<String>,
    store: Option<Store>,
    load_failed: bool,
    error: Option<String>,
    source: Option<ObservedSource>,
    serial: u64,
}

pub struct Snapshot {
    pub materials: BTreeSet<String>,
    pub status: Status,
    pub serial: u64,
}

impl Snapshot {
    pub fn contains(&self, material: &str) -> bool {
        canonical_name(material).is_ok_and(|name| self.materials.contains(&name))
    }
}

impl Authority {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        let _ = authority.configure(directory);
        authority
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("Material favorites authority was poisoned")
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
            Ok((materials, source)) => {
                state.materials = materials;
                state.source = Some(source);
                state.load_failed = false;
                state.error = None;
                state.serial += 1;
                Ok(())
            }
            Err(error) => {
                state.source = None;
                state.load_failed = true;
                Err(state.error(format!("Material favorites could not load: {error}")))
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let state = self.lock();
        Snapshot {
            materials: state.materials.clone(),
            status: state.status(),
            serial: state.serial,
        }
    }

    pub(crate) fn serial(&self) -> u64 {
        self.lock().serial
    }

    pub(crate) fn status(&self) -> Status {
        self.lock().status()
    }

    pub(crate) fn contains(&self, material: &str) -> bool {
        canonical_name(material).is_ok_and(|name| self.lock().materials.contains(&name))
    }

    pub(crate) fn reload(&self) -> Result<Status, String> {
        let mut state = self.lock();
        let store = state.store.as_ref().ok_or(RELOAD_UNAVAILABLE)?;
        match store.load_snapshot() {
            Ok((materials, source)) => {
                state.materials = materials;
                state.source = Some(source);
                state.load_failed = false;
                state.error = None;
                state.serial += 1;
                Ok(state.status())
            }
            Err(error) => {
                state.load_failed = true;
                Err(state.error(format!("Material favorites could not reload: {error}")))
            }
        }
    }

    pub(crate) fn set(&self, material: &str, favorite: bool) -> Result<(), String> {
        let mut state = self.lock();
        if state.load_failed {
            let error = format!(
                "{}; fix storage and Reload favorites before changing favorites",
                state
                    .error
                    .as_deref()
                    .unwrap_or("Material favorites did not load")
            );
            return Err(map_core::entity_helpers::bounded_preview(
                &error,
                MAX_ERROR_BYTES,
            ));
        }
        let name = canonical_name(material).map_err(|error| state.error(error))?;
        if state.materials.contains(&name) == favorite {
            return Ok(());
        }
        let mut materials = state.materials.clone();
        if favorite {
            if materials.len() == MAX_FAVORITES {
                return Err(state.error(format!("Keep at most {MAX_FAVORITES} material favorites")));
            }
            materials.insert(name);
        } else {
            materials.remove(&name);
        }
        if let Some(store) = &state.store {
            let expected = state
                .source
                .as_ref()
                .ok_or("Reload favorites before changing favorites")?;
            match store.save_checked(&materials, expected) {
                Ok(publication) => {
                    state.materials = materials;
                    match publication.source {
                        Ok(source) => state.source = Some(source),
                        Err(error) => {
                            state.source = None;
                            state.load_failed = true;
                            return Err(state.error(error));
                        }
                    }
                }
                Err(error) => {
                    if error.conflict {
                        state.load_failed = true;
                    }
                    return Err(state.error(format!(
                        "Material favorites could not save: {}",
                        error.message
                    )));
                }
            }
        } else {
            state.materials = materials;
        }
        state.error = None;
        state.serial += 1;
        Ok(())
    }
}

impl State {
    fn error(&mut self, error: String) -> String {
        let error = map_core::entity_helpers::bounded_preview(&error, MAX_ERROR_BYTES);
        self.error = Some(error.clone());
        self.serial += 1;
        error
    }

    fn status(&self) -> Status {
        let mut status = Status::session(self.materials.len());
        status.directory = self.store.as_ref().map(|store| store.directory.clone());
        status.error = self.error.clone();
        status.reload_available = self.store.is_some();
        status.persistence = if self.load_failed {
            Persistence::Blocked
        } else if self.store.is_some() {
            Persistence::Persistent
        } else {
            Persistence::Session
        };
        status
    }
}
