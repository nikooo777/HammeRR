use std::collections::BTreeMap;
use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use crate::Control;

use super::MAX_EXPANSION_DEPTH;

pub const MAX_OCCURRENCE_BYTES: usize = 2 * 1024 * 1024;
const LOOKUP_ENTRY_BYTES: usize = 128;
const LOOKUP_ROOT_BYTES: usize = 512;
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

    pub fn response_bytes(&self) -> usize {
        self.steps()
            .map(|step| size_of::<OccurrenceStep>() + step.file.len())
            .sum()
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
pub(crate) struct LightOccurrences {
    entities: BTreeMap<u32, Arc<OccurrencePath>>,
    bytes: usize,
}

impl LightOccurrences {
    fn charge(&mut self, additional: usize, control: &Control) -> Result<(), String> {
        control.checkpoint()?;
        self.bytes = self
            .bytes
            .checked_add(additional)
            .filter(|&bytes| bytes <= MAX_OCCURRENCE_BYTES)
            .ok_or("Instance light occurrence metadata exceeds its shared storage budget")?;
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
            return Err("Instance light occurrence path exceeds the helper-depth limit".into());
        }
        let bytes = size_of::<OccurrencePath>()
            .checked_add(ARC_HEADER_BYTES)
            .and_then(|bytes| bytes.checked_add(file.len()))
            .ok_or("Instance light occurrence allocation overflow")?;
        self.charge(bytes, control)?;
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

    pub(super) fn attach(
        &mut self,
        entity: u32,
        path: &Arc<OccurrencePath>,
        control: &Control,
    ) -> Result<(), String> {
        let bytes = LOOKUP_ENTRY_BYTES
            + if self.entities.is_empty() {
                LOOKUP_ROOT_BYTES
            } else {
                0
            };
        self.charge(bytes, control)?;
        if self.entities.insert(entity, Arc::clone(path)).is_some() {
            return Err("Expanded light already has an occurrence identity".into());
        }
        Ok(())
    }

    pub(crate) fn get(&self, entity: u32) -> Option<&Arc<OccurrencePath>> {
        self.entities.get(&entity)
    }
}
