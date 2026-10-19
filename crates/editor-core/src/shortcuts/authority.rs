use super::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Default)]
pub struct Authority {
    state: Arc<Mutex<State>>,
}

pub struct Snapshot {
    pub current: Bindings,
    pub error: Option<String>,
    pub ready: bool,
    pub stored: bool,
    pub path: Option<PathBuf>,
    pub serial: u64,
}

impl Snapshot {
    pub fn stamp(&self) -> String {
        self.serial.to_string()
    }

    pub fn report(&self) -> Value {
        let factory = Bindings::default();
        let rows = self.current.command_rows();
        json!({"snapshot":self.stamp(),"current":self.current,"factory_defaults":factory,
            "ready":self.ready,"stored":self.stored,"error":self.error,"path":self.path,"commands":rows,
            "limits":{"slots_per_command":2,"file_bytes":MAX_FILE_BYTES},
            "scope":"Viewport focus only. Logical A–Z, F1–F12, Delete and Backspace with exact Ctrl/Shift modifiers; Alt/Meta are unavailable. Escape, arrows/PageUp/PageDown, camera W/A/S/D/Q/E (including Shift) and Ctrl+N/O/S/C/X/V are reserved. Text fields keep native editing shortcuts. Save and Reset persist and activate; failed Reload preserves current bindings. No map or Undo changes."})
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
            .expect("Keyboard shortcuts authority was poisoned")
    }

    pub(crate) fn configure(&self, directory: Result<PathBuf, String>) -> Result<(), String> {
        let mut state = self.lock();
        state.store = directory.map(|directory| Store { directory });
        state.reload()
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.lock().snapshot()
    }

    pub(crate) fn serial(&self) -> u64 {
        self.lock().serial
    }

    pub(crate) fn resolve(&self, chord: &Chord) -> Option<Command> {
        self.lock().current.resolve(chord)
    }

    pub(crate) fn change(
        &self,
        snapshot: &str,
        action: &str,
        bindings: Option<Bindings>,
    ) -> Result<Value, String> {
        self.lock().change(snapshot, action, bindings)
    }
}

struct State {
    store: Result<Store, String>,
    current: Bindings,
    source: Option<ObservedSource>,
    error: Option<String>,
    loaded: bool,
    stored: bool,
    serial: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: Err("Keyboard shortcut storage is not configured".into()),
            current: Bindings::default(),
            source: None,
            error: None,
            loaded: false,
            stored: false,
            serial: 0,
        }
    }
}

impl State {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            current: self.current.clone(),
            error: self
                .error
                .clone()
                .or_else(|| self.store.as_ref().err().cloned()),
            ready: self.loaded,
            stored: self.loaded && self.stored,
            path: self.store.as_ref().ok().map(Store::path),
            serial: self.serial,
        }
    }

    fn error(&mut self, error: String) -> String {
        self.loaded = false;
        self.error = Some(error.clone());
        self.serial += 1;
        error
    }

    fn reload(&mut self) -> Result<(), String> {
        let read = self
            .store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(Store::read_source);
        let result = match read {
            Ok(source) => {
                let result = self.store.as_ref().unwrap().parse(&source);
                self.source = Some(source);
                result
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(saved) => {
                self.stored = saved.is_some();
                self.current = saved.unwrap_or_default();
                self.loaded = true;
                self.error = None;
                self.serial += 1;
                Ok(())
            }
            Err(error) => Err(self.error(error)),
        }
    }

    fn accept_publication(
        &mut self,
        current: Bindings,
        publication: crate::preference_file::Publication,
    ) {
        self.current = current;
        self.stored = true;
        match publication.source {
            Ok(source) => {
                self.source = Some(source);
                self.loaded = true;
                self.error = None;
                self.serial += 1;
            }
            Err(error) => {
                self.error(error);
            }
        }
    }

    fn check_source(&mut self) -> Result<(), String> {
        let read = self.store.as_ref().map_err(Clone::clone).and_then(|store| {
            match self.source.as_ref() {
                Some(expected) => store.read_current(expected),
                None => store.read_source(),
            }
        });
        let error = match read {
            Ok(source)
                if self
                    .source
                    .as_ref()
                    .is_some_and(|expected| expected.source == source.source) =>
            {
                return Ok(());
            }
            Ok(source) => {
                self.source = Some(source);
                "Saved shortcuts changed outside this session; Reload saved or explicitly Reset saved shortcuts".into()
            }
            Err(error) => error,
        };
        Err(self.error(error))
    }

    fn change(
        &mut self,
        snapshot: &str,
        action: &str,
        bindings: Option<Bindings>,
    ) -> Result<Value, String> {
        if snapshot != self.serial.to_string() {
            return Err("Keyboard shortcuts changed; Reload saved before saving this form".into());
        }
        let mut write_committed = false;
        match action {
            "reload" if bindings.is_none() => self.reload()?,
            "save" | "reset" => {
                let current = match (action, bindings) {
                    ("save", Some(bindings)) => bindings.validated()?,
                    ("reset", None) => Bindings::default(),
                    _ => {
                        return Err("Save requires bindings; Reset does not accept bindings".into());
                    }
                };
                if action == "save" && !self.loaded {
                    return Err("Saved shortcuts unavailable; Reload saved must succeed or explicitly Reset saved shortcuts first".into());
                }
                self.check_source()?;
                let result = self.store.as_ref().map_err(Clone::clone).and_then(|store| {
                    let expected = self
                        .source
                        .as_ref()
                        .ok_or("Inspect saved shortcuts before writing")?;
                    store
                        .save_checked(current.clone(), expected)
                        .map_err(|error| error.message)
                });
                let publication = match result {
                    Ok(publication) => publication,
                    Err(error) => return Err(self.error(error)),
                };
                write_committed = true;
                self.accept_publication(current, publication);
            }
            _ => return Err("Choose save, reload or reset for keyboard shortcuts".into()),
        }
        let mut state = self.snapshot().report();
        state["write_committed"] = json!(write_committed);
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
