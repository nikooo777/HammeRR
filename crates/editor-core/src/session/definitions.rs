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
    status_serial: u64,
    catalog_serial: u64,
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
pub struct MetadataSourceReport {
    pub source: Option<String>,
    pub line: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct MaterialExclusionReport {
    pub prefix: String,
    pub source: Option<String>,
    pub line: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct AutoVisGroupReport {
    pub parent: String,
    pub name: String,
    pub classes: Vec<String>,
    pub sources: Vec<MetadataSourceReport>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct VersionReport {
    pub value: i32,
    pub source: Option<String>,
    pub line: usize,
    pub diagnostic: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DefinitionsReport {
    pub available: bool,
    pub serial: u64,
    pub status_serial: u64,
    pub classes: usize,
    pub sources: Vec<String>,
    pub error: Option<String>,
    pub map_size: Option<MapSizeReport>,
    pub material_exclusions: Vec<MaterialExclusionReport>,
    pub auto_visgroups: Vec<AutoVisGroupReport>,
    pub versions: Vec<VersionReport>,
    pub hammerplusplus_resource_version_reference: i32,
    pub version_policy: &'static str,
}

impl DefinitionsReport {
    pub fn status(&self) -> String {
        let mut status = if self.available {
            format!("Loaded FGD catalog: {} classes", self.classes)
        } else {
            format!("FGD catalog unavailable: {} retained classes", self.classes)
        };
        status += &format!(
            " · {} material exclusions · {} automatic visibility groups",
            self.material_exclusions.len(),
            self.auto_visgroups.len()
        );
        if !self.versions.is_empty() {
            let mismatches = self
                .versions
                .iter()
                .filter(|version| version.diagnostic.is_some())
                .count();
            status += &format!(
                " · {} FGD resource version declarations",
                self.versions.len()
            );
            if mismatches != 0 {
                status += &format!(" ({mismatches} differ from Hammer++ resource version 9)");
            }
        }
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
    pub fn definitions_status_serial(&self) -> u64 {
        self.definitions_load.status_serial
    }

    pub(super) fn record_definitions_error(&mut self, actor: Actor, error: String) {
        self.log(actor, format!("entity definitions: {error}"));
        self.definitions_error = Some(error);
        self.definitions_load.status_serial += 1;
        self.view_serial += 1;
    }

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
        self.definitions_load.catalog_serial = self.definitions_serial;
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
        let catalog = match completed.result {
            Ok(catalog) => catalog,
            Err(error) => {
                self.record_definitions_error(actor, error.clone());
                return Err(error);
            }
        };
        self.set_definitions(actor, catalog);
        Ok(self.entity_definitions_report())
    }

    pub fn fail_definitions_load(&mut self, actor: Actor, token: u64, error: String) {
        if token == self.definitions_load.serial && self.definitions_load.identity.is_some() {
            self.definitions_load.identity = None;
            self.definitions_load.cancelled = None;
            if !self.closing && self.definitions_load.catalog_serial == self.definitions_serial {
                self.record_definitions_error(actor, error);
            }
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
            available: self.definitions_serial > 0 && self.definitions_error.is_none(),
            serial: self.definitions_serial,
            status_serial: self.definitions_status_serial(),
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
            material_exclusions: self
                .definitions
                .material_exclusions
                .iter()
                .map(|exclusion| MaterialExclusionReport {
                    prefix: exclusion.prefix.clone(),
                    source: exclusion
                        .source
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    line: exclusion.line,
                })
                .collect(),
            auto_visgroups: self
                .definitions
                .auto_visgroups
                .iter()
                .map(|group| AutoVisGroupReport {
                    parent: group.parent.clone(),
                    name: group.name.clone(),
                    classes: group.classes.clone(),
                    sources: group
                        .sources
                        .iter()
                        .map(|source| MetadataSourceReport {
                            source: source
                                .source
                                .as_ref()
                                .map(|path| path.display().to_string()),
                            line: source.line,
                        })
                        .collect(),
                })
                .collect(),
            versions: self
                .definitions
                .versions
                .iter()
                .map(|version| VersionReport {
                    value: version.value,
                    source: version
                        .source
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    line: version.line,
                    diagnostic: version.diagnostic().map(str::to_owned),
                })
                .collect(),
            hammerplusplus_resource_version_reference:
                source_formats::fgd::HAMMERPLUSPLUS_FGD_VERSION,
            version_policy: "Version declarations describe vendor FGD resource freshness; they do not select a grammar, require an additional FGD, or certify full vendor compatibility. An absent declaration is valid.",
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
