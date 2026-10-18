use crate::Control;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub portals: usize,
    /// Cumulative allocation slots, including conservative split scratch space.
    pub points: usize,
    pub pending: usize,
    pub work: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            portals: 1_048_576,
            points: 16_000_000,
            pending: 65_536,
            work: 100_000_000,
        }
    }
}

pub(super) struct Budget<'a> {
    pub(super) limits: Limits,
    pub(super) points: usize,
    pub(super) work: usize,
    control: &'a Control,
}

impl<'a> Budget<'a> {
    pub(super) fn new(limits: Limits, control: &'a Control) -> Self {
        Self {
            limits,
            points: 0,
            work: 0,
            control,
        }
    }

    pub(super) fn visit(&mut self, points: usize) -> Result<(), String> {
        self.charge(
            points.checked_add(1).ok_or("Portal work count overflow")?,
            0,
        )
    }

    fn charge(&mut self, work: usize, points: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        let work = self
            .work
            .checked_add(work)
            .filter(|&count| count <= self.limits.work)
            .ok_or("Portal construction exceeded its traversal work budget")?;
        let points = self
            .points
            .checked_add(points)
            .filter(|&count| count <= self.limits.points)
            .ok_or("Portal construction exceeded its winding allocation budget")?;
        self.work = work;
        self.points = points;
        Ok(())
    }

    pub(super) fn allocate(&mut self, points: usize) -> Result<(), String> {
        self.charge(
            points.checked_add(1).ok_or("Portal work count overflow")?,
            points,
        )
    }

    pub(super) fn split(&mut self, points: usize) -> Result<(), String> {
        let allocation = points
            .checked_mul(7)
            .and_then(|n| n.checked_add(12))
            .ok_or("Portal split allocation count overflow")?;
        self.allocate(allocation)
    }

    pub(super) fn pending<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), String> {
        self.visit(additional)?;
        reserve(values, additional, self.limits.pending, "pending fragment")
    }

    pub(super) fn output<T>(&mut self, values: &mut Vec<T>) -> Result<(), String> {
        self.visit(1)?;
        reserve(values, 1, self.limits.portals, "output portal")
    }
}

fn reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    limit: usize,
    kind: &str,
) -> Result<(), String> {
    let needed = values
        .len()
        .checked_add(additional)
        .filter(|&count| count <= limit)
        .ok_or_else(|| format!("Portal construction exceeded its {kind} budget"))?;
    if needed > values.capacity() {
        let capacity = values
            .capacity()
            .saturating_mul(2)
            .max(4)
            .min(limit)
            .max(needed);
        values
            .try_reserve_exact(capacity - values.len())
            .map_err(|error| format!("Could not reserve portal {kind} storage: {error}"))?;
    }
    Ok(())
}
