use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::gamefs::{GameFs, asset_relative};

use super::bake::{Allocation, Budget};
use crate::prop_lighting_assets::SourceAsset;

pub(super) const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCES: usize = 4096;
const MAX_PATH_BYTES: usize = 1024;
const MAX_LOOKUPS: usize = 16_384;

#[derive(Debug, Default)]
pub(crate) struct SourceReads {
    sources: BTreeMap<String, SourceAsset>,
    missing: BTreeSet<String>,
    bytes: usize,
    lookups: usize,
    work: usize,
}

fn bounded_path(path: &str) -> io::Result<String> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err(io::Error::other(
            "Static-prop lighting needs bounded relative source paths",
        ));
    }
    let path = asset_relative(path)?;
    if path.is_empty() {
        return Err(io::Error::other(
            "Static-prop lighting needs a nonempty relative source path",
        ));
    }
    Ok(path)
}

impl SourceReads {
    pub(crate) fn read_limit(&self, path: &str, maximum: usize) -> io::Result<usize> {
        let path = bounded_path(path)?;
        if self.lookups >= MAX_LOOKUPS {
            return Err(io::Error::other(
                "Static-prop lighting exceeds its 16,384 source lookup limit",
            ));
        }
        if !self.sources.contains_key(&path)
            && !self.missing.contains(&path)
            && self.sources.len() >= MAX_SOURCES
            && self.missing.len() >= MAX_SOURCES
        {
            return Err(io::Error::other(
                "Static-prop lighting exceeds its source observation limit",
            ));
        }
        Ok(maximum.min(
            self.sources
                .get(&path)
                .map_or(self.remaining_bytes(), |source| source.bytes),
        ))
    }

    pub(crate) fn record(&mut self, path: &str, data: Option<&[u8]>) -> io::Result<()> {
        self.read_limit(path, usize::MAX)?;
        let path = bounded_path(path)?;
        let work = self
            .work
            .checked_add(1)
            .and_then(|work| work.checked_add(data.map_or(0, |data| data.len().div_ceil(64))))
            .ok_or_else(|| io::Error::other("Static-prop lighting source work overflow"))?;
        match data {
            Some(data) => {
                if self.missing.contains(&path) {
                    return Err(io::Error::other(format!(
                        "Static-prop lighting source {path} appeared during baking"
                    )));
                }
                let known = self.sources.get(&path);
                if known.is_none() && self.sources.len() >= MAX_SOURCES {
                    return Err(io::Error::other(
                        "Static-prop lighting exceeds its 4,096 source limit",
                    ));
                }
                let bytes = self
                    .bytes
                    .checked_add(if known.is_some() { 0 } else { data.len() })
                    .filter(|bytes| *bytes <= MAX_SOURCE_BYTES)
                    .ok_or_else(|| {
                        io::Error::other(
                            "Static-prop lighting sources exceed their aggregate 64 MiB limit",
                        )
                    })?;
                if known.is_some_and(|source| source.bytes != data.len()) {
                    return Err(io::Error::other(format!(
                        "Static-prop lighting source {path} changed during baking"
                    )));
                }
                let hash = crate::asset_sources::hash(data);
                if let Some(known) = known {
                    if known.sha256 != hash {
                        return Err(io::Error::other(format!(
                            "Static-prop lighting source {path} changed during baking"
                        )));
                    }
                } else {
                    self.sources.insert(
                        path.clone(),
                        SourceAsset {
                            path,
                            sha256: hash,
                            bytes: data.len(),
                        },
                    );
                }
                self.bytes = bytes;
            }
            None => {
                if self.sources.contains_key(&path) {
                    return Err(io::Error::other(format!(
                        "Static-prop lighting source {path} disappeared during baking"
                    )));
                }
                if !self.missing.contains(&path) && self.missing.len() >= MAX_SOURCES {
                    return Err(io::Error::other(
                        "Static-prop lighting exceeds its 4,096 missing-source limit",
                    ));
                }
                self.missing.insert(path);
            }
        }
        self.lookups += 1;
        self.work = work;
        Ok(())
    }

    pub(crate) fn remaining_bytes(&self) -> usize {
        MAX_SOURCE_BYTES - self.bytes
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self
                .sources
                .keys()
                .map(|path| size_of::<SourceAsset>() + path.len() * 2 + 320)
                .sum::<usize>()
            + self
                .missing
                .iter()
                .map(|path| path.len() + 256)
                .sum::<usize>()
    }

    pub(crate) fn into_parts(self) -> (Vec<SourceAsset>, Vec<String>) {
        (
            self.sources.into_values().collect(),
            self.missing.into_iter().collect(),
        )
    }
}

