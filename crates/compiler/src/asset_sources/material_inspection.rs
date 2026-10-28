use std::sync::{Arc, OnceLock};

use source_formats::gamefs::{AssetOrigin, MountedAsset, asset_relative};

use super::*;

#[derive(Clone)]
pub struct CapturedMaterial {
    source: Arc<MaterialSource>,
}

impl std::fmt::Debug for CapturedMaterial {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("CapturedMaterial")
            .field("path", &self.path())
            .field("bytes", &self.bytes().len())
            .field("origin", self.origin())
            .finish()
    }
}

#[derive(Debug)]
struct MaterialSource {
    path: String,
    bytes: Vec<u8>,
    origin: AssetOrigin,
}

impl CapturedMaterial {
    pub fn path(&self) -> &str {
        &self.source.path
    }

    pub fn bytes(&self) -> &[u8] {
        &self.source.bytes
    }

    pub fn origin(&self) -> &AssetOrigin {
        &self.source.origin
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialInspectionIssueKind {
    Limited,
    UnavailableSource,
}

#[derive(Debug)]
pub(crate) struct MaterialInspectionIssue {
    pub(crate) kind: MaterialInspectionIssueKind,
    pub(crate) path: String,
    pub(crate) message: String,
}

struct Lookup {
    present: bool,
    paths: AssetSourcePaths,
}

struct ObservedContent {
    sha256: String,
    origin: AssetOrigin,
}

struct Observation {
    lookup: Option<Lookup>,
    content: Option<ObservedContent>,
}

impl Observation {
    fn changed(&self, path: &str, label: &str) -> io::Error {
        let detail = match &self.content {
            Some(content) => {
                let storage = match &content.origin {
                    AssetOrigin::Directory { .. } => "directory",
                    AssetOrigin::Vpk { .. } => "VPK",
                };
                format!("inspected {storage} SHA-256 {}", content.sha256)
            }
            None => "originally absent or unreadable".into(),
        };
        invalid(format!(
            "{label} source resolution changed: {path} ({detail})"
        ))
    }
}

struct ReadFailure {
    kind: io::ErrorKind,
    message: String,
}

impl ReadFailure {
    fn from_error(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }

    fn error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

enum CachedRead {
    Material(CapturedMaterial),
    Texture(usize),
    Missing,
    Failed(ReadFailure),
}

pub struct MaterialInspectionCapture<'a> {
    game: &'a GameFs,
    control: &'a Control,
    policy: SourceGuardPolicy,
    reads: BTreeMap<String, CachedRead>,
    observations: BTreeMap<String, Observation>,
    stamps: BTreeMap<PathBuf, FileStamp>,
    lookup_rows: usize,
    issue: OnceLock<MaterialInspectionIssue>,
}

impl<'a> MaterialInspectionCapture<'a> {
    pub fn new(game: &'a GameFs, control: &'a Control, policy: SourceGuardPolicy) -> Self {
        Self {
            game,
            control,
            policy,
            reads: BTreeMap::new(),
            observations: BTreeMap::new(),
            stamps: BTreeMap::new(),
            lookup_rows: 0,
            issue: OnceLock::new(),
        }
    }

    pub fn read_material(
        &mut self,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<CapturedMaterial>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let path = asset_relative(path)?;
        if let Some(cached) = self.reads.get(&path) {
            return match cached {
                CachedRead::Material(material) => {
                    check_length(material.bytes().len(), maximum, Some(material.origin()))?;
                    Ok(Some(material.clone()))
                }
                CachedRead::Missing => Ok(None),
                CachedRead::Failed(failure) => Err(failure.error()),
                CachedRead::Texture(_) => Err(invalid(format!(
                    "Material inspection payload was already consumed as a texture: {path}"
                ))),
            };
        }
        match self.read_original(&path, maximum) {
            Ok(Some(asset)) => {
                let material = CapturedMaterial {
                    source: Arc::new(MaterialSource {
                        path: path.clone(),
                        bytes: asset.data,
                        origin: asset.origin,
                    }),
                };
                self.reads
                    .insert(path, CachedRead::Material(material.clone()));
                Ok(Some(material))
            }
            Ok(None) => {
                self.reads.insert(path, CachedRead::Missing);
                Ok(None)
            }
            Err(error) => {
                self.reads
                    .insert(path, CachedRead::Failed(ReadFailure::from_error(&error)));
                Err(error)
            }
        }
    }

    pub(crate) fn materials(&self) -> impl Iterator<Item = &CapturedMaterial> {
        self.reads.values().filter_map(|read| match read {
            CachedRead::Material(material) => Some(material),
            _ => None,
        })
    }

    pub(crate) fn read_texture(
        &mut self,
        path: &str,
        maximum: usize,
    ) -> io::Result<Option<MountedAsset>> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let path = asset_relative(path)?;
        if let Some(cached) = self.reads.get(&path) {
            return match cached {
                CachedRead::Material(material) => {
                    check_length(material.bytes().len(), maximum, Some(material.origin()))?;
                    Ok(Some(MountedAsset {
                        data: material.bytes().to_vec(),
                        origin: material.origin().clone(),
                    }))
                }
                CachedRead::Missing => Ok(None),
                CachedRead::Failed(failure) => Err(failure.error()),
                CachedRead::Texture(bytes) => {
                    let origin = self.observations[&path]
                        .content
                        .as_ref()
                        .map(|content| &content.origin);
                    check_length(*bytes, maximum, origin)?;
                    Err(invalid(format!(
                        "Material inspection texture payload was already consumed: {path}"
                    )))
                }
            };
        }
        match self.read_original(&path, maximum) {
            Ok(Some(asset)) => {
                self.reads
                    .insert(path, CachedRead::Texture(asset.data.len()));
                Ok(Some(asset))
            }
            Ok(None) => {
                self.reads.insert(path, CachedRead::Missing);
                Ok(None)
            }
            Err(error) => {
                self.reads
                    .insert(path, CachedRead::Failed(ReadFailure::from_error(&error)));
                Err(error)
            }
        }
    }

    pub(crate) fn issue(&self) -> Option<&MaterialInspectionIssue> {
        self.issue.get()
    }

    pub(crate) fn verify_originals(&self) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)?;
        if let Some(issue) = self.issue() {
            return Err(invalid(issue.message.clone()));
        }
        let result = self.verify_snapshot();
        if let Err(error) = &result {
            self.record_issue("", error);
        }
        result
    }

    fn verify_snapshot(&self) -> io::Result<()> {
        let guard = AssetSourceGuard {
            stamps: self.stamps.values().cloned().collect(),
            material_lookups: BTreeMap::new(),
        };
        guard.verify_metadata()?;
        for (path, observation) in &self.observations {
            self.control.checkpoint().map_err(io::Error::other)?;
            if let Some(CachedRead::Failed(failure)) = self.reads.get(path) {
                return Err(failure.error());
            }
            let original = observation.lookup.as_ref().ok_or_else(|| {
                invalid(format!(
                    "{} original lookup is unavailable: {path}",
                    self.policy.label
                ))
            })?;
            let current = self.lookup(path)?;
            if current.present != original.present || current.paths != original.paths {
                return Err(observation.changed(path, self.policy.label));
            }
        }
        self.control.checkpoint().map_err(io::Error::other)?;
        guard.verify_metadata()
    }

    fn lookup(&self, path: &str) -> io::Result<Lookup> {
        match self.game.asset_source_paths(path)? {
            Some(paths) => Ok(Lookup {
                present: true,
                paths,
            }),
            None => Ok(Lookup {
                present: false,
                paths: self.game.asset_lookup_paths(path)?,
            }),
        }
    }

    fn read_original(&mut self, path: &str, maximum: usize) -> io::Result<Option<MountedAsset>> {
        let before = match self.lookup(path) {
            Ok(lookup) => self.retain_lookup(path, lookup),
            Err(error) => {
                self.record_issue(path, &error);
                None
            }
        };
        let captured = if let Some(lookup) = &before {
            match capture_paths(&lookup.paths, &mut self.stamps, self.policy) {
                Ok(()) => true,
                Err(error) => {
                    self.record_issue(path, &error);
                    false
                }
            }
        } else {
            false
        };
        self.observations.insert(
            path.to_owned(),
            Observation {
                lookup: before,
                content: None,
            },
        );
        self.control.checkpoint().map_err(io::Error::other)?;

        let asset = self.game.read_asset_limited(path, maximum);
        let content = asset
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|asset| ObservedContent {
                sha256: hash(&asset.data),
                origin: asset.origin.clone(),
            });
        self.observations.get_mut(path).unwrap().content = content;
        if let Some(before) = &self.observations[path].lookup {
            match self.lookup(path) {
                Ok(after) if after.present == before.present && after.paths == before.paths => {
                    if let Ok(payload) = &asset
                        && payload.is_some() != before.present
                    {
                        let error = self.observations[path].changed(path, self.policy.label);
                        self.record_issue(path, &error);
                    }
                    if captured
                        && let Err(error) =
                            capture_paths(&after.paths, &mut self.stamps, self.policy)
                    {
                        self.record_issue(path, &error);
                    }
                }
                Ok(_) => {
                    let error = self.observations[path].changed(path, self.policy.label);
                    self.record_issue(path, &error);
                }
                Err(error) => self.record_issue(path, &error),
            }
        }
        asset
    }

    fn retain_lookup(&mut self, path: &str, lookup: Lookup) -> Option<Lookup> {
        let rows = lookup
            .paths
            .files
            .len()
            .checked_add(lookup.paths.lookup_directories.len())
            .and_then(|rows| rows.checked_add(lookup.paths.lookup_symlinks.len()));
        let total = rows.and_then(|rows| self.lookup_rows.checked_add(rows));
        if let Some(total) = total.filter(|total| *total <= self.policy.physical_paths) {
            self.lookup_rows = total;
            Some(lookup)
        } else {
            self.record_issue_as(
                path,
                &invalid(format!(
                    "{} source capture exceeds {} retained lookup rows",
                    self.policy.label, self.policy.physical_paths,
                )),
                MaterialInspectionIssueKind::Limited,
            );
            None
        }
    }

    fn record_issue(&self, path: &str, error: &io::Error) {
        self.record_issue_as(path, error, MaterialInspectionIssueKind::UnavailableSource);
    }

    fn record_issue_as(&self, path: &str, error: &io::Error, kind: MaterialInspectionIssueKind) {
        let context = if path.is_empty() {
            String::new()
        } else {
            format!(" for {path}")
        };
        let _ = self.issue.set(MaterialInspectionIssue {
            kind,
            path: path.to_owned(),
            message: format!(
                "{} original source capture failed{context}: {error}",
                self.policy.label
            ),
        });
    }
}

fn check_length(bytes: usize, maximum: usize, origin: Option<&AssetOrigin>) -> io::Result<()> {
    if bytes > maximum {
        return Err(invalid(match origin {
            Some(AssetOrigin::Vpk { .. }) => "VPK entry exceeds the caller's byte read limit",
            _ => "asset exceeds the caller's byte read limit",
        }));
    }
    Ok(())
}
