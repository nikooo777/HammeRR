use crate::Control;

const MAX_WORK: usize = 4_000_000;
const MAX_FRAGMENTS: usize = 65_536;
const MAX_POINTS: usize = 1_000_000;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub work: usize,
    pub fragments: usize,
    pub points: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            work: MAX_WORK,
            fragments: MAX_FRAGMENTS,
            points: MAX_POINTS,
        }
    }
}

#[cfg_attr(not(test), derive(Default))]
pub(super) struct Budget {
    limits: Limits,
    work: usize,
    fragments: usize,
    points: usize,
    #[cfg(test)]
    visits: usize,
    #[cfg(test)]
    cancel_after_visits: Option<usize>,
}

#[cfg(test)]
impl Default for Budget {
    fn default() -> Self {
        let config = TEST_CONFIG.with(std::cell::Cell::get).unwrap_or_default();
        Self {
            limits: config.limits,
            work: 0,
            fragments: 0,
            points: 0,
            visits: 0,
            cancel_after_visits: config.cancel_after_visits,
        }
    }
}

impl Budget {
    fn charge(
        &mut self,
        work: usize,
        fragments: usize,
        points: usize,
        control: &Control,
    ) -> Result<(), String> {
        control.checkpoint()?;
        let work = self
            .work
            .checked_add(work)
            .filter(|&count| count <= self.limits.work)
            .ok_or("Water undersides exceed HammeRR's traversal work budget")?;
        let fragments = self
            .fragments
            .checked_add(fragments)
            .filter(|&count| count <= self.limits.fragments)
            .ok_or("Water undersides exceed HammeRR's generated fragment budget")?;
        let points = self
            .points
            .checked_add(points)
            .filter(|&count| count <= self.limits.points)
            .ok_or("Water undersides exceed HammeRR's winding point allocation budget")?;
        self.work = work;
        self.fragments = fragments;
        self.points = points;
        Ok(())
    }

    pub fn polygon(&mut self, points: usize, control: &Control) -> Result<(), String> {
        self.charge(points.saturating_add(1), 1, points, control)
    }

    pub fn visit(&mut self, points: usize, control: &Control) -> Result<(), String> {
        self.charge(points.saturating_add(1), 0, 0, control)?;
        #[cfg(test)]
        {
            self.visits += 1;
            if self.cancel_after_visits == Some(self.visits) {
                control.cancel()?;
                control.checkpoint()?;
            }
        }
        Ok(())
    }

    pub fn split(&mut self, points: usize, control: &Control) -> Result<(), String> {
        // Include both growing output vectors and the distance scratch array.
        let outputs = points.saturating_mul(5).saturating_add(8);
        self.charge(
            points.saturating_mul(4).saturating_add(1),
            2,
            outputs,
            control,
        )
    }

    pub fn merge(
        &mut self,
        a: usize,
        b: usize,
        leaves: usize,
        control: &Control,
    ) -> Result<(), String> {
        let points = a.saturating_add(b);
        let sorting = leaves.saturating_mul(leaves.checked_ilog2().unwrap_or(0) as usize + 1);
        let work = a
            .saturating_mul(b)
            .saturating_add(points.saturating_mul(2))
            .saturating_add(sorting)
            .saturating_add(1);
        self.charge(work, 0, points.saturating_mul(2), control)
    }

    pub fn clone_points(&mut self, points: usize, control: &Control) -> Result<(), String> {
        self.charge(points.saturating_add(1), 0, points, control)
    }

    #[cfg(test)]
    pub(super) fn usage(&self) -> (usize, usize, usize) {
        (self.work, self.fragments, self.points)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
pub(super) struct TestConfig {
    pub limits: Limits,
    pub cancel_after_visits: Option<usize>,
}

#[cfg(test)]
thread_local! {
    static TEST_CONFIG: std::cell::Cell<Option<TestConfig>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn with_config<T>(config: TestConfig, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<TestConfig>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_CONFIG.with(|slot| slot.set(self.0));
        }
    }
    let previous = TEST_CONFIG.with(|slot| slot.replace(Some(config)));
    let _restore = Restore(previous);
    operation()
}
