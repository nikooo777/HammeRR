use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{Value, json};

use crate::preference_file::{ObservedSource, Publication, SaveError};

#[derive(Clone, Default)]
pub struct SavedDefaults {
    pub(crate) grid: Authority<crate::grid_preferences::Store>,
    pub(crate) pivot: Authority<crate::pivot_preferences::Store>,
    pub(crate) preview: Authority<crate::preview_preferences::Store>,
    pub(crate) navigation: Authority<crate::navigation_preferences::Store>,
    pub(crate) selection: Authority<crate::selection_preferences::Store>,
}

impl SavedDefaults {
    pub fn configured(directory: Result<PathBuf, String>) -> Self {
        let defaults = Self::default();
        let _ = defaults.grid.configure(directory.clone());
        let _ = defaults.pivot.configure(directory.clone());
        let _ = defaults.preview.configure(directory.clone());
        let _ = defaults.navigation.configure(directory.clone());
        let _ = defaults.selection.configure(directory);
        defaults
    }
}

pub(crate) trait Store: Send {
    type Settings: Clone + Default + Serialize + Send;
    const LABEL: &'static str;
    const CHANGED: &'static str;
    const UNREADY: &'static str;
    fn new(directory: PathBuf) -> Self;
    fn path(&self) -> PathBuf;
    fn observe(&self) -> Result<ObservedSource, String>;
    fn current(&self, expected: &ObservedSource) -> Result<ObservedSource, String>;
    fn parse(&self, source: &ObservedSource) -> Result<Option<Self::Settings>, String>;
    fn publish(
        &self,
        settings: Self::Settings,
        reset: bool,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError>;
}

macro_rules! store {
    ($module:ident, $settings:ident, $label:literal, $changed:literal) => {
        impl Store for crate::$module::Store {
            type Settings = crate::$module::$settings;
            const LABEL: &'static str = $label;
            const CHANGED: &'static str = $changed;
            const UNREADY: &'static str = concat!(
                $label,
                " did not load; Reload defaults or explicitly Reset saved defaults first"
            );
            fn new(directory: PathBuf) -> Self {
                Self { directory }
            }
            fn path(&self) -> PathBuf {
                self.path()
            }
            fn observe(&self) -> Result<ObservedSource, String> {
                self.read_source()
            }
            fn current(&self, expected: &ObservedSource) -> Result<ObservedSource, String> {
                self.read_current(expected)
            }
            fn parse(&self, source: &ObservedSource) -> Result<Option<Self::Settings>, String> {
                self.parse_source(source)
            }
            fn publish(
                &self,
                settings: Self::Settings,
                reset: bool,
                expected: &ObservedSource,
            ) -> Result<Publication, SaveError> {
                self.save_checked(settings, reset, expected)
            }
        }
    };
}
store!(
    grid_preferences,
    Grid,
    "Grid defaults",
    "Saved grid defaults changed outside this session; Reload defaults before using them"
);
store!(
    preview_preferences,
    Preview,
    "Preview defaults",
    "Saved preview defaults changed outside this session; Reload defaults before using them"
);
store!(
    navigation_preferences,
    Navigation,
    "Navigation defaults",
    "Saved navigation defaults changed outside this session; Reload defaults before using them"
);
store!(
    selection_preferences,
    SelectionDefaults,
    "Selection defaults",
    "Saved selection defaults changed outside this session; Reload defaults before using them"
);

store!(
    pivot_preferences,
    Pivot,
    "Transform pivot defaults",
    "Saved transform pivot defaults changed outside this session; Reload defaults before using them"
);

pub(crate) struct Authority<S: Store> {
    state: Arc<Mutex<State<S>>>,
}

impl<S: Store> Clone for Authority<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<S: Store> Default for Authority<S> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                store: Err(format!("{} storage is not configured", S::LABEL)),
                saved: None,
                loaded: false,
                error: None,
                source: None,
                serial: 0,
            })),
        }
    }
}

struct State<S: Store> {
    store: Result<S, String>,
    saved: Option<S::Settings>,
    loaded: bool,
    error: Option<String>,
    source: Option<ObservedSource>,
    serial: u64,
}

#[derive(Clone)]
pub(crate) struct Snapshot<T> {
    pub saved: Option<T>,
    pub loaded: bool,
    pub error: Option<String>,
    pub storage_error: Option<String>,
    pub path: Option<PathBuf>,
    pub serial: u64,
    #[cfg(test)]
    pub source: Option<ObservedSource>,
}

impl<T: Copy + Default + Serialize> Snapshot<T> {
    pub fn report(&self, current: T, snapshot: String) -> Value {
        json!({"snapshot":snapshot,"current":current,"defaults":self.loaded.then(||self.saved.unwrap_or_default()),
            "saved":self.saved,"stored":self.loaded && self.saved.is_some(),"ready":self.loaded,"error":self.error,
            "path":self.path,"storage_error":self.storage_error,"factory_defaults":T::default()})
    }
}

