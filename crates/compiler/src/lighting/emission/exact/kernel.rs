use super::super::{MAX_CELLS, ReceiverAngularMode, Sample};
use super::{Allocation, Budget, Interval, Point, Rational, Work, dot, hull_cross, turn, vector};

const ATAN_ORDER: usize = 16;
const MAX_REDUCED_ATAN: f64 = 0.2;

pub(super) struct Integral {
    pub(super) interval: Interval,
    pub(super) cells: usize,
}

pub(super) struct Constants {
    half_pi: Interval,
    _allocation: Allocation,
}

struct Arithmetic<'a, 'b> {
    work: &'a mut Work,
    budget: &'a mut Budget<'b>,
}

impl Arithmetic<'_, '_> {
    fn rational(&mut self, value: Rational) -> Result<Interval, String> {
        self.work.charge(64, self.budget)?;
        Interval::enclose(value)
    }

    fn add(&mut self, left: Interval, right: Interval) -> Result<Interval, String> {
        if left.lower == 0.0 && left.upper == 0.0 {
            return Ok(right);
        }
        if right.lower == 0.0 && right.upper == 0.0 {
            return Ok(left);
        }
        self.work.charge_raw(4, self.budget)?;
        left.add(right)
    }

    fn sub(&mut self, left: Interval, right: Interval) -> Result<Interval, String> {
        if right.lower == 0.0 && right.upper == 0.0 {
            return Ok(left);
        }
        self.work.charge_raw(4, self.budget)?;
        left.sub(right)
    }

    fn mul(&mut self, left: Interval, right: Interval) -> Result<Interval, String> {
        if (left.lower == 0.0 && left.upper == 0.0) || (right.lower == 0.0 && right.upper == 0.0) {
            return Ok(Interval::ZERO);
        }
        if left.lower == 1.0 && left.upper == 1.0 {
            return Ok(right);
        }
        if right.lower == 1.0 && right.upper == 1.0 {
            return Ok(left);
        }
        let products = if right.lower >= 0.0 {
            if left.lower >= 0.0 {
                Some((left.lower * right.lower, left.upper * right.upper))
            } else if left.upper <= 0.0 {
                Some((left.lower * right.upper, left.upper * right.lower))
            } else {
                Some((left.lower * right.upper, left.upper * right.upper))
            }
        } else if right.upper <= 0.0 {
            if left.lower >= 0.0 {
                Some((left.upper * right.lower, left.lower * right.upper))
            } else if left.upper <= 0.0 {
                Some((left.upper * right.upper, left.lower * right.lower))
            } else {
                Some((left.upper * right.lower, left.lower * right.lower))
            }
        } else if left.lower >= 0.0 {
            Some((left.upper * right.lower, left.upper * right.upper))
        } else if left.upper <= 0.0 {
            Some((left.lower * right.upper, left.lower * right.lower))
        } else {
            None
        };
        if let Some((lower, upper)) = products {
            self.work.charge_raw(4, self.budget)?;
            if !lower.is_finite() || !upper.is_finite() {
                return Err("Rational emission kernel multiplication is not finite".into());
            }
            Interval::new(lower.next_down(), upper.next_up())
        } else {
            self.work.charge_raw(16, self.budget)?;
            left.mul(right)
        }
    }

    fn div(&mut self, left: Interval, right: Interval) -> Result<Interval, String> {
        if right.lower <= 0.0 {
            return Err("Rational emission kernel requires a certified positive divisor".into());
        }
        if right.lower == 1.0 && right.upper == 1.0 {
            return Ok(left);
        }
        if left.lower == 0.0 && left.upper == 0.0 {
            return Ok(Interval::ZERO);
        }
        if right.lower == right.upper {
            let lower = left.lower / right.lower;
            let upper = left.upper / right.upper;
            self.work.charge_raw(4, self.budget)?;
            if !lower.is_finite() || !upper.is_finite() {
                return Err("Rational emission kernel division is not finite".into());
            }
            return Interval::new(lower.next_down(), upper.next_up());
        }
        self.work.charge_raw(16, self.budget)?;
        let values = [
            left.lower / right.lower,
            left.lower / right.upper,
            left.upper / right.lower,
            left.upper / right.upper,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err("Rational emission kernel division is not finite".into());
        }
        Interval::new(
            values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            values
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        )
    }

    fn scale(&mut self, value: Interval, factor: f64) -> Result<Interval, String> {
        self.mul(value, point(factor))
    }

    fn sqrt(&mut self, value: Interval) -> Result<Interval, String> {
        if value.lower <= 0.0 {
            return Err("Rational emission kernel requires a positive square-root argument".into());
        }
        self.work.charge_raw(2, self.budget)?;
        let lower = self.sqrt_endpoint(value.lower, false)?;
        let upper = self.sqrt_endpoint(value.upper, true)?;
        Interval::new(lower, upper)
    }

