use std::io;
use std::ops::Range;

use super::invalid;

pub const MAX_HARDWARE_VERTICES: usize = 6_000_000;
pub const MAX_HARDWARE_SECTIONS: usize = 65_536;
pub const MAX_HARDWARE_WORK: usize = 32_000_000;
pub const MAX_HARDWARE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct HardwareLimits {
    pub vertices: usize,
    pub sections: usize,
    pub work: usize,
    pub bytes: usize,
}

impl Default for HardwareLimits {
    fn default() -> Self {
        Self {
            vertices: MAX_HARDWARE_VERTICES,
            sections: MAX_HARDWARE_SECTIONS,
            work: MAX_HARDWARE_WORK,
            bytes: MAX_HARDWARE_BYTES,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HardwareCharge {
    pub bytes: usize,
    pub work: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HardwareSection {
    pub body_part: usize,
    pub model: usize,
    pub lod: usize,
    pub mesh: usize,
    pub group: usize,
    pub part: usize,
    pub vertices: Range<usize>,
}

#[derive(Debug)]
pub struct HardwareLayout {
    pub checksum: i32,
    pub lods: usize,
    pub root_lod: usize,
    /// LOD, body part, model, mesh, then strip-group order.
    pub sections: Vec<HardwareSection>,
    /// Indices into root-LOD-0, fixed-up `Model::vertices`, including repeats.
    pub vertices: Vec<u32>,
}

impl HardwareLayout {
    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.sections.capacity() * std::mem::size_of::<HardwareSection>()
            + self.vertices.capacity() * std::mem::size_of::<u32>()
    }
}

pub(super) struct Budget<'a> {
    control: Option<Control<'a>>,
}

struct Control<'a> {
    limits: HardwareLimits,
    charge: &'a mut dyn FnMut(HardwareCharge) -> io::Result<()>,
    used: HardwareCharge,
    vertices: usize,
    sections: usize,
}

impl<'a> Budget<'a> {
    pub fn ordinary() -> Self {
        Self { control: None }
    }

    pub fn hardware(
        limits: HardwareLimits,
        charge: &'a mut dyn FnMut(HardwareCharge) -> io::Result<()>,
    ) -> io::Result<Self> {
        if limits.vertices > MAX_HARDWARE_VERTICES
            || limits.sections > MAX_HARDWARE_SECTIONS
            || limits.work > MAX_HARDWARE_WORK
            || limits.bytes > MAX_HARDWARE_BYTES
        {
            return Err(invalid("model hardware limits exceed their hard bounds"));
        }
        let mut result = Self {
            control: Some(Control {
                limits,
                charge,
                used: HardwareCharge::default(),
                vertices: 0,
                sections: 0,
            }),
        };
        result.charge(0, 0)?;
        Ok(result)
    }

    pub fn enabled(&self) -> bool {
        self.control.is_some()
    }

    pub fn charge(&mut self, bytes: usize, work: usize) -> io::Result<()> {
        let Some(control) = &mut self.control else {
            return Ok(());
        };
        let next = HardwareCharge {
            bytes: control
                .used
                .bytes
                .checked_add(bytes)
                .ok_or_else(|| invalid("model hardware allocation accounting overflow"))?,
            work: control
                .used
                .work
                .checked_add(work)
                .ok_or_else(|| invalid("model hardware work accounting overflow"))?,
        };
        if next.bytes > control.limits.bytes || next.work > control.limits.work {
            return Err(invalid(
                "model hardware traversal exceeds its resource limit",
            ));
        }
        (control.charge)(HardwareCharge { bytes, work })?;
        control.used = next;
        Ok(())
    }

    pub fn array<T>(&mut self, count: usize) -> io::Result<()> {
        let bytes = count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| invalid("model hardware allocation size overflow"))?;
        self.charge(bytes, count)
    }

    pub fn checkpoint(&mut self, index: usize) -> io::Result<()> {
        if index.is_multiple_of(1024) {
            self.charge(0, 0)?;
        }
        Ok(())
    }

    pub fn group(&mut self, count: usize) -> io::Result<()> {
        let Some(control) = &mut self.control else {
            return Ok(());
        };
        control.vertices = control
            .vertices
            .checked_add(count)
            .ok_or_else(|| invalid("model hardware vertex count overflow"))?;
        control.sections = control
            .sections
            .checked_add(1)
            .ok_or_else(|| invalid("model hardware section count overflow"))?;
        if control.vertices > control.limits.vertices || control.sections > control.limits.sections
        {
            return Err(invalid("model hardware groups exceed their resource limit"));
        }
        Ok(())
    }

    pub fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> io::Result<()> {
        let required = values
            .len()
            .checked_add(additional)
            .ok_or_else(|| invalid("model hardware vector length overflow"))?;
        if required > values.capacity() {
            let capacity = required.max(values.capacity().saturating_mul(2));
            self.array::<T>(capacity)?;
            values
                .try_reserve_exact(capacity - values.len())
                .map_err(|_| invalid("model hardware allocation cannot be reserved"))?;
        }
        Ok(())
    }
}
