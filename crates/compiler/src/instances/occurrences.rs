use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use crate::Control;

use super::MAX_EXPANSION_DEPTH;

pub const MAX_OCCURRENCE_BYTES: usize = 2 * 1024 * 1024;
const PROVENANCE_SLOT_BYTES: usize = 3 * size_of::<Arc<OccurrencePath>>();
const PROVENANCE_ROOT_BYTES: usize = 128;
const ARC_HEADER_BYTES: usize = 2 * size_of::<usize>();

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct OccurrenceStep {
    pub helper: u32,
    pub file: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OccurrencePath {
    parent: Option<Arc<Self>>,
    step: OccurrenceStep,
    depth: usize,
}

impl OccurrencePath {
    pub fn steps(&self) -> impl Iterator<Item = &OccurrenceStep> {
        let mut steps = [None; MAX_EXPANSION_DEPTH];
        let mut current = Some(self);
        let mut index = self.depth;
        while let Some(path) = current {
            index -= 1;
            steps[index] = Some(&path.step);
            current = path.parent.as_deref();
        }
        steps.into_iter().flatten()
    }

    pub fn label(&self) -> String {
        self.steps()
            .map(|step| format!("entity:{} → {}", step.helper, step.file))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn response_bytes(&self) -> usize {
        self.steps()
            .map(|step| size_of::<OccurrenceStep>() + step.file.len())
            .sum()
    }

    pub fn response_copy_bytes(&self) -> usize {
        self.response_bytes().saturating_mul(8).saturating_add(1024)
    }
}

impl Serialize for OccurrencePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.depth))?;
        for step in self.steps() {
            sequence.serialize_element(step)?;
        }
        sequence.end()
    }
}

#[derive(Debug, Default)]
pub(crate) struct OccurrenceBudget {
    bytes: usize,
    paths_bytes: usize,
    slots: usize,
}

impl OccurrenceBudget {
    fn charge(&mut self, additional: usize, control: &Control) -> Result<(), String> {
        control.checkpoint()?;
        self.bytes = self
            .bytes
            .checked_add(additional)
            .filter(|&bytes| bytes <= MAX_OCCURRENCE_BYTES)
            .ok_or("Instance occurrence metadata exceeds its shared storage budget")?;
        Ok(())
    }

    pub(super) fn enter(
        &mut self,
        parent: Option<Arc<OccurrencePath>>,
        helper: u32,
        file: &str,
        control: &Control,
    ) -> Result<Arc<OccurrencePath>, String> {
        let depth = parent.as_ref().map_or(1, |path| path.depth + 1);
        if depth > MAX_EXPANSION_DEPTH {
            return Err("Instance occurrence path exceeds the helper-depth limit".into());
        }
        let bytes = size_of::<OccurrencePath>()
            .checked_add(ARC_HEADER_BYTES)
            .and_then(|bytes| bytes.checked_add(file.len()))
            .ok_or("Instance occurrence allocation overflow")?;
        self.charge(bytes, control)?;
        self.paths_bytes += bytes;
        let mut path = String::new();
        path.try_reserve_exact(file.len())
            .map_err(|_| "Instance occurrence filename cannot be allocated")?;
        path.push_str(file);
        Ok(Arc::new(OccurrencePath {
            parent,
            step: OccurrenceStep { helper, file: path },
            depth,
        }))
    }

    pub(super) fn slot(&mut self, control: &Control) -> Result<(), String> {
        let bytes = PROVENANCE_SLOT_BYTES
            + if self.slots == 0 {
                PROVENANCE_ROOT_BYTES
            } else {
                0
            };
        self.charge(bytes, control)?;
        self.slots += 1;
        Ok(())
    }

    pub(super) fn paths_bytes(&self) -> usize {
        self.paths_bytes
    }
}

pub fn serialize_occurrence<S: Serializer>(
    path: &Arc<OccurrencePath>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    path.as_ref().serialize(serializer)
}
