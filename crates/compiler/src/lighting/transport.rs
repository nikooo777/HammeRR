use glam::DVec3;

use super::Occluder;
use super::bake::Budget;
use super::props::PropTriangle;
use super::shadows::ShadowIndex;
use super::terrain_shadows::TerrainTriangle;

#[derive(Default)]
pub(super) struct Coverage(u8);

impl Coverage {
    pub(super) fn add(&mut self, coverage: u8) {
        self.0 = self.0.saturating_add(coverage);
    }

    pub(super) fn opaque(&self) -> bool {
        self.0 == u8::MAX
    }

    pub(super) fn visibility(&self) -> f64 {
        f64::from(u8::MAX - self.0) / f64::from(u8::MAX)
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
    ) -> Result<f64, String> {
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
    ) -> Result<f64, String> {
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
    ) -> Result<f64, String> {
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
    ) -> Result<f64, String> {
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