    fn sqrt_endpoint(&mut self, value: f64, upper: bool) -> Result<f64, String> {
        self.work.charge(16, self.budget)?;
        let exact = Rational::from_f64(value)?;
        let mut candidate = value.sqrt();
        for _ in 0..8 {
            self.work.charge(64, self.budget)?;
            if !candidate.is_finite() || candidate <= 0.0 {
                return Err(
                    "Rational emission kernel square root is not positive and finite".into(),
                );
            }
            let dyadic = Rational::from_f64(candidate)?;
            let comparison = dyadic.mul(dyadic)?.compare(exact);
            if (upper && comparison != std::cmp::Ordering::Less)
                || (!upper && comparison != std::cmp::Ordering::Greater)
            {
                return Ok(candidate);
            }
            candidate = if upper {
                candidate.next_up()
            } else {
                candidate.next_down()
            };
        }
        Err("Rational emission kernel cannot certify its square-root enclosure".into())
    }

    fn width(&mut self, value: Interval) -> Result<f64, String> {
        self.work.charge_raw(2, self.budget)?;
        let width = (value.upper - value.lower).next_up();
        if !width.is_finite() || width < 0.0 {
            return Err("Rational emission kernel enclosure width is not finite".into());
        }
        Ok(width)
    }
}

fn point(value: f64) -> Interval {
    Interval {
        lower: value,
        upper: value,
    }
}

fn norm(value: [Rational; 3], arithmetic: &mut Arithmetic<'_, '_>) -> Result<Interval, String> {
    let mut squared = Interval::ZERO;
    for component in value {
        let component = arithmetic.rational(component)?;
        let mut square = arithmetic.mul(component, component)?;
        square.lower = square.lower.max(0.0);
        squared = arithmetic.add(squared, square)?;
    }
    arithmetic.sqrt(squared)
}

fn small_atan(
    denominator: i128,
    arithmetic: &mut Arithmetic<'_, '_>,
) -> Result<(Rational, Rational), String> {
    arithmetic.work.charge(64, arithmetic.budget)?;
    let value = Rational::new(1, denominator)?;
    let squared = value.mul(value)?;
    let mut power = value;
    let mut sum = Rational::ZERO;
    for order in 0..=ATAN_ORDER {
        arithmetic.work.charge(128, arithmetic.budget)?;
        let term = power.div(Rational::new((2 * order + 1) as i128, 1)?)?;
        sum = if order % 2 == 0 {
            sum.add(term)?
        } else {
            sum.sub(term)?
        };
        power = power.mul(squared)?;
    }
    arithmetic.work.charge(64, arithmetic.budget)?;
    let remainder = power.div(Rational::new((2 * ATAN_ORDER + 3) as i128, 1)?)?;
    Ok((sum.sub(remainder)?, sum))
}

fn pi(arithmetic: &mut Arithmetic<'_, '_>) -> Result<Interval, String> {
    let (left_lower, left_upper) = small_atan(5, arithmetic)?;
    let (right_lower, right_upper) = small_atan(239, arithmetic)?;
    arithmetic.work.charge(256, arithmetic.budget)?;
    let sixteen = Rational::new(16, 1)?;
    let four = Rational::new(4, 1)?;
    let lower = left_lower.mul(sixteen)?.sub(right_upper.mul(four)?)?;
    let upper = left_upper.mul(sixteen)?.sub(right_lower.mul(four)?)?;
    let lower = arithmetic.rational(lower)?.lower;
    let upper = arithmetic.rational(upper)?.upper;
    Interval::new(lower, upper)
}

fn half_pi(arithmetic: &mut Arithmetic<'_, '_>) -> Result<Interval, String> {
    arithmetic.work.charge_raw(2, arithmetic.budget)?;
    if let Some(constants) = &arithmetic.work.kernel_constants {
        return Ok(constants.half_pi);
    }
    let allocation = arithmetic.budget.reserve::<Constants>(1)?;
    let pi = pi(arithmetic)?;
    let half_pi = arithmetic.scale(pi, 0.5)?;
    arithmetic.work.charge_raw(2, arithmetic.budget)?;
    arithmetic.work.kernel_constants = Some(Constants {
        half_pi,
        _allocation: allocation,
    });
    Ok(half_pi)
}

