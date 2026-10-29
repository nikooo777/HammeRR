use std::cell::RefCell;

use glam::DVec3;

use super::Occluder;
use super::bake::Budget;
use super::props::PropTriangle;
use super::shadows::{ShadowIndex, TRIANGLE_EPSILON, Visit};
use super::terrain_shadows::TerrainTriangle;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Coverage([u8; 3]);

impl Coverage {
    pub(super) const CLEAR: Self = Self::scalar(0);
    pub(super) const OPAQUE: Self = Self::scalar(u8::MAX);

    pub(super) const fn scalar(coverage: u8) -> Self {
        Self([coverage; 3])
    }

    pub(super) const fn from_channels(coverage: [u8; 3]) -> Self {
        Self(coverage)
    }

    pub(super) fn add(&mut self, coverage: Self) {
        self.0 = std::array::from_fn(|axis| self.0[axis].saturating_add(coverage.0[axis]));
    }

    pub(super) fn maximum(self, coverage: Self) -> Self {
        Self(std::array::from_fn(|axis| {
            self.0[axis].max(coverage.0[axis])
        }))
    }

    pub(super) fn opaque(&self) -> bool {
        *self == Self::OPAQUE
    }

    pub(super) fn clear(&self) -> bool {
        *self == Self::CLEAR
    }

    pub(super) fn visibility(&self) -> DVec3 {
        DVec3::from_array(
            self.0
                .map(|value| f64::from(u8::MAX - value) / f64::from(u8::MAX)),
        )
    }

    pub(super) fn amount(&self) -> DVec3 {
        DVec3::from_array(self.0.map(|value| f64::from(value) / f64::from(u8::MAX)))
    }
}

const GLOBAL_HULL_FAMILY: u8 = 0;
const MODEL_HULL_FAMILY: u8 = 1;
const TERRAIN_FAMILY: u8 = 2;
const PROP_FAMILY: u8 = 3;
const TERMINAL_FAMILY: u8 = 4;
const SOURCE_PREFIX_WORK: usize = 24;

#[derive(Clone, Copy)]
pub(super) enum SourceTarget {
    Hull { slot: usize, boundary: usize },
    Terrain { slot: usize },
    Sky,
    Sheet,
}

impl SourceTarget {
    fn includes(self, fraction: f64, event: (f64, u8, usize, usize)) -> bool {
        let key = match self {
            Self::Hull { slot, boundary } => (fraction, GLOBAL_HULL_FAMILY, slot, boundary),
            Self::Terrain { slot } => (fraction, TERRAIN_FAMILY, slot, 0),
            Self::Sheet => (fraction, TERMINAL_FAMILY, 0, 0),
            Self::Sky => return event.0 <= fraction + TRIANGLE_EPSILON,
        };
        event < key
    }
}

pub(super) struct Scene<'a> {
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<TerrainTriangle>,
    pub props: &'a ShadowIndex<PropTriangle>,
    pub model: Option<&'a ShadowIndex<Occluder>>,
}

