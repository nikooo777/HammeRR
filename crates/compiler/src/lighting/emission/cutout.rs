use std::collections::BTreeMap;

use super::super::props::alpha::Material;
use super::super::texture_regions::{self, Rectangle};
use super::exact::HullBounds;
use super::{Allocation, Budget};

pub(super) struct Regions {
    pub rectangles: Vec<Rectangle<bool>>,
    _allocation: Allocation,
}

pub(super) struct Cache {
    masks: BTreeMap<usize, Regions>,
    hulls: BTreeMap<usize, HullBounds>,
    allocation: Allocation,
}

impl Cache {
    pub fn new(budget: &Budget<'_>) -> Result<Self, String> {
        Ok(Self {
            masks: BTreeMap::new(),
            hulls: BTreeMap::new(),
            allocation: budget.reserve_bytes(0)?,
        })
    }

    pub fn admit(&mut self, material: &Material, budget: &mut Budget<'_>) -> Result<(), String> {
        budget.consume(self.masks.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
        let key = std::ptr::from_ref(material) as usize;
        if self.masks.contains_key(&key) {
            return Ok(());
        }
        let mask = material.binary_mask()?;
        let mut allocation = budget.reserve_bytes(size_of::<Regions>())?;
        let rectangles = texture_regions::rectangles(
            mask.dimensions,
            |x, y| mask.opaque(x, y),
            budget,
            &mut allocation,
        )?;
        let slots = if self.masks.is_empty() { 16 } else { 4 };
        self.allocation
            .grow(budget, slots * size_of::<(usize, Regions)>() + 256)?;
        self.masks.insert(
            key,
            Regions {
                rectangles,
                _allocation: allocation,
            },
        );
        Ok(())
    }

    pub fn get(&self, material: &Material) -> Result<&Regions, String> {
        self.masks
            .get(&(std::ptr::from_ref(material) as usize))
            .ok_or_else(|| "Exact cutout lost its original admitted alpha owner".into())
    }

    pub fn admit_hull(
        &mut self,
        hull: &super::super::Occluder,
        bounds: HullBounds,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        budget.consume(self.hulls.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
        let key = std::ptr::from_ref(hull) as usize;
        if self.hulls.contains_key(&key) {
            return Err("Exact cutout hull has duplicate bound admission".into());
        }
        let slots = if self.hulls.is_empty() { 16 } else { 4 };
        self.allocation
            .grow(budget, slots * size_of::<(usize, HullBounds)>() + 256)?;
        self.hulls.insert(key, bounds);
        Ok(())
    }

    pub fn hull_bounds(&self, hull: &super::super::Occluder) -> Result<&HullBounds, String> {
        self.hulls
            .get(&(std::ptr::from_ref(hull) as usize))
            .ok_or_else(|| "Exact cutout hull lost its original certified plane bounds".into())
    }
}
