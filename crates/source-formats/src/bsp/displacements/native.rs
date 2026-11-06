use std::io;

use super::{DisplacementInfo, Displacements, invalid, range};

const SIGN: u32 = 1 << 31;
const FRACTION: u32 = (1 << 23) - 1;
const HIDDEN_BIT: u128 = 1 << 23;
const MAX_SIGNIFICAND: u128 = (1 << 24) - 1;
const MAX_EXACT_ADD_EXPONENT_DIFFERENCE: i32 = u128::BITS as i32 - 24;
const FIND_START_DISTANCE_BOUND: f32 = 1_000_000_000.0;
const MAX_MAPPING_DIMENSION: u32 = 1 << 15;
const MIN_NORMAL_CROSS_COMPONENT: u32 = (127 - 50) << 23;
const MAX_NORMAL_CROSS_COMPONENT: u32 = (127 + 50) << 23;
const MIN_NORMAL_CROSS_MAXIMUM: u32 = (127 - 6) << 23;
const MAX_NORMAL_COMPONENT_RATIO: f32 = 256.0;
const NORMAL_RATIO_EVENTS_PER_PATCH: usize = 352;
const NORMAL_STENCIL_RATIO_EVENTS: usize = 9;
const MAX_NORMAL_RATIO_EVENTS: usize = 1 << 20;
const ALPHA_NORMALIZATION: f32 = f32::from_bits(0x3b80_8081);
const ALPHA_BYTE_MAXIMUM: f32 = 255.0;
const ALPHA_ROUNDING_BIAS: f32 = 8_388_608.0;
const SECONDARY_ALPHA_THRESHOLD: f32 = 382.5;

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

/// Encodes the alpha byte written by the installed BSP-load static-mesh route
/// under the selected profile's rounding mode. This does not establish the
/// runtime mode or certify shader interpolation and final pixels.
pub fn native_alpha_byte(alpha: f32, profile: NativeDisplacementProfile) -> io::Result<u8> {
    let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
    let arithmetic = Arithmetic { rounding };
    let normalized = arithmetic.multiply(alpha, ALPHA_NORMALIZATION)?;
    let normalized = if normalized.to_bits() & SIGN != 0 {
        0.0
    } else if normalized.to_bits() >= 1.0f32.to_bits() {
        1.0
    } else {
        normalized
    };
    let scaled = arithmetic.multiply(normalized, ALPHA_BYTE_MAXIMUM)?;
    let encoded = arithmetic.add(scaled, ALPHA_ROUNDING_BIAS)?;
    Ok(encoded.to_bits() as u8)
}