impl<S: Store> Authority<S> {
    fn lock(&self) -> MutexGuard<'_, State<S>> {
        self.state
            .lock()
            .expect("Saved defaults authority was poisoned")
    }

    pub fn snapshot(&self) -> Snapshot<S::Settings> {
        self.lock().snapshot()
    }

    pub fn configure(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        state.store = directory.map(S::new);
        state.reload()
    }

    pub fn reload(&self, serial: u64, stale: &str) -> Result<(), String> {
        let mut state = self.lock();
        state.check_revision(serial, stale)?;
        state.reload()
    }

    pub fn checked_value(&self, serial: u64, stale: &str) -> Result<S::Settings, String> {
        let mut state = self.lock();
        state.check_revision(serial, stale)?;
        if !state.loaded {
            return Err(S::UNREADY.into());
        }
        state.check_source()?;
        Ok(state.saved.clone().unwrap_or_default())
    }

    pub fn save(
        &self,
        serial: u64,
        stale: &str,
        settings: S::Settings,
        reset: bool,
    ) -> Result<(), String> {
        self.update(serial, stale, reset, |_| Ok(settings))
            .map(|_| ())
    }

    pub fn update(
        &self,
        serial: u64,
        stale: &str,
        reset: bool,
        edit: impl FnOnce(Option<&S::Settings>) -> Result<S::Settings, String>,
    ) -> Result<Snapshot<S::Settings>, String> {
        let mut state = self.lock();
        state.check_revision(serial, stale)?;
        if !reset && !state.loaded {
            return Err(S::UNREADY.into());
        }
        state.check_source()?;
        let settings = edit(state.saved.as_ref())?;
        let result = state
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| {
                let expected = state
                    .source
                    .as_ref()
                    .ok_or("Inspect defaults before writing")?;
                store
                    .publish(settings.clone(), reset, expected)
                    .map_err(|error| error.message)
            });
        match result {
            Ok(publication) => {
                state.accept(settings, publication);
                Ok(state.snapshot())
            }
            Err(error) => Err(state.fail(error)),
        }
    }

    #[cfg(test)]
    pub fn accept(&self, settings: S::Settings, publication: Publication) {
        self.lock().accept(settings, publication);
    }

    pub fn fail(&self, error: String) -> String {
        self.lock().fail(error)
    }
}

impl<S: Store> State<S> {
    fn snapshot(&self) -> Snapshot<S::Settings> {
        Snapshot {
            saved: self.saved.clone(),
            loaded: self.loaded,
            error: self.error.clone(),
            storage_error: self.store.as_ref().err().cloned(),
            path: self.store.as_ref().ok().map(Store::path),
            serial: self.serial,
            #[cfg(test)]
            source: self.source.clone(),
        }
    }

    fn fail(&mut self, error: String) -> String {
        self.loaded = false;
        self.error = Some(error.clone());
        self.serial += 1;
        error
    }

    fn check_revision(&self, serial: u64, stale: &str) -> Result<(), String> {
        if self.serial != serial {
            Err(stale.into())
        } else {
            Ok(())
        }
    }

    fn reload(&mut self) -> Result<(), String> {
        let result = self.store.as_ref().map_err(Clone::clone).and_then(|store| {
            let source = store.observe()?;
            let result = store.parse(&source);
            self.source = Some(source);
            result
        });
        match result {
            Ok(saved) => {
                self.saved = saved;
                self.loaded = true;
                self.error = None;
                self.serial += 1;
                Ok(())
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    fn check_source(&mut self) -> Result<(), String> {
        let result =
            self.store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|store| match &self.source {
                    Some(expected) => store.current(expected),
                    None => store.observe(),
                });
        let error = match result {
            Ok(current)
                if self
                    .source
                    .as_ref()
                    .is_some_and(|source| source.source == current.source) =>
            {
                return Ok(());
            }
            Ok(current) => {
                self.source = Some(current);
                S::CHANGED.into()
            }
            Err(error) => error,
        };
        Err(self.fail(error))
    }

    fn accept(&mut self, settings: S::Settings, publication: Publication) {
        self.saved = Some(settings);
        match publication.source {
            Ok(source) => {
                self.source = Some(source);
                self.loaded = true;
                self.error = None;
                self.serial += 1;
            }
            Err(error) => {
                self.fail(error);
            }
        }
    }
}

pub(crate) enum Binding<S: Store> {
    Independent(Authority<S>),
    Application(Authority<S>),
}

impl<S: Store> Default for Binding<S> {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl<S: Store> Binding<S> {
    pub fn authority(&self) -> &Authority<S> {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
    pub fn configure(&self, directory: Result<PathBuf, String>) -> Option<Result<(), String>> {
        match self {
            Self::Independent(authority) => Some(authority.configure(directory)),
            Self::Application(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
