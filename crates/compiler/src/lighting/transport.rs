use glam::DVec3;

use super::Occluder;
use super::bake::Budget;
use super::props::PropTriangle;
use super::shadows::ShadowIndex;
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

pub(super) struct Scene<'a> {
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<TerrainTriangle>,
    pub props: &'a ShadowIndex<PropTriangle>,
    pub model: Option<&'a ShadowIndex<Occluder>>,
}

impl Scene<'_> {
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
