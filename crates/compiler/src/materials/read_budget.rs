use std::io;

use source_formats::gamefs::GameFs;

use crate::asset_sources::BuildMaterialSources;
use crate::lighting::asset_tracking::SourceReads;

#[derive(Clone, Copy)]
pub(super) struct ReadLimits {
    pub label: &'static str,
    pub file_bytes: usize,
    pub total_bytes: usize,
    pub operations: usize,
}

pub(super) struct ReadBudget {
    limits: ReadLimits,
    pub operations: usize,
    pub bytes: usize,
    build_sources: Option<BuildMaterialSources>,
}

impl ReadBudget {
    pub fn new(limits: ReadLimits) -> Self {
        Self {
            limits,
            operations: 0,
            bytes: 0,
            build_sources: None,
        }
    }

    pub fn track_build_sources(&mut self, sources: &BuildMaterialSources) {
        self.build_sources = Some(sources.clone());
    }

    pub fn uses_build_sources(&self, sources: &BuildMaterialSources) -> bool {
        self.build_sources
            .as_ref()
            .is_some_and(|owner| owner.same_capture(sources))
    }

    pub fn read(
        &mut self,
        fs: &GameFs,
        path: &str,
        maximum: usize,
        sources: Option<&mut SourceReads>,
    ) -> io::Result<Option<Vec<u8>>> {
        if self.operations >= self.limits.operations {
            return Err(io::Error::other(format!(
                "{} {path} exceeds {} source reads",
                self.limits.label, self.limits.operations,
            )));
        }
        let remaining = self
            .limits
            .total_bytes
            .checked_sub(self.bytes)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{} {path} byte accounting overflow",
                    self.limits.label
                ))
            })?;
        if remaining == 0 {
            return Err(io::Error::other(format!(
                "{} {path} exhausts its aggregate limit {} bytes",
                self.limits.label, self.limits.total_bytes,
            )));
        }
        let limit = self.limits.file_bytes.min(remaining).min(maximum);
        let limit = match &sources {
            Some(sources) => sources.read_limit(path, limit),
            None => Ok(limit),
        }
        .map_err(|error| self.context(path, maximum, remaining, limit, error))?;
        let operations = self.operations.checked_add(1).ok_or_else(|| {
            io::Error::other(format!(
                "{} {path} source read accounting overflow",
                self.limits.label
            ))
        })?;
        self.operations = operations;
        let data = match &self.build_sources {
            Some(owner) => owner
                .read_asset(fs, path, limit)
                .map(|asset| asset.map(|asset| asset.bytes().to_vec())),
            None => fs.read_limited(path, limit),
        }
        .map_err(|error| self.context(path, maximum, remaining, limit, error))?;
        self.bytes = self
            .bytes
            .checked_add(data.as_ref().map_or(0, Vec::len))
            .filter(|bytes| *bytes <= self.limits.total_bytes)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "{} {path} byte accounting overflow",
                    self.limits.label
                ))
            })?;
        if let Some(sources) = sources {
            sources
                .record(path, data.as_deref())
                .map_err(|error| self.context(path, maximum, remaining, limit, error))?;
        }
        Ok(data)
    }

    fn context(
        &self,
        path: &str,
        maximum: usize,
        remaining: usize,
        limit: usize,
        error: io::Error,
    ) -> io::Error {
        io::Error::new(
            error.kind(),
            format!(
                "{} {path} (per-file limit {} bytes; aggregate limit {} bytes, remaining {remaining} bytes; caller limit {maximum} bytes; effective read limit {limit} bytes): {error}",
                self.limits.label, self.limits.file_bytes, self.limits.total_bytes,
            ),
        )
    }
}
