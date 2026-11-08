use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use super::super::{Budget, EDGE_TOLERANCE, NeighborPair, SurfaceGrid, root};
use super::{Groups, accepts};

const RULE_WORK: usize = 64;
const MIN_PACKED_NORMAL_SQUARED: f64 = 1e-12;
const SCRATCH_FIXED_BYTES: usize = 4096;
const SCRATCH_BYTES_PER_RULE: usize = 512;

pub(in crate::displacement::neighbors) fn scratch_bytes(power: u32) -> Result<usize, String> {
    if !(2..=4).contains(&power) {
        return Err("Terrain normal scratch requires power 2, 3 or 4".into());
    }
    // Reserve sparse roots and up to cells−2 targets per approved edge, including Vec slack.
    Ok(SCRATCH_FIXED_BYTES + 4 * ((1usize << power) - 2) * SCRATCH_BYTES_PER_RULE)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rule {
    first: usize,
    second: usize,
    numerator: usize,
    denominator: usize,
}

impl Rule {
    fn new(
        mut first: usize,
        mut second: usize,
        mut numerator: usize,
        mut denominator: usize,
    ) -> Self {
        if first == second {
            numerator = 0;
            denominator = 1;
        } else {
            if first > second {
                std::mem::swap(&mut first, &mut second);
                numerator = denominator - numerator;
            }
            while numerator.is_multiple_of(2) && denominator.is_multiple_of(2) {
                numerator /= 2;
                denominator /= 2;
            }
        }
        Self {
            first,
            second,
            numerator,
            denominator,
        }
    }

    fn weight(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    fn parents(self) -> impl Iterator<Item = usize> {
        [
            Some(self.first),
            (self.second != self.first).then_some(self.second),
        ]
        .into_iter()
        .flatten()
    }
}

pub(in crate::displacement::neighbors) struct Constraints {
    rules: BTreeMap<usize, Rule>,
    order: Vec<usize>,
}

impl Constraints {
    pub(in crate::displacement::neighbors) fn prepare<F: FnMut(usize) -> Result<(), String>>(
        grids: &[SurfaceGrid<'_>],
        pairs: &[NeighborPair],
        groups: &mut Groups<'_>,
        budget: &mut Budget<'_, F>,
    ) -> Result<Self, String> {
        let mut result = Self {
            rules: BTreeMap::new(),
            order: Vec::new(),
        };
        for pair in pairs {
            budget.consume(1)?;
            if groups.angles[pair.first] == Some(0.0) || groups.angles[pair.second] == Some(0.0) {
                continue;
            }
            let a = pair
                .first_span
                .indices(grids[pair.first].power, pair.first_edge)?;
            let mut b = pair
                .second_span
                .indices(grids[pair.second].power, pair.second_edge)?;
            b.reverse();
            budget.consume(a.len() + b.len())?;
            if a.len() == b.len() {
                continue;
            }
            let (fine, fine_indices, coarse, coarse_indices) = if a.len() > b.len() {
                (pair.first, &a, pair.second, &b)
            } else {
                (pair.second, &b, pair.first, &a)
            };
            let steps = coarse_indices.len() - 1;
            let stride = (fine_indices.len() - 1) / steps;
            let angles = [groups.angles[fine], groups.angles[coarse]];
            for interval in 0..steps {
                let coarse_ends = [coarse_indices[interval], coarse_indices[interval + 1]];
                let fine_ends = [
                    fine_indices[interval * stride],
                    fine_indices[(interval + 1) * stride],
                ];
                budget.consume(stride + 2)?;
                if !(0..2).all(|end| {
                    accepts(
                        groups.local[fine][fine_ends[end]],
                        groups.local[coarse][coarse_ends[end]],
                        angles,
                    )
                }) {
                    continue;
                }
                let local =
                    coarse_ends.map(|index| groups.local[coarse][index].normalize_or_zero());
                if !(1..stride).all(|step| {
                    let t = step as f64 / stride as f64;
                    accepts(
                        groups.local[fine][fine_indices[interval * stride + step]],
                        local[0] * (1.0 - t) + local[1] * t,
                        angles,
                    )
                }) {
                    continue;
                }
                let ends = coarse_ends.map(|index| grids[coarse].positions[index]);
                let first = root(
                    groups.parents,
                    groups.offsets[coarse] + coarse_ends[0],
                    budget,
                )?;
                let second = root(
                    groups.parents,
                    groups.offsets[coarse] + coarse_ends[1],
                    budget,
                )?;
                for step in 1..stride {
                    budget.consume(1)?;
                    let vertex = fine_indices[interval * stride + step];
                    let t = step as f64 / stride as f64;
                    let expected = ends[0] * (1.0 - t) + ends[1] * t;
                    if !expected.is_finite()
                        || grids[fine].positions[vertex].distance(expected) > EDGE_TOLERANCE
                    {
                        return Err("Terrain normal interpolation requires the prepared coarse-edge geometry".into());
                    }
                    let target = root(groups.parents, groups.offsets[fine] + vertex, budget)?;
                    result.insert(target, Rule::new(first, second, step, stride), budget)?;
                }
            }
        }
        result.sort(budget)?;
        Ok(result)
    }

    fn insert<F: FnMut(usize) -> Result<(), String>>(
        &mut self,
        target: usize,
        rule: Rule,
        budget: &mut Budget<'_, F>,
    ) -> Result<(), String> {
        budget.consume(1)?;
        if rule.first == target && rule.second == target {
            return Ok(());
        }
        if let Some(previous) = self.rules.get(&target) {
            if *previous != rule {
                return Err("Terrain normal group has conflicting interpolation supports".into());
            }
            return Ok(());
        }
        budget.consume(RULE_WORK)?;
        self.rules.insert(target, rule);
        Ok(())
    }

    fn sort<F: FnMut(usize) -> Result<(), String>>(
        &mut self,
        budget: &mut Budget<'_, F>,
    ) -> Result<(), String> {
        budget.consume(self.rules.len() * 4)?;
        let mut degrees = BTreeMap::new();
        let mut dependents = BTreeMap::<usize, Vec<usize>>::new();
        let mut ready = BTreeSet::new();
        for (&target, rule) in &self.rules {
            let mut degree = 0usize;
            for parent in rule
                .parents()
                .filter(|parent| self.rules.contains_key(parent))
            {
                budget.consume(2)?;
                degree += 1;
                dependents.entry(parent).or_default().push(target);
            }
            degrees.insert(target, degree);
            if degree == 0 {
                ready.insert(target);
            }
        }
        self.order.reserve(self.rules.len());
        while let Some(target) = ready.pop_first() {
            budget.consume(1)?;
            self.order.push(target);
            if let Some(children) = dependents.get(&target) {
                for child in children {
                    budget.consume(1)?;
                    let degree = degrees
                        .get_mut(child)
                        .ok_or("Terrain normal dependency lost its group")?;
                    *degree = degree
                        .checked_sub(1)
                        .ok_or("Terrain normal dependency count underflow")?;
                    if *degree == 0 {
                        ready.insert(*child);
                    }
                }
            }
        }
        if self.order.len() != self.rules.len() {
            return Err("Terrain normal interpolation contains a cyclic group dependency".into());
        }
        Ok(())
    }

    pub(in crate::displacement::neighbors) fn apply<F: FnMut(usize) -> Result<(), String>>(
        &self,
        totals: &mut [DVec3],
        budget: &mut Budget<'_, F>,
    ) -> Result<bool, String> {
        if self.rules.is_empty() {
            return Ok(false);
        }
        for &target in self.order.iter().rev() {
            budget.consume(3)?;
            let rule = self.rules[&target];
            let load = totals[target];
            let weight = rule.weight();
            totals[rule.first] += load * (1.0 - weight);
            if rule.second != rule.first {
                totals[rule.second] += load * weight;
            }
            if !totals[rule.first].is_finite() || !totals[rule.second].is_finite() {
                return Err("Terrain constrained normal area accumulation is not finite".into());
            }
        }
        budget.consume(totals.len())?;
        for total in totals.iter_mut() {
            *total = total.normalize_or_zero();
        }
        for &target in &self.order {
            budget.consume(8)?;
            let rule = self.rules[&target];
            let first = totals[rule.first];
            let second = totals[rule.second];
            segment(first, second)?;
            let weight = rule.weight();
            let value = first * (1.0 - weight) + second * weight;
            packed(value)?;
            totals[target] = value;
        }
        Ok(true)
    }
}

fn packed(value: DVec3) -> Result<DVec3, String> {
    let value = value.as_vec3();
    let squared = value.length_squared();
    if !value.is_finite() || !squared.is_finite() || f64::from(squared) < MIN_PACKED_NORMAL_SQUARED
    {
        return Err("Terrain normal interpolation requires finite nonzero packed fields".into());
    }
    Ok(value.as_dvec3())
}

fn segment(first: DVec3, second: DVec3) -> Result<(), String> {
    let first = packed(first)?;
    let second = packed(second)?;
    let delta = second - first;
    let squared = delta.length_squared();
    let t = if squared == 0.0 {
        0.0
    } else {
        (-first.dot(delta) / squared).clamp(0.0, 1.0)
    };
    let minimum = (first + delta * t).length_squared();
    let rounding =
        128.0 * f64::EPSILON * (first.length_squared() + second.length_squared() + squared);
    if !minimum.is_finite() || minimum < MIN_PACKED_NORMAL_SQUARED + rounding {
        return Err(
            "Terrain normal interpolation can cancel within its packed edge segment".into(),
        );
    }
    Ok(())
}
