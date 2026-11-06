use std::io;

use super::{DisplacementInfo, Displacements, invalid, range};

const SIGN: u32 = 1 << 31;
const FRACTION: u32 = (1 << 23) - 1;
const HIDDEN_BIT: u128 = 1 << 23;
const MAX_SIGNIFICAND: u128 = (1 << 24) - 1;
const MAX_EXACT_ADD_EXPONENT_DIFFERENCE: i32 = u128::BITS as i32 - 24;
const FIND_START_DISTANCE_BOUND: f32 = 1_000_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDisplacementRounding {
    NearestEven,
    TowardNegative,
    TowardPositive,
    TowardZero,
}

impl NativeDisplacementRounding {
    pub const ALL: [Self; 4] = [
        Self::NearestEven,
        Self::TowardNegative,
        Self::TowardPositive,
        Self::TowardZero,
    ];
}

/// The installed Linux64 CS:S build's separate scalar f32 operations.
/// Subnormal operands, exact nonzero underflow and overflow are unsupported.
/// Corner selection must agree across all rounding modes; kernel rounding is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDisplacementProfile {
    CssLinux64Build25338318 {
        rounding: NativeDisplacementRounding,
    },
}

#[derive(Clone, Copy)]
struct Arithmetic {
    rounding: NativeDisplacementRounding,
}

#[derive(Clone, Copy)]
struct Operand {
    negative: bool,
    significand: u128,
    exponent: i32,
}

impl Operand {
    fn decode(value: f32) -> io::Result<Self> {
        let bits = value.to_bits();
        let exponent = (bits >> 23) & 0xff;
        let fraction = bits & FRACTION;
        if exponent == 0xff {
            return Err(invalid(
                "native displacement arithmetic requires finite operands",
            ));
        }
        if exponent == 0 && fraction != 0 {
            return Err(invalid(
                "native displacement arithmetic does not admit subnormal operands",
            ));
        }
        Ok(Self {
            negative: bits & SIGN != 0,
            significand: if exponent == 0 {
                0
            } else {
                HIDDEN_BIT | u128::from(fraction)
            },
            exponent: exponent as i32 - 150,
        })
    }
}

impl Arithmetic {
    fn zero(self, negative: bool) -> f32 {
        f32::from_bits(if negative { SIGN } else { 0 })
    }

    fn round(self, negative: bool, significand: u128, exponent: i32) -> io::Result<f32> {
        if significand == 0 {
            return Ok(self.zero(negative));
        }
        let highest_bit = u128::BITS as i32 - 1 - significand.leading_zeros() as i32;
        let mut native_exponent = exponent + highest_bit;
        if native_exponent < -126 {
            return Err(invalid(
                "native displacement arithmetic does not admit nonzero underflow",
            ));
        }
        if native_exponent > 127
            || native_exponent == 127
                && highest_bit > 23
                && significand > MAX_SIGNIFICAND << (highest_bit - 23)
        {
            return Err(invalid(
                "native displacement arithmetic exceeds finite floats",
            ));
        }
        let mut rounded = if highest_bit > 23 {
            let shift = highest_bit - 23;
            let truncated = significand >> shift;
            let remainder = significand & ((1u128 << shift) - 1);
            let halfway = 1u128 << (shift - 1);
            let increment = match self.rounding {
                NativeDisplacementRounding::NearestEven => {
                    remainder > halfway || remainder == halfway && truncated & 1 != 0
                }
                NativeDisplacementRounding::TowardNegative => negative && remainder != 0,
                NativeDisplacementRounding::TowardPositive => !negative && remainder != 0,
                NativeDisplacementRounding::TowardZero => false,
            };
            truncated + u128::from(increment)
        } else {
            significand << (23 - highest_bit)
        };
        if rounded > MAX_SIGNIFICAND {
            rounded >>= 1;
            native_exponent += 1;
        }
        if native_exponent > 127 {
            return Err(invalid(
                "native displacement arithmetic exceeds finite floats",
            ));
        }
        Ok(f32::from_bits(
            if negative { SIGN } else { 0 }
                | ((native_exponent + 127) as u32) << 23
                | rounded as u32 & FRACTION,
        ))
    }

    fn add(self, left: f32, right: f32) -> io::Result<f32> {
        let a = Operand::decode(left)?;
        let b = Operand::decode(right)?;
        if a.significand == 0 && b.significand == 0 {
            return Ok(self.zero(
                a.negative && b.negative
                    || a.negative != b.negative
                        && self.rounding == NativeDisplacementRounding::TowardNegative,
            ));
        }
        if a.significand == 0 {
            return Ok(right);
        }
        if b.significand == 0 {
            return Ok(left);
        }
        let difference = a.exponent - b.exponent;
        if difference.abs() > MAX_EXACT_ADD_EXPONENT_DIFFERENCE {
            let (dominant, small, value) = if difference > 0 {
                (a, b, left)
            } else {
                (b, a, right)
            };
            return self.add_distant(dominant, small, value);
        }
        let exponent = a.exponent.min(b.exponent);
        let a_significand = a.significand << (a.exponent - exponent);
        let b_significand = b.significand << (b.exponent - exponent);
        let (negative, significand) = if a.negative == b.negative {
            (a.negative, a_significand + b_significand)
        } else if a_significand > b_significand {
            (a.negative, a_significand - b_significand)
        } else if b_significand > a_significand {
            (b.negative, b_significand - a_significand)
        } else {
            (
                self.rounding == NativeDisplacementRounding::TowardNegative,
                0,
            )
        };
        self.round(negative, significand, exponent)
    }