fn reduced_atan(
    mut value: Interval,
    arithmetic: &mut Arithmetic<'_, '_>,
) -> Result<Interval, String> {
    for _ in 0..2 {
        let square = arithmetic.mul(value, value)?;
        let argument = arithmetic.add(point(1.0), square)?;
        let root = arithmetic.sqrt(argument)?;
        let divisor = arithmetic.add(point(1.0), root)?;
        value = arithmetic.div(value, divisor)?;
    }
    if value.lower < 0.0 || value.upper > MAX_REDUCED_ATAN {
        return Err("Rational emission atan cannot certify its half-angle reduction".into());
    }
    let square = arithmetic.mul(value, value)?;
    let mut polynomial = arithmetic.div(point(1.0), point((2 * ATAN_ORDER + 1) as f64))?;
    for order in (0..ATAN_ORDER).rev() {
        let coefficient = arithmetic.div(point(1.0), point((2 * order + 1) as f64))?;
        let product = arithmetic.mul(square, polynomial)?;
        polynomial = arithmetic.sub(coefficient, product)?;
    }
    let estimate = arithmetic.mul(value, polynomial)?;
    let upper = point(value.upper);
    let upper_squared = arithmetic.mul(upper, upper)?;
    let mut power = upper;
    for _ in 0..=ATAN_ORDER {
        power = arithmetic.mul(power, upper_squared)?;
    }
    let remainder = arithmetic
        .div(power, point((2 * ATAN_ORDER + 3) as f64))?
        .upper;
    let enclosure = arithmetic.add(estimate, Interval::new(-remainder, remainder)?)?;
    arithmetic.scale(enclosure, 4.0)
}

fn atan_point(
    value: f64,
    half_pi: Interval,
    arithmetic: &mut Arithmetic<'_, '_>,
) -> Result<Interval, String> {
    if !value.is_finite() {
        return Err("Rational emission atan argument is not finite".into());
    }
    if value == 0.0 {
        return Ok(Interval::ZERO);
    }
    let magnitude = value.abs();
    let angle = if magnitude > 1.0 {
        let reciprocal = arithmetic.div(point(1.0), point(magnitude))?;
        let angle = reduced_atan(reciprocal, arithmetic)?;
        arithmetic.sub(half_pi, angle)?
    } else {
        reduced_atan(point(magnitude), arithmetic)?
    };
    Ok(if value < 0.0 { angle.neg() } else { angle })
}

fn atan2(
    y: Interval,
    x: Interval,
    half_pi: Interval,
    arithmetic: &mut Arithmetic<'_, '_>,
) -> Result<Interval, String> {
    let ratio = arithmetic.div(x, y)?;
    let lower = atan_point(ratio.lower, half_pi, arithmetic)?;
    let upper = atan_point(ratio.upper, half_pi, arithmetic)?;
    let angle = Interval::new(lower.lower, upper.upper)?;
    arithmetic.sub(half_pi, angle)
}

struct Frame {
    rays: [[Rational; 3]; 3],
    normal: [Rational; 3],
    height: Rational,
    jacobian: Rational,
    angular: ReceiverAngularMode,
}

impl Frame {
    fn new(
        source: [[Rational; 3]; 3],
        domain: [Point; 3],
        sample: &Sample,
        arithmetic: &mut Arithmetic<'_, '_>,
    ) -> Result<Self, String> {
        arithmetic.work.charge(1024, arithmetic.budget)?;
        let sample_point = vector(sample.point)?;
        let normal = vector(sample.normal)?;
        let mut edges = [[Rational::ZERO; 3]; 2];
        for (edge, endpoint) in edges.iter_mut().zip(&source[1..]) {
            for (axis, value) in edge.iter_mut().enumerate() {
                *value = endpoint[axis].sub(source[0][axis])?;
            }
        }
        let offset = [
            sample_point[0].sub(source[0][0])?,
            sample_point[1].sub(source[0][1])?,
            sample_point[2].sub(source[0][2])?,
        ];
        let height = dot(hull_cross(edges[0], edges[1])?, offset)?;
        let jacobian = turn(domain[0], domain[1], domain[2])?;
        if height.sign() != std::cmp::Ordering::Greater
            || jacobian.sign() != std::cmp::Ordering::Greater
        {
            return Err(
                "Rational emission boundary kernel lost positive source/chart facing".into(),
            );
        }
        let mut rays = [[Rational::ZERO; 3]; 3];
        for (ray, coordinate) in rays.iter_mut().zip(domain) {
            arithmetic.work.charge(384, arithmetic.budget)?;
            for (axis, value) in ray.iter_mut().enumerate() {
                *value = source[0][axis]
                    .add(edges[0][axis].mul(coordinate.0[0])?)?
                    .add(edges[1][axis].mul(coordinate.0[1])?)?
                    .sub(sample_point[axis])?;
            }
            if matches!(sample.angular, ReceiverAngularMode::Normal)
                && dot(normal, *ray)?.sign() == std::cmp::Ordering::Less
            {
                return Err(
                    "Rational emission boundary kernel crosses the receiver horizon".into(),
                );
            }
        }
        Ok(Self {
            rays,
            normal,
            height,
            jacobian,
            angular: sample.angular,
        })
    }

