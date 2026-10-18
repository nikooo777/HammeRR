use crate::{Control, winding::Winding};

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Limits {
    pub nodes: usize,
    pub pending: usize,
    pub points: usize,
    pub work: usize,
}

pub(super) struct Budget<'a> {
    limits: Option<Limits>,
    points: usize,
    work: usize,
    control: Option<&'a Control>,
}

impl<'a> Budget<'a> {
    pub(super) fn new(limits: Option<Limits>, control: &'a Control) -> Self {
        Self {
            limits,
            points: 0,
            work: 0,
            control: Some(control),
        }
    }

    pub(super) fn unlimited() -> Self {
        Self {
            limits: None,
            points: 0,
            work: 0,
            control: None,
        }
    }

    pub(super) fn charge(&mut self, work: usize, points: usize) -> Result<(), String> {
        let Some(limits) = self.limits else {
            return Ok(());
        };
        if let Some(control) = self.control {
            control.checkpoint()?;
        }
        self.work = self
            .work
            .checked_add(work)
            .filter(|&n| n <= limits.work)
            .ok_or("BSP tree exceeded its traversal work budget")?;
        self.points = self
            .points
            .checked_add(points)
            .filter(|&n| n <= limits.points)
            .ok_or("BSP tree exceeded its geometry allocation budget")?;
        Ok(())
    }

    pub(super) fn split(&mut self, winding: &Winding) -> Result<(), String> {
        let points = winding
            .0
            .len()
            .checked_mul(7)
            .and_then(|n| n.checked_add(12))
            .ok_or("BSP tree split allocation count overflow")?;
        self.charge(points, points)
    }

    pub(super) fn nodes(&self, count: usize) -> Result<(), String> {
        if self.limits.is_some_and(|limits| count > limits.nodes) {
            return Err("BSP tree exceeded its node allocation budget".into());
        }
        Ok(())
    }

    pub(super) fn pending(&self, count: usize) -> Result<(), String> {
        if self.limits.is_some_and(|limits| count > limits.pending) {
            return Err("BSP tree exceeded its pending fragment budget".into());
        }
        Ok(())
    }
}