impl Scene<'_> {
    pub(super) fn source_visibility(
        &self,
        start: DVec3,
        end: DVec3,
        fraction: f64,
        target: SourceTarget,
        skip_owner: Option<u32>,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        if !start.is_finite() || !end.is_finite() || !(end - start).is_finite() {
            return Err("Source event ray and arithmetic must be finite".into());
        }
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err("Source event fraction must be finite and within its original ray".into());
        }
        match target {
            SourceTarget::Hull { slot, boundary }
                if slot >= self.shadows.objects().len() || boundary >= 2 =>
            {
                return Err("Source event references a missing global hull boundary".into());
            }
            SourceTarget::Terrain { slot } if slot >= self.terrain.objects().len() => {
                return Err("Source event references a missing terrain triangle".into());
            }
            _ => {}
        }
        let limit = (fraction + TRIANGLE_EPSILON).min(1.0);
        let mut coverage = Coverage::CLEAR;
        let mut sky: Option<f64> = None;
        let mut found = false;
        {
            let budget = RefCell::new(&mut *budget);
            self.shadows.visit_indexed(
                start,
                end,
                &mut |work| budget.borrow_mut().consume(work),
                &mut |slot, hull| {
                    budget.borrow_mut().consume(SOURCE_PREFIX_WORK)?;
                    for (boundary, event) in
                        hull.optical_boundaries(start, end)?.into_iter().enumerate()
                    {
                        let Some(event) = event else {
                            continue;
                        };
                        if event.fraction > limit {
                            continue;
                        }
                        if event.sky {
                            sky =
                                Some(sky.map_or(event.fraction, |first| first.min(event.fraction)));
                        }
                        if matches!(target, SourceTarget::Hull { slot: source, boundary: side }
                            if source == slot && side == boundary && event.fraction == fraction)
                        {
                            found = true;
                        }
                        if target.includes(
                            fraction,
                            (event.fraction, GLOBAL_HULL_FAMILY, slot, boundary),
                        ) {
                            coverage.add(event.non_sky_coverage);
                        }
                        if coverage.opaque() {
                            return Ok(Visit::Stop);
                        }
                    }
                    Ok(Visit::Continue(limit))
                },
            )?;
        }
        if coverage.opaque() || sky.is_some_and(|terminal| fraction > terminal + TRIANGLE_EPSILON) {
            return Ok(DVec3::ZERO);
        }
        if matches!(target, SourceTarget::Hull { .. }) && !found {
            return Err("Source event does not match its actual global hull boundary".into());
        }
        if matches!(target, SourceTarget::Sky) && sky != Some(fraction) {
            return Err("Sky source event does not match the actual global terminal".into());
        }
        if let Some(model) = self.model {
            let budget = RefCell::new(&mut *budget);
            model.visit_indexed(
                start,
                end,
                &mut |work| budget.borrow_mut().consume(work),
                &mut |slot, hull| {
                    budget.borrow_mut().consume(SOURCE_PREFIX_WORK)?;
                    for (boundary, event) in
                        hull.optical_boundaries(start, end)?.into_iter().enumerate()
                    {
                        let Some(event) = event else {
                            continue;
                        };
                        if target.includes(
                            fraction,
                            (event.fraction, MODEL_HULL_FAMILY, slot, boundary),
                        ) {
                            let value = if sky.is_some_and(|terminal| {
                                event.fraction >= terminal - TRIANGLE_EPSILON
                                    && event.fraction <= terminal + TRIANGLE_EPSILON
                            }) {
                                event.non_sky_coverage
                            } else {
                                event.coverage
                            };
                            coverage.add(value);
                        }
                        if coverage.opaque() {
                            return Ok(Visit::Stop);
                        }
                    }
                    Ok(Visit::Continue(limit))
                },
            )?;
        }
        if coverage.opaque() {
            return Ok(DVec3::ZERO);
        }
        {
            let budget = RefCell::new(&mut *budget);
            self.terrain.visit_indexed(
                start,
                end,
                &mut |work| budget.borrow_mut().consume(work),
                &mut |slot, triangle| {
                    budget.borrow_mut().consume(SOURCE_PREFIX_WORK)?;
                    if let Some((hit, weights)) = triangle.source_intersection(start, end)? {
                        if matches!(target, SourceTarget::Terrain { slot: source }
                            if source == slot && hit == fraction)
                        {
                            found = true;
                        }
                        if target.includes(fraction, (hit, TERRAIN_FAMILY, slot, 0)) {
                            coverage.add(triangle.sampled_coverage(weights)?);
                        }
                    }
                    Ok(if coverage.opaque() {
                        Visit::Stop
                    } else {
                        Visit::Continue(limit)
                    })
                },
            )?;
        }
        if coverage.opaque() {
            return Ok(DVec3::ZERO);
        }
        if matches!(target, SourceTarget::Terrain { .. }) && !found {
            return Err("Source event does not match its actual native terrain crossing".into());
        }
        {
            let budget = RefCell::new(&mut *budget);
            self.props.visit_indexed(
                start,
                end,
                &mut |work| budget.borrow_mut().consume(work),
                &mut |slot, triangle| {
                    budget.borrow_mut().consume(SOURCE_PREFIX_WORK)?;
                    if !triangle.owned_by(skip_owner)
                        && let Some((hit, value)) = triangle.sampled_coverage(start, end)?
                        && target.includes(fraction, (hit, PROP_FAMILY, slot, 0))
                    {
                        coverage.add(value);
                    }
                    Ok(if coverage.opaque() {
                        Visit::Stop
                    } else {
                        Visit::Continue(limit)
                    })
                },
            )?;
        }
        Ok(coverage.visibility())
    }

    pub(super) fn visibility(
        &self,
        start: DVec3,
        end: DVec3,
        skip_owner: Option<u32>,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        let mut coverage = Coverage::default();
        self.shadows
            .accumulate_coverage(start, end, &mut coverage, &mut |work| budget.consume(work))?;
        if !coverage.opaque()
            && let Some(model) = self.model
        {
            model
                .accumulate_coverage(start, end, &mut coverage, &mut |work| budget.consume(work))?;
        }
        self.finish(start, end, skip_owner, coverage, budget)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn visibility_to_source(
        &self,
        start: DVec3,
        end: DVec3,
        skip_owner: Option<u32>,
        boundary: Option<&super::radiance::SourceBoundary>,
        error: f64,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        let mut coverage = Coverage::default();
        super::radiance::source_coverage(
            self.shadows,
            start,
            end,
            boundary,
            error,
            &mut coverage,
            budget,
        )?;
        if !coverage.opaque()
            && let Some(model) = self.model
        {
            super::radiance::source_coverage(
                model,
                start,
                end,
                boundary,
                error,
                &mut coverage,
                budget,
            )?;
        }
        self.finish(start, end, skip_owner, coverage, budget)
    }

    fn finish(
        &self,
        start: DVec3,
        end: DVec3,
        skip_owner: Option<u32>,
        mut coverage: Coverage,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        if !coverage.opaque() {
            self.terrain
                .accumulate_coverage(start, end, &mut coverage, &mut |work| budget.consume(work))?;
        }
        if !coverage.opaque() {
            self.props
                .accumulate_coverage(start, end, skip_owner, &mut coverage, &mut |work| {
                    budget.consume(work)
                })?;
        }
        Ok(coverage.visibility())
    }

    pub(super) fn sky_visibility(
        &self,
        start: DVec3,
        end: DVec3,
        fraction: f64,
        skip_owner: Option<u32>,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err("Sky transport endpoint must be finite and within its ray".into());
        }
        let mut coverage = Coverage::default();
        for hulls in std::iter::once(self.shadows).chain(self.model) {
            if coverage.opaque() {
                break;
            }
            hulls.accumulate_sky_coverage(start, end, fraction, &mut coverage, &mut |work| {
                budget.consume(work)
            })?;
        }
        if !coverage.opaque() {
            self.terrain.accumulate_sky_coverage(
                start,
                end,
                fraction,
                &mut coverage,
                &mut |work| budget.consume(work),
            )?;
        }
        if !coverage.opaque() {
            self.props.accumulate_sky_coverage(
                start,
                end,
                fraction,
                skip_owner,
                &mut coverage,
                &mut |work| budget.consume(work),
            )?;
        }
        Ok(coverage.visibility())
    }
}
