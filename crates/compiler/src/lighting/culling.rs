use std::collections::{HashMap, HashSet};

use glam::DVec3;
use source_formats::bsp::{Visibility as NativeVisibility, VisibilityKind};

use super::bake::{Budget, Emitter};
use crate::{map::Map, tree::Tree, visibility::Visibility};

const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
const MAX_CANDIDATES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub evaluated: usize,
    pub pvs_skipped: usize,
    pub range_skipped: usize,
    pub work: usize,
}

pub(super) fn allocation_bytes(clusters: usize, lights: usize) -> Result<usize, String> {
    clusters
        .checked_mul(256)
        .and_then(|bytes| bytes.checked_add(MAX_ROW_BYTES))
        .and_then(|bytes| bytes.checked_add(2 * MAX_CANDIDATES * size_of::<usize>()))
        .and_then(|bytes| bytes.checked_add(lights * 4 * size_of::<usize>()))
        .ok_or_else(|| "Lighting candidate allocation size overflow".into())
}

pub(super) struct Candidates {
    all: Vec<usize>,
    rows: HashMap<usize, Vec<u8>>,
    cached: HashMap<usize, Vec<usize>>,
    entries: usize,
    enabled: bool,
    pub stats: Stats,
}

impl Candidates {
    pub fn new(
        lights: &[Emitter],
        visibility: &Visibility,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let mut index = Self {
            all: (0..lights.len()).collect(),
            rows: HashMap::new(),
            cached: HashMap::new(),
            entries: 0,
            enabled: enabled(),
            stats: Stats::default(),
        };
        if !index.enabled || visibility.data.is_empty() || lights.is_empty() {
            return Ok(index);
        }
        index.consume(visibility.count * 2 + lights.len(), budget)?;
        let native =
            NativeVisibility::parse(&visibility.data).map_err(|error| error.to_string())?;
        if native.clusters() != visibility.count {
            return Err("Lighting visibility cluster count disagrees with the BSP tree".into());
        }
        let width = visibility.count.div_ceil(8);
        let mut decoded = HashSet::new();
        for light in lights {
            let Emitter::Point(light) = light else {
                continue;
            };
            let Ok(cluster) = usize::try_from(light.cluster) else {
                continue;
            };
            if !decoded.insert(cluster) {
                continue;
            }
            if (index.rows.len() + 1) * width > MAX_ROW_BYTES {
                continue;
            }
            index.consume(width * 3, budget)?;
            let row = native
                .row(cluster, VisibilityKind::Visible)
                .map_err(|error| error.to_string())?;
            if row.iter().enumerate().any(|(index, &byte)| {
                let bits = (visibility.count - index * 8).min(8);
                byte != ((1_u16 << bits) - 1) as u8
            }) {
                index.rows.insert(cluster, row);
            }
        }
        Ok(index)
    }

    fn consume(&mut self, work: usize, budget: &mut Budget<'_>) -> Result<(), String> {
        budget.consume(work)?;
        self.stats.work += work;
        Ok(())
    }

    pub fn cluster(
        &mut self,
        point: DVec3,
        map: &Map,
        tree: &Tree,
        visibility: &Visibility,
        budget: &mut Budget<'_>,
    ) -> Result<Option<usize>, String> {
        if self.rows.is_empty() {
            return Ok(None);
        }
        let mut index = 0;
        while let Some((plane, children)) = tree.nodes[index].split {
            self.consume(1, budget)?;
            let distance = map.planes.get(plane).distance(point);
            if distance.abs() <= 1e-6 {
                return Ok(None);
            }
            index = children[usize::from(distance < 0.0)];
        }
        Ok(visibility.clusters[index])
    }

    pub fn for_cluster(
        &mut self,
        cluster: Option<usize>,
        lights: &[Emitter],
        budget: &mut Budget<'_>,
    ) -> Result<&[usize], String> {
        let Some(cluster) = cluster.filter(|_| !self.rows.is_empty()) else {
            return Ok(&self.all);
        };
        if !self.cached.contains_key(&cluster) {
            if self.entries + lights.len() > MAX_CANDIDATES {
                return Ok(&self.all);
            }
            self.consume(lights.len(), budget)?;
            let eligible = lights
                .iter()
                .enumerate()
                .filter_map(|(index, light)| {
                    let visible = match light {
                        Emitter::Sky(_) => true,
                        Emitter::Point(light) => usize::try_from(light.cluster)
                            .ok()
                            .and_then(|source| self.rows.get(&source))
                            .and_then(|row| row.get(cluster / 8))
                            .is_none_or(|byte| byte & (1 << (cluster % 8)) != 0),
                    };
                    visible.then_some(index)
                })
                .collect::<Vec<_>>();
            self.entries += eligible.len();
            self.cached.insert(cluster, eligible);
        }
        let eligible = &self.cached[&cluster];
        self.stats.pvs_skipped += lights.len() - eligible.len();
        Ok(eligible)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

pub(super) fn outside_range(light: &Emitter, point: DVec3) -> bool {
    matches!(light, Emitter::Point(light) if light.falloff.outside_range(light.origin.distance(point)))
}

fn enabled() -> bool {
    true
}
