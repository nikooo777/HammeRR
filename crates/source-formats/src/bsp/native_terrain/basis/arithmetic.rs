use std::io;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::bsp::{NativeDisplacementRounding, invalid};

pub(super) const EPSILON: f32 = f32::from_bits(0x2edb_e6ff);

#[derive(Clone, Copy)]
pub(super) enum NormalizeOrder {
    ZFirst,
    ZFirstYx,
    XyFirst,
}

#[derive(Clone, Copy)]
pub(super) struct Arithmetic;

#[cfg(target_arch = "x86_64")]
macro_rules! binary {
    ($name:ident, $instruction:literal) => {
        #[inline]
        pub fn $name(&self, mut left: f32, right: f32) -> f32 {
            unsafe { std::arch::asm!(concat!($instruction," {left}, {right}"),left=inout(xmm_reg)left,right=in(xmm_reg)right,options(nostack,nomem,preserves_flags)); }
            left
        }
    };
}

impl Arithmetic {
    #[cfg(target_arch = "x86_64")]
    binary!(add, "addss");
    #[cfg(target_arch = "x86_64")]
    binary!(sub, "subss");
    #[cfg(target_arch = "x86_64")]
    binary!(mul, "mulss");
    #[cfg(target_arch = "x86_64")]
    binary!(div, "divss");

    #[cfg(target_arch = "x86_64")]
    pub fn sqrt(&self, mut value: f32) -> f32 {
        unsafe {
            std::arch::asm!("sqrtss {value}, {value}",value=inout(xmm_reg)value,options(nostack,nomem,preserves_flags));
        }
        value
    }
    #[cfg(target_arch = "x86_64")]
    pub fn rsqrt(&self, mut value: f32) -> f32 {
        unsafe {
            std::arch::asm!("rsqrtss {value}, {value}",value=inout(xmm_reg)value,options(nostack,nomem,preserves_flags));
        }
        value
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn add(&self, _a: f32, _b: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn sub(&self, _a: f32, _b: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn mul(&self, _a: f32, _b: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn div(&self, _a: f32, _b: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn sqrt(&self, _a: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn rsqrt(&self, _a: f32) -> f32 {
        unreachable!("x86-64 profile admission required")
    }

    pub fn vec_add(&self, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| self.add(a[axis], b[axis]))
    }
    pub fn vec_sub(&self, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| self.sub(a[axis], b[axis]))
    }
    pub fn cross(&self, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            self.sub(self.mul(a[1], b[2]), self.mul(a[2], b[1])),
            self.sub(self.mul(a[2], b[0]), self.mul(a[0], b[2])),
            self.sub(self.mul(a[0], b[1]), self.mul(a[1], b[0])),
        ]
    }
    pub fn vector_inverse_length(&self, v: [f32; 3], order: NormalizeOrder) -> f32 {
        let x = self.mul(v[0], v[0]);
        let y = self.mul(v[1], v[1]);
        let xy = match order {
            NormalizeOrder::ZFirstYx => self.add(y, x),
            _ => self.add(x, y),
        };
        let z = self.add(self.mul(v[2], v[2]), EPSILON);
        let q = match order {
            NormalizeOrder::ZFirst | NormalizeOrder::ZFirstYx => self.add(z, xy),
            NormalizeOrder::XyFirst => self.add(xy, z),
        };
        self.inverse_length(q)
    }
    pub fn normalize(&self, v: [f32; 3], order: NormalizeOrder) -> [f32; 3] {
        let inverse = self.vector_inverse_length(v, order);
        [
            self.mul(v[0], inverse),
            self.mul(v[1], inverse),
            self.mul(inverse, v[2]),
        ]
    }
    pub fn inverse_length(&self, q: f32) -> f32 {
        let r = self.rsqrt(q);
        let correction = self.mul(self.sub(3.0, self.mul(self.mul(r, r), q)), 0.5);
        self.mul(r, correction)
    }
}

pub(super) struct MxcsrGuard {
    original: u32,
    _thread: PhantomData<Rc<()>>,
}

impl MxcsrGuard {
    pub fn selected(
        rounding: NativeDisplacementRounding,
        ftz: bool,
        daz: bool,
    ) -> io::Result<Self> {
        let rounding = match rounding {
            NativeDisplacementRounding::NearestEven => 0,
            NativeDisplacementRounding::TowardNegative => 1,
            NativeDisplacementRounding::TowardPositive => 2,
            NativeDisplacementRounding::TowardZero => 3,
        };
        let selected = 0x1f80 | (rounding << 13) | (u32::from(ftz) << 15) | (u32::from(daz) << 6);
        #[cfg(target_arch = "x86_64")]
        {
            #[repr(align(16))]
            struct Saved([u8; 512]);
            let mut saved = Saved([0; 512]);
            unsafe {
                std::arch::asm!("fxsave64 [{value}]",value=in(reg)&mut saved,options(nostack,preserves_flags));
            }
            let mask = u32::from_le_bytes(
                saved.0[28..32]
                    .try_into()
                    .map_err(|_| invalid("Incomplete native MXCSR mask"))?,
            );
            if mask == 0 || selected & !mask != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Native basis CPU does not support the selected MXCSR controls",
                ));
            }
        }
        Self::raw(selected)
    }
    #[cfg(target_arch = "x86_64")]
    pub fn raw(selected: u32) -> io::Result<Self> {
        let original = read_mxcsr()?;
        unsafe {
            std::arch::asm!("ldmxcsr [{value}]",value=in(reg)&selected,options(nostack,preserves_flags));
        }
        let guard = Self {
            original,
            _thread: PhantomData,
        };
        if read_mxcsr()? != selected {
            return Err(invalid("Native basis MXCSR selection did not take effect"));
        }
        Ok(guard)
    }
    #[cfg(not(target_arch = "x86_64"))]
    pub fn raw(_selected: u32) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Native basis prediction requires x86-64 scalar SSE/RSQRTSS",
        ))
    }
    pub fn original(&self) -> u32 {
        self.original
    }
}
impl Drop for MxcsrGuard {
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::asm!("ldmxcsr [{value}]",value=in(reg)&self.original,options(nostack,preserves_flags));
        }
    }
}
#[cfg(target_arch = "x86_64")]
pub(super) fn read_mxcsr() -> io::Result<u32> {
    let mut value = 0;
    unsafe {
        std::arch::asm!("stmxcsr [{value}]",value=in(reg)&mut value,options(nostack,preserves_flags));
    }
    Ok(value)
}
#[cfg(not(target_arch = "x86_64"))]
pub(super) fn read_mxcsr() -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Native basis prediction requires x86-64",
    ))
}
