use super::{Entry, Store};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

const UNCONFIGURED: &str = "Prefab storage is not configured";
const POISONED: &str = "Prefab registry lock is poisoned; restart the application";
const STALE: &str = "Prefab library changed; refresh it before continuing";

#[derive(Clone, PartialEq, Eq)]
struct Catalog {
    entries: Vec<Entry>,
    stamp: String,
    bytes: u64,
}

struct Registry {
    store: Result<Store, String>,
    catalog: Result<Catalog, String>,
    observation: Option<Arc<super::Snapshot>>,
    write_error: Option<String>,
    revision: u64,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            store: Err(UNCONFIGURED.into()),
            catalog: Err(UNCONFIGURED.into()),
            observation: None,
            write_error: None,
            revision: 1,
        }
    }
}

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<Registry>>,
}

#[derive(Clone)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub stamp: String,
    pub bytes: u64,
    pub revision: u64,
    pub directory: Option<PathBuf>,
    pub error: Option<String>,
    pub write_error: Option<String>,
    _observation: Option<Arc<super::Snapshot>>,
}

pub(crate) enum Edit<'a> {
    Save { name: &'a str, text: &'a str },
    Rename { name: &'a str, new_name: &'a str },
    Delete { name: &'a str },
}

fn directory(root: Result<PathBuf, String>) -> Result<PathBuf, String> {
    std::env::var_os("HAMMERR_PREFABS_DIR")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| root.map(|path| path.join("prefabs")))
}

impl Registry {
    fn observe(&mut self) {
        let observation = self
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::snapshot)
            .map(Arc::new);
        let catalog = observation
            .as_ref()
            .map_err(Clone::clone)
            .map(|snapshot| Catalog {
                entries: snapshot.entries.clone(),
                stamp: snapshot.stamp.clone(),
                bytes: snapshot.bytes,
            });
        if self.catalog != catalog {
            self.catalog = catalog;
            self.revision = self.revision.saturating_add(1);
        }
        self.observation = observation.ok();
    }

    fn snapshot(&self) -> Snapshot {
        let (entries, stamp, bytes, error) = match &self.catalog {
            Ok(catalog) => (
                catalog.entries.clone(),
                catalog.stamp.clone(),
                catalog.bytes,
                None,
            ),
            Err(error) => (Vec::new(), String::new(), 0, Some(error.clone())),
        };
        Snapshot {
            entries,
            stamp,
            bytes,
            revision: self.revision,
            directory: self
                .store
                .as_ref()
                .ok()
                .map(|store| store.directory.clone()),
            error,
            write_error: self.write_error.clone(),
            _observation: self.observation.clone(),
        }
    }

    fn checked(&mut self, revision: u64, stamp: &str) -> Result<(), String> {
        self.observe();
        let catalog = self.catalog.as_ref().map_err(Clone::clone)?;
        if self.revision != revision || catalog.stamp != stamp {
            return Err(STALE.into());
        }
        Ok(())
    }

    fn completed(&mut self, result: Result<super::Mutation, String>) -> Result<Snapshot, String> {
        let mutation = result?;
        self.write_error = mutation.warning;
        self.revision = self.revision.saturating_add(1);
        Ok(self.snapshot())
    }
}

impl Authority {
    pub fn configured(root: Result<PathBuf, String>) -> Self {
        Self::from_directory(directory(root))
    }

    pub fn from_directory(directory: Result<PathBuf, String>) -> Self {
        let authority = Self::default();
        authority.bind(directory).expect("new prefab registry lock");
        authority
    }

    fn lock(&self) -> Result<MutexGuard<'_, Registry>, String> {
        self.state.lock().map_err(|_| POISONED.into())
    }

    fn bind(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock()?;
        let previous = state
            .store
            .as_ref()
            .map(|store| &store.directory)
            .map_err(Clone::clone);
        if previous != directory.as_ref().map_err(Clone::clone) || state.write_error.is_some() {
            state.revision = state.revision.saturating_add(1);
        }
        state.write_error = None;
        state.store = directory.map(|directory| Store { directory });
        state.observe();
        Ok(())
    }

    pub(crate) fn configure(&self, root: Result<PathBuf, String>) -> Result<(), String> {
        self.bind(directory(root))
    }

    pub fn serial(&self) -> u64 {
        self.lock().map_or(u64::MAX, |state| state.revision)
    }

    pub fn snapshot(&self) -> Snapshot {
        match self.lock() {
            Ok(mut state) => {
                state.observe();
                state.snapshot()
            }
            Err(error) => Snapshot {
                entries: Vec::new(),
                stamp: String::new(),
                bytes: 0,
                revision: u64::MAX,
                directory: None,
                error: Some(error),
                write_error: None,
                _observation: None,
            },
        }
    }

    pub(crate) fn check(&self, revision: u64, stamp: &str) -> Result<(), String> {
        self.lock()?.checked(revision, stamp)
    }

    pub(crate) fn read(
        &self,
        name: &str,
        revision: u64,
        stamp: &str,
    ) -> Result<(Entry, String), String> {
        let mut state = self.lock()?;
        state.checked(revision, stamp)?;
        let entry = state
            .catalog
            .as_ref()
            .map_err(Clone::clone)?
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or("Prefab no longer exists; refresh the library")?
            .clone();
        let text = state
            .store
            .as_ref()
            .map_err(Clone::clone)?
            .read(name, stamp);
        state.observe();
        let text = text?;
        state.checked(revision, stamp)?;
        Ok((entry, text))
    }

    pub(crate) fn check_preview(&self, source: &Entry) -> Result<(), String> {
        let mut state = self.lock()?;
        state.observe();
        state.catalog.as_ref().map_err(Clone::clone)?;
        let result = state
            .store
            .as_ref()
            .map_err(Clone::clone)?
            .check_preview(source);
        state.observe();
        result
    }

    pub(crate) fn change(
        &self,
        revision: u64,
        stamp: &str,
        edit: Edit<'_>,
    ) -> Result<Snapshot, String> {
        let mut state = self.lock()?;
        state.checked(revision, stamp)?;
        let store = state.store.as_ref().map_err(Clone::clone)?;
        let result = match edit {
            Edit::Save { name, text } => store.save(name, text, stamp),
            Edit::Rename { name, new_name } => store.rename(name, new_name, stamp),
            Edit::Delete { name } => store.remove(name, stamp),
        };
        state.observe();
        state.completed(result)
    }
}