pub(super) struct Tracker<'a> {
    fs: &'a GameFs,
    reads: SourceReads,
    allocation: Allocation,
    _input_allocation: Allocation,
}

impl<'a> Tracker<'a> {
    pub(super) fn new(
        fs: &'a GameFs,
        reads: SourceReads,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let allocation = budget.reserve_bytes(reads.retained_bytes())?;
        let input_allocation = budget.reserve_bytes(MAX_SOURCE_BYTES)?;
        budget.consume(reads.work)?;
        Ok(Self {
            fs,
            reads,
            allocation,
            _input_allocation: input_allocation,
        })
    }

    pub(super) fn read(
        &mut self,
        path: &str,
        maximum: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Vec<u8>>, String> {
        let _read_overhead = budget.reserve_bytes(4096)?;
        budget.consume(1)?;
        let maximum = self
            .reads
            .read_limit(path, maximum)
            .map_err(|error| error.to_string())?;
        let path = bounded_path(path).map_err(|error| error.to_string())?;
        if !self.reads.sources.contains_key(&path) && !self.reads.missing.contains(&path) {
            self.allocation
                .grow(budget, size_of::<SourceAsset>() + path.len() * 2 + 320)?;
        }
        let data = self
            .fs
            .read_limited(&path, maximum)
            .map_err(|error| format!("Static-prop lighting source {path}: {error}"))?;
        if let Some(data) = &data {
            budget.consume(data.len().div_ceil(64))?;
        }
        self.reads
            .record(&path, data.as_deref())
            .map_err(|error| error.to_string())?;
        Ok(data)
    }

    pub(super) fn finish(self) -> (Vec<SourceAsset>, Vec<String>, Allocation) {
        let (sources, missing) = self.reads.into_parts();
        (sources, missing, self.allocation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_observations_require_the_same_exact_bytes_and_presence() {
        let mut reads = SourceReads::default();
        reads.record("materials/a.vmt", Some(b"original")).unwrap();
        reads.record("MATERIALS/A.VMT", Some(b"original")).unwrap();
        assert_eq!(reads.remaining_bytes(), MAX_SOURCE_BYTES - 8);
        assert_eq!(reads.read_limit("materials/a.vmt", usize::MAX).unwrap(), 8);
        assert!(reads.record("materials/a.vmt", Some(b"modified")).is_err());
        assert!(
            reads
                .record("materials/a.vmt", Some(b"longer data"))
                .is_err()
        );
        assert!(reads.record("materials/a.vmt", None).is_err());
        reads.record("materials/missing.vmt", None).unwrap();
        assert!(reads.record("materials/missing.vmt", Some(b"new")).is_err());
        let (sources, missing) = reads.into_parts();
        assert_eq!(sources.len(), 1);
        assert_eq!(missing, ["materials/missing.vmt"]);
    }

    #[test]
    fn lookup_path_and_unique_source_caps_precede_growth() {
        let mut reads = SourceReads::default();
        assert!(reads.record(&"a".repeat(MAX_PATH_BYTES + 1), None).is_err());
        assert!(reads.record("../outside", None).is_err());
        for index in 0..MAX_SOURCES {
            reads
                .record(&format!("materials/{index}.vmt"), Some(b"a"))
                .unwrap();
        }
        assert!(reads.record("materials/overflow.vmt", Some(b"a")).is_err());
        assert_eq!(reads.sources.len(), MAX_SOURCES);
        let mut bytes = SourceReads {
            bytes: MAX_SOURCE_BYTES - 1,
            ..Default::default()
        };
        bytes.record("materials/last.vmt", Some(b"a")).unwrap();
        assert_eq!(bytes.read_limit("models/new.mdl", usize::MAX).unwrap(), 0);
        assert_eq!(
            bytes.read_limit("materials/last.vmt", usize::MAX).unwrap(),
            1
        );
        assert!(bytes.record("models/new.mdl", Some(b"a")).is_err());
        let mut missing = SourceReads::default();
        for index in 0..MAX_SOURCES {
            missing
                .record(&format!("materials/missing{index}.vmt"), None)
                .unwrap();
        }
        assert!(
            missing
                .record("materials/missing-overflow.vmt", None)
                .is_err()
        );
        assert_eq!(missing.missing.len(), MAX_SOURCES);
        reads.lookups = MAX_LOOKUPS;
        assert!(reads.read_limit("materials/0.vmt", usize::MAX).is_err());
        assert!(reads.record("materials/0.vmt", Some(b"a")).is_err());
    }
}
