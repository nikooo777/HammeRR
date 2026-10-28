use serde::Serialize;

use super::worker::{Slot, Ticket};
use super::{Actor, Session};
use source_formats::fgd::Catalog;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct LoadState {
    worker: Slot,
    serial: u64,
    identity: Option<Arc<()>>,
    cancelled: Option<Arc<AtomicBool>>,
}

pub struct Request {
    path: PathBuf,
    token: u64,
    definitions: u64,
    identity: Arc<()>,
    cancelled: Arc<AtomicBool>,
    worker: Ticket,
}

pub struct Completed {
    token: u64,
    definitions: u64,
    identity: Arc<()>,
    result: Result<Catalog, String>,
    _worker: Ticket,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Catalog::load_controlled(&self.path, &mut || {
                if self.cancelled.load(Ordering::Acquire) {
                    Err("FGD load cancelled".into())
                } else {
                    Ok(())
                }
            })
        }))
        .unwrap_or_else(|_| Err("FGD load worker panicked".into()));
        Completed {
            token: self.token,
            definitions: self.definitions,
            identity: self.identity,
            result,
            _worker: self.worker,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct MapSizeReport {
    pub authored: [i32; 2],
    pub bounds: [i32; 2],
    pub source: Option<String>,
    pub line: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DefinitionsReport {
    pub classes: usize,
    pub sources: Vec<String>,
    pub error: Option<String>,
    pub map_size: Option<MapSizeReport>,
}

impl DefinitionsReport {
    pub fn status(&self) -> String {
        let mut status = format!("Loaded FGD catalog: {} classes", self.classes);
        match &self.map_size {
            Some(size) => {
                status += &format!(
                    " · FGD map size: {} to {} (authored {}, {}) · {}:{} · Informational only",
                    size.bounds[0],
                    size.bounds[1],
                    size.authored[0],
                    size.authored[1],
                    size.source.as_deref().unwrap_or("Inline FGD"),
                    size.line,
                );
            }
            None => status += " · No effective @mapsize declaration",
        }
        if let Some(error) = &self.error {
            status += &format!(" · {error}");
        }
        status
    }
}

impl Session {
    pub fn begin_definitions_load(&mut self, path: &Path) -> Result<Request, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        let worker = self
            .definitions_load
            .worker
            .begin()
            .ok_or("An FGD load is already in progress")?;
        self.definitions_load.serial += 1;
        let identity = Arc::new(());
        let cancelled = Arc::new(AtomicBool::new(false));
        self.definitions_load.identity = Some(identity.clone());
        self.definitions_load.cancelled = Some(cancelled.clone());
        Ok(Request {
            path: path.to_owned(),
            token: self.definitions_load.serial,
            definitions: self.definitions_serial,
            identity,
            cancelled,
            worker,
        })
    }

    pub fn finish_definitions_load(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<DefinitionsReport, String> {
        if self.closing
            || completed.definitions != self.definitions_serial
            || completed.token != self.definitions_load.serial
            || !self
                .definitions_load
                .identity
                .as_ref()
                .is_some_and(|identity| Arc::ptr_eq(identity, &completed.identity))
        {
            return Err("FGD load no longer belongs to the current editor operation".into());
        }
        self.definitions_load.identity = None;
        self.definitions_load.cancelled = None;
        let catalog = completed.result?;
        self.set_definitions(actor, catalog);
        Ok(self.entity_definitions_report())
    }

    pub fn fail_definitions_load(&mut self, token: u64) {
        if token == self.definitions_load.serial {
            self.definitions_load.identity = None;
            self.definitions_load.cancelled = None;
        }
    }

    pub(crate) fn definitions_load_busy(&self) -> bool {
        self.definitions_load.worker.busy()
    }

    pub(super) fn shutdown_definitions_load(&mut self) {
        if let Some(cancelled) = &self.definitions_load.cancelled {
            cancelled.store(true, Ordering::Release);
        }
    }

    pub fn entity_definitions_report(&self) -> DefinitionsReport {
        DefinitionsReport {
            classes: self.definitions.classes().count(),
            sources: self
                .definitions
                .sources
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            error: self.definitions_error.clone(),
            map_size: self
                .definitions
                .map_size
                .as_ref()
                .map(|size| MapSizeReport {
                    authored: size.authored,
                    bounds: size.bounds,
                    source: size.source.as_ref().map(|path| path.display().to_string()),
                    line: size.line,
                }),
        }
    }
}

impl LoadState {
    pub(super) fn close_work(&self) -> serde_json::Value {
        serde_json::json!({
            "sequence": self.serial,
            "worker_active": self.worker.busy(),
            "cancelled": self.cancelled
                .as_ref()
                .map(|cancelled| cancelled.load(Ordering::Acquire))
        })
    }
}