    fn add_distant(self, dominant: Operand, small: Operand, value: f32) -> io::Result<f32> {
        let same_sign = dominant.negative == small.negative;
        if same_sign && value.to_bits() & !SIGN == 0x7f7f_ffff {
            return Err(invalid(
                "native displacement arithmetic exceeds finite floats",
            ));
        }
        let step = match self.rounding {
            NativeDisplacementRounding::NearestEven => false,
            NativeDisplacementRounding::TowardNegative => small.negative,
            NativeDisplacementRounding::TowardPositive => !small.negative,
            NativeDisplacementRounding::TowardZero => !same_sign,
        };
        if !step {
            return Ok(value);
        }
        let bits = if same_sign {
            value.to_bits() + 1
        } else {
            value.to_bits() - 1
        };
        let result = f32::from_bits(bits);
        Operand::decode(result)?;
        Ok(result)
    }

    fn subtract(self, left: f32, right: f32) -> io::Result<f32> {
        self.add(left, f32::from_bits(right.to_bits() ^ SIGN))
    }

    fn multiply(self, left: f32, right: f32) -> io::Result<f32> {
        let a = Operand::decode(left)?;
        let b = Operand::decode(right)?;
        self.round(
            a.negative != b.negative,
            a.significand * b.significand,
            a.exponent + b.exponent,
        )
    }

    fn start_corner(self, info: DisplacementInfo, quad: [[f32; 3]; 4]) -> io::Result<usize> {
        let mut minimum = FIND_START_DISTANCE_BOUND.to_bits();
        let mut first = None;
        for (index, point) in quad.iter().enumerate() {
            let dx = self.subtract(info.start[0], point[0])?;
            let dy = self.subtract(info.start[1], point[1])?;
            let dz = self.subtract(info.start[2], point[2])?;
            let xx = self.multiply(dx, dx)?;
            let yy = self.multiply(dy, dy)?;
            let zz = self.multiply(dz, dz)?;
            let xy = self.add(yy, xx)?;
            let distance = self.add(zz, xy)?;
            if distance.to_bits() & !SIGN < minimum {
                minimum = distance.to_bits() & !SIGN;
                first = Some(index);
            }
        }
        first.ok_or_else(|| invalid("native displacement start exceeds its corner distance bound"))
    }
}

impl Displacements {
    /// Reconstructs BSP-loaded positions with explicit native rounding, independent
    /// of the caller's floating-point state. Elevation and subdivision offsets are
    /// positive zero. Does not apply runtime LOD updates.
    pub fn native_positions(
        &self,
        index: usize,
        quad: [[f32; 3]; 4],
        profile: NativeDisplacementProfile,
    ) -> io::Result<Vec<[f32; 3]>> {
        let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
        let arithmetic = Arithmetic { rounding };
        let info = *self
            .infos
            .get(index)
            .ok_or_else(|| invalid("displacement index is invalid"))?;
        let size = info.grid_size()?;
        let vertices = &self.vertices[range(info.vertex_start, size * size, self.vertices.len())?];
        let first = arithmetic.start_corner(info, quad)?;
        for rounding in NativeDisplacementRounding::ALL {
            if rounding != arithmetic.rounding
                && (Arithmetic { rounding }).start_corner(info, quad)? != first
            {
                return Err(invalid(
                    "native displacement start corner depends on rounding mode",
                ));
            }
        }
        let [a, b, c, d] = std::array::from_fn(|offset| quad[(first + offset) % 4]);
        let reciprocal = f32::from_bits(((127 - info.power) as u32) << 23);
        let mut first_edge = [0.0; 3];
        let mut second_edge = [0.0; 3];
        for axis in 0..3 {
            first_edge[axis] =
                arithmetic.multiply(arithmetic.subtract(b[axis], a[axis])?, reciprocal)?;
            second_edge[axis] =
                arithmetic.multiply(arithmetic.subtract(c[axis], d[axis])?, reciprocal)?;
        }
        let mut result = Vec::with_capacity(size * size);
        for row in 0..size {
            let mut left = [0.0; 3];
            let mut step = [0.0; 3];
            for axis in 0..3 {
                left[axis] =
                    arithmetic.add(arithmetic.multiply(first_edge[axis], row as f32)?, a[axis])?;
                let right =
                    arithmetic.add(arithmetic.multiply(second_edge[axis], row as f32)?, d[axis])?;
                step[axis] =
                    arithmetic.multiply(arithmetic.subtract(right, left[axis])?, reciprocal)?;
            }
            for col in 0..size {
                let vertex = vertices[row * size + col];
                let mut point = [0.0; 3];
                for axis in 0..3 {
                    let base =
                        arithmetic.add(arithmetic.multiply(step[axis], col as f32)?, left[axis])?;
                    let distance = arithmetic.multiply(vertex.direction[axis], vertex.distance)?;
                    let offset = arithmetic.add(distance, 0.0)?;
                    point[axis] = if axis == 0 {
                        arithmetic.add(offset, base)?
                    } else {
                        arithmetic.add(base, offset)?
                    };
                }
                result.push(point);
            }
        }
        Ok(result)
    }
}