    fn normal_integral(
        &self,
        half_pi: Interval,
        arithmetic: &mut Arithmetic<'_, '_>,
        edges: &mut usize,
    ) -> Result<Interval, String> {
        let mut sum = Interval::ZERO;
        for index in 0..3 {
            *edges = index + 1;
            arithmetic.work.charge(384, arithmetic.budget)?;
            let a = self.rays[index];
            let b = self.rays[(index + 1) % 3];
            let cross = hull_cross(a, b)?;
            if cross.iter().all(|value| *value == Rational::ZERO) {
                return Err(
                    "Rational emission boundary kernel has a collapsed angular edge".into(),
                );
            }
            let length = norm(cross, arithmetic)?;
            let cosine = arithmetic.rational(dot(a, b)?)?;
            let angle = atan2(length, cosine, half_pi, arithmetic)?;
            let projection = arithmetic.rational(dot(self.normal, cross)?)?;
            let projection = arithmetic.div(projection, length)?;
            let contribution = arithmetic.mul(projection, angle)?;
            sum = arithmetic.add(sum, contribution)?;
        }
        arithmetic.scale(sum, -0.5)
    }

    fn solid_angle_integral(
        &self,
        half_pi: Interval,
        arithmetic: &mut Arithmetic<'_, '_>,
    ) -> Result<Interval, String> {
        let mut lengths = [Interval::ZERO; 3];
        for (length, ray) in lengths.iter_mut().zip(self.rays) {
            *length = norm(ray, arithmetic)?;
        }
        arithmetic.work.charge(256, arithmetic.budget)?;
        let height = arithmetic.rational(self.height)?;
        let jacobian = arithmetic.rational(self.jacobian)?;
        let determinant = arithmetic.mul(height, jacobian)?;
        let first = arithmetic.mul(lengths[0], lengths[1])?;
        let mut denominator = arithmetic.mul(first, lengths[2])?;
        for index in 0..3 {
            arithmetic.work.charge(128, arithmetic.budget)?;
            let product =
                arithmetic.rational(dot(self.rays[index], self.rays[(index + 1) % 3])?)?;
            let term = arithmetic.mul(product, lengths[(index + 2) % 3])?;
            denominator = arithmetic.add(denominator, term)?;
        }
        let angle = atan2(determinant, denominator, half_pi, arithmetic)?;
        let solid_angle = arithmetic.scale(angle, 2.0)?;
        arithmetic.scale(solid_angle, super::super::super::IGNORE_NORMALS_FACTOR)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn integrate_triangle(
    source: [[Rational; 3]; 3],
    domain: [Point; 3],
    sample: &Sample,
    error_limit: f64,
    remaining_cells: usize,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Integral, String> {
    let numeric_start = work.numeric_work;
    let mut edges = 0;
    let result = (|| {
        if !error_limit.is_finite() || error_limit <= 0.0 {
            return Err(
                "Rational emission boundary kernel requires a finite positive error allowance"
                    .into(),
            );
        }
        if remaining_cells.min(MAX_CELLS) == 0 {
            return Err("Rational emission boundary kernel has no remaining receiver cells".into());
        }
        let mut arithmetic = Arithmetic {
            work: &mut *work,
            budget: &mut *budget,
        };
        let frame = Frame::new(source, domain, sample, &mut arithmetic)?;
        let half_pi = half_pi(&mut arithmetic)?;
        let integral = match frame.angular {
            ReceiverAngularMode::Normal => {
                frame.normal_integral(half_pi, &mut arithmetic, &mut edges)?
            }
            ReceiverAngularMode::IgnoreNormals => {
                frame.solid_angle_integral(half_pi, &mut arithmetic)?
            }
        };
        if integral.upper < 0.0 {
            return Err("Rational emission boundary kernel lost its nonnegative integral".into());
        }
        let interval = Interval::new(integral.lower.max(0.0), integral.upper)?;
        if arithmetic.width(interval)? > error_limit {
            return Err(format!(
                "Rational emission boundary kernel enclosure exceeds its error allowance: [{}, {}], limit {error_limit}",
                interval.lower, interval.upper,
            ));
        }
        Ok(Integral { interval, cells: 1 })
    })();
    result.map_err(|error| {
        format!(
            "{error}; rational boundary kernel edges:{edges} raw_work:{} source_raw_work:{} source_work:{} lighting_work:{}",
            work.numeric_work.saturating_sub(numeric_start),
            work.numeric_work,
            work.used,
            budget.work(),
        )
    })
}