/// Selects the installed collision route's secondary material from raw alpha
/// values supplied in native triangle index order.
pub fn native_alpha_secondary(
    alphas: [f32; 3],
    profile: NativeDisplacementProfile,
) -> io::Result<bool> {
    let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
    let arithmetic = Arithmetic { rounding };
    let mut sum = 0.0;
    for alpha in alphas {
        sum = arithmetic.add(sum, alpha)?;
    }
    Ok(sum > SECONDARY_ALPHA_THRESHOLD)
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

struct BilinearGrid<const AXES: usize> {
    first: [f32; AXES],
    last: [f32; AXES],
    first_edge: [f32; AXES],
    second_edge: [f32; AXES],
    reciprocal: f32,
}

struct BilinearRow<const AXES: usize> {
    left: [f32; AXES],
    step: [f32; AXES],
}

fn same_coordinate(first: f32, second: f32) -> bool {
    let first = first.to_bits();
    let second = second.to_bits();
    first == second || (first | second) & !SIGN == 0
}

fn same_coordinates<const AXES: usize>(first: [f32; AXES], second: [f32; AXES]) -> bool {
    first
        .into_iter()
        .zip(second)
        .all(|(first, second)| same_coordinate(first, second))
}

impl<const AXES: usize> BilinearGrid<AXES> {
    fn new(arithmetic: Arithmetic, quad: [[f32; AXES]; 4], power: i32) -> io::Result<Self> {
        let [first, next, opposite, last] = quad;
        let reciprocal = f32::from_bits(((127 - power) as u32) << 23);
        let mut first_edge = [0.0; AXES];
        let mut second_edge = [0.0; AXES];
        for axis in 0..AXES {
            first_edge[axis] =
                arithmetic.multiply(arithmetic.subtract(next[axis], first[axis])?, reciprocal)?;
            second_edge[axis] = arithmetic
                .multiply(arithmetic.subtract(opposite[axis], last[axis])?, reciprocal)?;
        }
        Ok(Self {
            first,
            last,
            first_edge,
            second_edge,
            reciprocal,
        })
    }

    fn row(&self, arithmetic: Arithmetic, row: usize) -> io::Result<BilinearRow<AXES>> {
        let mut left = [0.0; AXES];
        let mut step = [0.0; AXES];
        for axis in 0..AXES {
            left[axis] = arithmetic.add(
                arithmetic.multiply(self.first_edge[axis], row as f32)?,
                self.first[axis],
            )?;
            let right = arithmetic.add(
                arithmetic.multiply(self.second_edge[axis], row as f32)?,
                self.last[axis],
            )?;
            step[axis] =
                arithmetic.multiply(arithmetic.subtract(right, left[axis])?, self.reciprocal)?;
        }
        Ok(BilinearRow { left, step })
    }

    fn samples(&self, arithmetic: Arithmetic, size: usize) -> io::Result<Vec<[f32; AXES]>> {
        let mut samples = Vec::with_capacity(size * size);
        for row in 0..size {
            let row = self.row(arithmetic, row)?;
            for column in 0..size {
                samples.push(row.point(arithmetic, column)?);
            }
        }
        Ok(samples)
    }
}

impl<const AXES: usize> BilinearRow<AXES> {
    fn point(&self, arithmetic: Arithmetic, column: usize) -> io::Result<[f32; AXES]> {
        let mut point = [0.0; AXES];
        for (axis, value) in point.iter_mut().enumerate() {
            *value = arithmetic.add(
                arithmetic.multiply(self.step[axis], column as f32)?,
                self.left[axis],
            )?;
        }
        Ok(point)
    }
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

    fn exact_add(self, left: f32, right: f32) -> io::Result<f32> {
        let lower = Arithmetic {
            rounding: NativeDisplacementRounding::TowardNegative,
        }
        .add(left, right)?;
        let upper = Arithmetic {
            rounding: NativeDisplacementRounding::TowardPositive,
        }
        .add(left, right)?;
        if !same_coordinate(lower, upper) {
            return Err(invalid(
                "native displacement certificate requires exact arithmetic",
            ));
        }
        self.add(left, right)
    }

    fn exact_multiply(self, left: f32, right: f32) -> io::Result<f32> {
        let lower = Arithmetic {
            rounding: NativeDisplacementRounding::TowardNegative,
        }
        .multiply(left, right)?;
        let upper = Arithmetic {
            rounding: NativeDisplacementRounding::TowardPositive,
        }
        .multiply(left, right)?;
        if !same_coordinate(lower, upper) {
            return Err(invalid(
                "native displacement certificate requires exact arithmetic",
            ));
        }
        self.multiply(left, right)
    }

    fn exact_cross(
        self,
        origin: [f32; 3],
        first: [f32; 3],
        second: [f32; 3],
    ) -> io::Result<[f32; 3]> {
        let mut first_edge = [0.0; 3];
        let mut second_edge = [0.0; 3];
        for axis in 0..3 {
            let negative_origin = f32::from_bits(origin[axis].to_bits() ^ SIGN);
            first_edge[axis] = self.exact_add(first[axis], negative_origin)?;
            second_edge[axis] = self.exact_add(second[axis], negative_origin)?;
        }
        let mut cross = [0.0; 3];
        for (axis, value) in cross.iter_mut().enumerate() {
            let next = (axis + 1) % 3;
            let last = (axis + 2) % 3;
            let first = self.exact_multiply(first_edge[next], second_edge[last])?;
            let second = self.exact_multiply(first_edge[last], second_edge[next])?;
            *value = self.exact_add(first, f32::from_bits(second.to_bits() ^ SIGN))?;
        }
        Ok(cross)
    }

    fn exact_texture_coordinate(
        self,
        point: [f32; 3],
        texture_vectors: [[f32; 4]; 2],
        reciprocal_mapping: [f32; 2],
    ) -> io::Result<[f32; 2]> {
        let mut result = [0.0; 2];
        for axis in 0..2 {
            let vector = texture_vectors[axis];
            let x = self.exact_multiply(point[0], vector[0])?;
            let y = self.exact_multiply(point[1], vector[1])?;
            let z = self.exact_multiply(point[2], vector[2])?;
            let xy = if axis == 0 {
                self.exact_add(x, y)?
            } else {
                self.exact_add(y, x)?
            };
            let zw = self.exact_add(z, vector[3])?;
            let numerator = self.exact_add(xy, zw)?;
            result[axis] = self.exact_multiply(numerator, reciprocal_mapping[axis])?;
        }
        Ok(result)
    }

    fn stable_start_corner(self, info: DisplacementInfo, quad: [[f32; 3]; 4]) -> io::Result<usize> {
        let first = self.start_corner(info, quad)?;
        for rounding in NativeDisplacementRounding::ALL {
            if rounding != self.rounding
                && (Arithmetic { rounding }).start_corner(info, quad)? != first
            {
                return Err(invalid(
                    "native displacement start corner depends on rounding mode",
                ));
            }
        }
        Ok(first)
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

fn conditioned_normal_signs(cross: [f32; 3]) -> io::Result<([i8; 3], u32)> {
    let mut signs = [0; 3];
    let mut maximum = 0;
    for (axis, component) in cross.into_iter().enumerate() {
        let bits = component.to_bits();
        let magnitude = bits & !SIGN;
        if magnitude != 0 {
            if !(MIN_NORMAL_CROSS_COMPONENT..=MAX_NORMAL_CROSS_COMPONENT).contains(&magnitude) {
                return Err(invalid(
                    "native displacement normal components exceed the supported conditioning range",
                ));
            }
            signs[axis] = if bits & SIGN == 0 { 1 } else { -1 };
            maximum = maximum.max(magnitude);
        }
    }
    if maximum < MIN_NORMAL_CROSS_MAXIMUM {
        return Err(invalid(
            "native displacement normal triangles are collapsed or poorly conditioned",
        ));
    }
    Ok((signs, maximum))
}

fn common_normal_support(first: [i8; 3], second: [i8; 3]) -> [i8; 3] {
    std::array::from_fn(|axis| {
        if first[axis] == second[axis] {
            first[axis]
        } else {
            0
        }
    })
}

impl DisplacementInfo {
    /// Certifies exact edge/cross arithmetic and conditioned inputs for the native
    /// cell-normal stencil, independently of rounding mode. Nonzero cross components
    /// must lie in [2^-50, 2^50], with at least one magnitude of 2^-6 or greater.
    /// The caller must prove a common exact plane and positive orientation to infer
    /// a preserved normal direction. This does not certify native normal bits or pixels.
    pub fn certify_native_normal_inputs(
        self,
        positions: &[[f32; 3]],
        profile: NativeDisplacementProfile,
    ) -> io::Result<()> {
        self.certify_native_normal_stencil(positions, None, profile)
    }

    /// Certifies exact, bounded native cell-normal inputs for a heightfield.
    /// Nonzero cross magnitudes must lie in [2^-50, 2^50], with at least one
    /// magnitude of 2^-6 or greater per triangle.
    /// Each triangle must have a nonzero, consistently signed height component,
    /// with every component at most 256 times its magnitude. Tangential components
    /// may cancel or flush to zero; the height projection keeps the averages finite
    /// and nonzero. The caller must prove heightfield geometry and positive projected
    /// orientation. This does not certify native normal bits or pixels.
    pub fn certify_native_height_normal_inputs(
        self,
        positions: &[[f32; 3]],
        axis: usize,
        profile: NativeDisplacementProfile,
    ) -> io::Result<()> {
        if axis >= 3 {
            return Err(invalid("native displacement height axis must be 0, 1 or 2"));
        }
        self.certify_native_normal_stencil(positions, Some(axis), profile)
    }

    /// Returns every conditioned common component sign for each vertex's native
    /// adjacent-cell stencil. Callers must retain a common supported component
    /// across all participants in each neighbor-normal interpolation or average.
    /// This certifies finite nonzero directions, not native normal bits or pixels.
    pub fn certify_native_local_normal_inputs(
        self,
        positions: &[[f32; 3]],
        profile: NativeDisplacementProfile,
    ) -> io::Result<Vec<[i8; 3]>> {
        let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
        let arithmetic = Arithmetic { rounding };
        let size = self.grid_size()?;
        if positions.len() != size * size {
            return Err(invalid(
                "native displacement normal input grid dimensions are invalid",
            ));
        }
        let cells = size - 1;
        let mut cell_support = Vec::with_capacity(cells * cells);
        for row in 0..cells {
            for column in 0..cells {
                let a = positions[row * size + column];
                let b = positions[row * size + column + 1];
                let c = positions[(row + 1) * size + column];
                let d = positions[(row + 1) * size + column + 1];
                let mut support = None;
                for [origin, first, second] in [[a, b, c], [b, d, c]] {
                    let cross = arithmetic.exact_cross(origin, first, second)?;
                    let (mut signs, maximum) = conditioned_normal_signs(cross)?;
                    for axis in 0..3 {
                        let magnitude = f32::from_bits(cross[axis].to_bits() & !SIGN);
                        if signs[axis] != 0
                            && maximum
                                > arithmetic
                                    .multiply(magnitude, MAX_NORMAL_COMPONENT_RATIO)?
                                    .to_bits()
                        {
                            signs[axis] = 0;
                        }
                    }
                    support = Some(
                        support.map_or(signs, |previous| common_normal_support(previous, signs)),
                    );
                }
                cell_support.push(support.unwrap());
            }
        }
        let mut result = Vec::with_capacity(positions.len());
        for row in 0..size {
            for column in 0..size {
                let mut support = None;
                for cell_row in row.saturating_sub(1)..=row.min(cells - 1) {
                    for cell_column in column.saturating_sub(1)..=column.min(cells - 1) {
                        let signs = cell_support[cell_row * cells + cell_column];
                        support = Some(
                            support
                                .map_or(signs, |previous| common_normal_support(previous, signs)),
                        );
                    }
                }
                let support = support.unwrap();
                if support == [0; 3] {
                    return Err(invalid(
                        "native displacement normal stencil has no common conditioned component",
                    ));
                }
                result.push(support);
            }
        }
        Ok(result)
    }

    /// Intersects support for an addition group or two finalized LERP endpoints.
    /// The caller must prove reciprocal disjoint spans and that interpolation
    /// targets never become averaging or interpolation endpoints in this pass.
    pub fn intersect_native_normal_support(
        first: [i8; 3],
        second: [i8; 3],
        displacement_count: usize,
    ) -> io::Result<[i8; 3]> {
        let events = displacement_count
            .checked_mul(NORMAL_RATIO_EVENTS_PER_PATCH)
            .and_then(|events| events.checked_add(NORMAL_STENCIL_RATIO_EVENTS));
        if !(1..=super::MAX_DISPLACEMENTS).contains(&displacement_count)
            || events.is_none_or(|events| events > MAX_NORMAL_RATIO_EVENTS)
        {
            return Err(invalid(
                "native displacement normal averaging exceeds its conditioning bound",
            ));
        }
        if first
            .into_iter()
            .chain(second)
            .any(|sign| !(-1..=1).contains(&sign))
        {
            return Err(invalid("native displacement normal support is invalid"));
        }
        let support = common_normal_support(first, second);
        if support == [0; 3] {
            return Err(invalid(
                "native displacement neighbor normals have no common conditioned component",
            ));
        }
        Ok(support)
    }

    fn certify_native_normal_stencil(
        self,
        positions: &[[f32; 3]],
        height_axis: Option<usize>,
        profile: NativeDisplacementProfile,
    ) -> io::Result<()> {
        let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
        let arithmetic = Arithmetic { rounding };
        let size = self.grid_size()?;
        if positions.len() != size * size {
            return Err(invalid(
                "native displacement normal input grid dimensions are invalid",
            ));
        }
        let mut expected_signs = None;
        for row in 0..size - 1 {
            for column in 0..size - 1 {
                let a = positions[row * size + column];
                let b = positions[row * size + column + 1];
                let c = positions[(row + 1) * size + column];
                let d = positions[(row + 1) * size + column + 1];
                for [origin, first, second] in [[a, b, c], [b, d, c]] {
                    let cross = arithmetic.exact_cross(origin, first, second)?;
                    let (mut signs, maximum) = conditioned_normal_signs(cross)?;
                    if let Some(axis) = height_axis {
                        let height = f32::from_bits(cross[axis].to_bits() & !SIGN);
                        if signs[axis] == 0
                            || maximum
                                > arithmetic
                                    .multiply(height, MAX_NORMAL_COMPONENT_RATIO)?
                                    .to_bits()
                        {
                            return Err(invalid(
                                "native displacement height normal projection is collapsed or poorly conditioned",
                            ));
                        }
                        let sign = signs[axis];
                        signs = [0; 3];
                        signs[axis] = sign;
                    }
                    if expected_signs.is_some_and(|expected| expected != signs) {
                        return Err(invalid(
                            "native displacement normal stencil changes orientation",
                        ));
                    }
                    expected_signs = Some(signs);
                }
            }
        }
        Ok(())
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
        let first = arithmetic.stable_start_corner(info, quad)?;
        let quad = std::array::from_fn(|offset| quad[(first + offset) % 4]);
        let grid = BilinearGrid::new(arithmetic, quad, info.power)?;
        let mut result = Vec::with_capacity(size * size);
        for row in 0..size {
            let interpolated_row = grid.row(arithmetic, row)?;
            for col in 0..size {
                let vertex = vertices[row * size + col];
                let mut point = interpolated_row.point(arithmetic, col)?;
                for (axis, value) in point.iter_mut().enumerate() {
                    let distance = arithmetic.multiply(vertex.direction[axis], vertex.distance)?;
                    let offset = arithmetic.add(distance, 0.0)?;
                    *value = if axis == 0 {
                        arithmetic.add(offset, *value)?
                    } else {
                        arithmetic.add(*value, offset)?
                    };
                }
                result.push(point);
            }
        }
        Ok(result)
    }

    /// Certifies normalized native UV coordinates against one exact affine field.
    /// Mapping dimensions must come from the qualified runtime material provider.
    /// Base and corner UV values must be stable across rounding modes; final zero
    /// signs compare as the same coordinate. Returned values retain the selected
    /// profile's arithmetic signs.
    /// Call for every rounding mode to certify an unknown UV-kernel mode. This is
    /// a coordinate-value contract, not a pixel or radiance guarantee.
    pub fn native_affine_texture_coordinates(
        &self,
        index: usize,
        quad: [[f32; 3]; 4],
        texture_vectors: [[f32; 4]; 2],
        mapping_dimensions: [u32; 2],
        profile: NativeDisplacementProfile,
    ) -> io::Result<Vec<[f32; 2]>> {
        let NativeDisplacementProfile::CssLinux64Build25338318 { rounding } = profile;
        let arithmetic = Arithmetic { rounding };
        let info = *self
            .infos
            .get(index)
            .ok_or_else(|| invalid("displacement index is invalid"))?;
        let size = info.grid_size()?;
        let first = arithmetic.stable_start_corner(info, quad)?;
        let quad = std::array::from_fn(|offset| quad[(first + offset) % 4]);
        let mut reciprocal_mapping = [0.0; 2];
        for (axis, dimension) in mapping_dimensions.into_iter().enumerate() {
            if !dimension.is_power_of_two() || dimension > MAX_MAPPING_DIMENSION {
                return Err(invalid(
                    "native displacement mapping dimensions must be powers of two from 1 through 32768",
                ));
            }
            reciprocal_mapping[axis] = f32::from_bits((127 - dimension.ilog2()) << 23);
        }
        let base = BilinearGrid::new(arithmetic, quad, info.power)?.samples(arithmetic, size)?;
        let mut corner_uv = [[0.0; 2]; 4];
        for (output, point) in corner_uv.iter_mut().zip(quad) {
            *output =
                arithmetic.exact_texture_coordinate(point, texture_vectors, reciprocal_mapping)?;
        }
        for rounding in NativeDisplacementRounding::ALL {
            if rounding == arithmetic.rounding {
                continue;
            }
            let candidate = Arithmetic { rounding };
            let candidate_base =
                BilinearGrid::new(candidate, quad, info.power)?.samples(candidate, size)?;
            if base
                .iter()
                .zip(candidate_base)
                .any(|(&original, candidate)| !same_coordinates(original, candidate))
            {
                return Err(invalid(
                    "native displacement base coordinates depend on rounding mode",
                ));
            }
            for (point, original) in quad.into_iter().zip(corner_uv) {
                let candidate_uv = candidate.exact_texture_coordinate(
                    point,
                    texture_vectors,
                    reciprocal_mapping,
                )?;
                if !same_coordinates(original, candidate_uv) {
                    return Err(invalid(
                        "native displacement corner UV coordinates depend on rounding mode",
                    ));
                }
            }
        }
        let result =
            BilinearGrid::new(arithmetic, corner_uv, info.power)?.samples(arithmetic, size)?;
        for (&base, &uv) in base.iter().zip(&result) {
            let projected =
                arithmetic.exact_texture_coordinate(base, texture_vectors, reciprocal_mapping)?;
            if !same_coordinates(projected, uv) {
                return Err(invalid(
                    "native displacement UV coordinates do not preserve the exact affine field",
                ));
            }
        }
        Ok(result)
    }
}
