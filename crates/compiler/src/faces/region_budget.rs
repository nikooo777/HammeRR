use crate::Control;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Limits {
    pub faces: usize,
    pub pending: usize,
    pub points: usize,
    pub work: usize,
}

pub(super) struct Budget<'a> {
    limits: Option<Limits>,
    faces: usize,
    points: usize,
    work: usize,
    control: &'a Control,
}

impl<'a> Budget<'a> {
    pub(super) fn new(limits: Option<Limits>, control: &'a Control) -> Self {
        Self {
            limits,
            faces: 0,
            points: 0,
            work: 0,
            control,
        }
    }

    pub(super) fn charge(&mut self, work: usize, points: usize) -> Result<(), String> {
        let Some(limits) = self.limits else {
            return Ok(());
        };
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(work)
            .filter(|&n| n <= limits.work)
            .ok_or("Sky region faces exceeded their traversal work budget")?;
        self.points = self
            .points
            .checked_add(points)
            .filter(|&n| n <= limits.points)
            .ok_or("Sky region faces exceeded their winding allocation budget")?;
        Ok(())
    }

    pub(super) fn split(&mut self, points: usize) -> Result<(), String> {
        let slots = points
            .checked_mul(7)
            .and_then(|n| n.checked_add(12))
            .ok_or("Sky region face split allocation count overflow")?;
        self.charge(slots, slots)
    }

    pub(super) fn pending(&self, count: usize) -> Result<(), String> {
        if self.limits.is_some_and(|limits| count > limits.pending) {
            return Err("Sky region faces exceeded their pending fragment budget".into());
        }
        Ok(())
    }

    pub(super) fn face(&mut self) -> Result<(), String> {
        let Some(limits) = self.limits else {
            return Ok(());
        };
        self.faces = self
            .faces
            .checked_add(1)
            .filter(|&n| n <= limits.faces)
            .ok_or("Sky region faces exceeded their output fragment budget")?;
        self.charge(1, 0)
    }
}
