use std::cmp::Ordering;

use bnum::{cast::As, n, types::I1024};
use glam::{DVec2, DVec3};
use map_core::Plane;

use super::super::bake::{Allocation, Budget};

pub(super) const MAX_HULL_PLANES: usize = 128;
const TRIANGLE_PLANES: usize = 5;
const MAX_CONSTRAINTS: usize = MAX_HULL_PLANES * MAX_HULL_PLANES / 4 + MAX_HULL_PLANES;
const MAX_FRAGMENTS: usize = 4096;
const MAX_VERTICES: usize = 65_536;
const MAX_POLYGON_VERTICES: usize = MAX_CONSTRAINTS + 3;
const MAX_WORK: usize = 32_000_000;
const ARITHMETIC_WORK_SCALE: usize = size_of::<I1024>() / size_of::<i128>();
const MAX_DYADIC_POWER: i32 = I1024::BITS as i32 - 2;
const MAX_CONVERSION_ERROR: f64 = 1e-12;
const MIN_TRIANGLE_CONDITION: f64 = 1.0 / (1u64 << 32) as f64;
const MIN_ANGULAR_SPAN_SQUARED: f64 = 1.0 / (1u64 << 48) as f64;
const ARITHMETIC_ERROR: &str =
    "Exact emission exceeds checked 1023-bit rational arithmetic; sampled fallback is disabled";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rational {
    numerator: I1024,
    denominator: I1024,
}

fn gcd(mut a: I1024, mut b: I1024) -> I1024 {
    while !b.is_zero() {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

impl Rational {
    const ZERO: Self = Self {
        numerator: n!(0),
        denominator: n!(1),
    };
    const ONE: Self = Self {
        numerator: n!(1),
        denominator: n!(1),
    };

    fn new(numerator: i128, denominator: i128) -> Result<Self, String> {
        let numerator = numerator.as_::<I1024>();
        let denominator = denominator.as_::<I1024>();
        Self::ratio(numerator, denominator)
    }

    fn ratio(mut numerator: I1024, mut denominator: I1024) -> Result<Self, String> {
        if denominator.is_zero() || numerator == I1024::MIN || denominator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        if denominator.is_negative() {
            numerator = -numerator;
            denominator = -denominator;
        }
        let divisor = gcd(numerator.abs(), denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn from_f64(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("Exact emission requires finite prepared coordinates".into());
        }
        if value == 0.0 {
            return Ok(Self::ZERO);
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let mut mantissa = bits & ((1u64 << 52) - 1);
        let mut power = if exponent == 0 {
            -1074
        } else {
            exponent - 1075
        };
        if exponent != 0 {
            mantissa |= 1u64 << 52;
        }
        let trailing = mantissa.trailing_zeros();
        mantissa >>= trailing;
        power += trailing as i32;
        let mut numerator = mantissa.as_::<I1024>();
        let denominator = if power >= 0 {
            if power > MAX_DYADIC_POWER || numerator > (I1024::MAX >> power) {
                return Err(ARITHMETIC_ERROR.into());
            }
            numerator <<= power;
            n!(1)
        } else {
            if power < -MAX_DYADIC_POWER {
                return Err(ARITHMETIC_ERROR.into());
            }
            n!(1I1024) << -power
        };
        if bits >> 63 != 0 {
            numerator = -numerator;
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    fn sign(self) -> Ordering {
        Ord::cmp(&self.numerator, &n!(0))
    }

    fn neg(self) -> Self {
        Self {
            numerator: -self.numerator,
            denominator: self.denominator,
        }
    }

    fn abs(self) -> Self {
        Self {
            numerator: self.numerator.abs(),
            denominator: self.denominator,
        }
    }

    fn add(self, other: Self) -> Result<Self, String> {
        let divisor = gcd(self.denominator, other.denominator);
        let left = self
            .numerator
            .checked_mul(other.denominator / divisor)
            .ok_or(ARITHMETIC_ERROR)?;
        let right = other
            .numerator
            .checked_mul(self.denominator / divisor)
            .ok_or(ARITHMETIC_ERROR)?;
        let numerator = left.checked_add(right).ok_or(ARITHMETIC_ERROR)?;
        if numerator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        let reduction = gcd(numerator.abs(), divisor);
        let denominator = (self.denominator / divisor)
            .checked_mul(other.denominator / reduction)
            .ok_or(ARITHMETIC_ERROR)?;
        Ok(Self {
            numerator: numerator / reduction,
            denominator,
        })
    }

    fn sub(self, other: Self) -> Result<Self, String> {
        self.add(other.neg())
    }

    fn mul(self, other: Self) -> Result<Self, String> {
        let left = gcd(self.numerator.abs(), other.denominator);
        let right = gcd(other.numerator.abs(), self.denominator);
        let numerator = (self.numerator / left)
            .checked_mul(other.numerator / right)
            .ok_or(ARITHMETIC_ERROR)?;
        let denominator = (self.denominator / right)
            .checked_mul(other.denominator / left)
            .ok_or(ARITHMETIC_ERROR)?;
        if numerator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    fn div(self, other: Self) -> Result<Self, String> {
        if other.numerator.is_zero() {
            return Err("Exact emission encountered a zero rational denominator".into());
        }
        let inverse = Self {
            numerator: if other.numerator.is_negative() {
                -other.denominator
            } else {
                other.denominator
            },
            denominator: other.numerator.abs(),
        };
        self.mul(inverse)
    }

    fn compare(self, other: Self) -> Ordering {
        let sign = self.sign().cmp(&other.sign());
        if sign != Ordering::Equal {
            return sign;
        }
        let mut a = self.numerator.abs();
        let mut b = self.denominator;
        let mut c = other.numerator.abs();
        let mut d = other.denominator;
        let mut reverse = self.numerator.is_negative();
        loop {
            let order = Ord::cmp(&(a / b), &(c / d));
            if order != Ordering::Equal {
                return if reverse { order.reverse() } else { order };
            }
            let left = a % b;
            let right = c % d;
            if left.is_zero() || right.is_zero() {
                let order = Ord::cmp(&left, &right);
                return if reverse { order.reverse() } else { order };
            }
            (a, b, c, d) = (b, left, d, right);
            reverse = !reverse;
        }
    }

    fn approximate(self) -> f64 {
        self.numerator.as_::<f64>() / self.denominator.as_::<f64>()
    }

    fn round_dyadic(self, power: u32) -> Result<Self, String> {
        if self.sign() == Ordering::Less || power > f64::MANTISSA_DIGITS {
            return Err(
                "Exact emission dyadic conversion requires bounded nonnegative weights".into(),
            );
        }
        let denominator = n!(1I1024) << power;
        let scaled = self.mul(Self::ratio(denominator, n!(1))?)?;
        let mut integer = scaled.numerator / scaled.denominator;
        let remainder = scaled.numerator % scaled.denominator;
        if remainder >= scaled.denominator - remainder {
            integer = integer.checked_add(n!(1)).ok_or(ARITHMETIC_ERROR)?;
        }
        Self::ratio(integer, denominator)
    }

    fn round_signed_dyadic(self, power: u32) -> Result<Self, String> {
        let rounded = self.abs().round_dyadic(power)?;
        Ok(if self.sign() == Ordering::Less {
            rounded.neg()
        } else {
            rounded
        })
    }

    fn upper(self) -> Result<f64, String> {
        let mut value = self.approximate();
        for _ in 0..8 {
            if Self::from_f64(value)?.compare(self) != Ordering::Less {
                return Ok(value);
            }
            value = value.next_up();
        }
        Err("Exact emission cannot bound binary64 conversion".into())
    }

    fn lower(self) -> Result<f64, String> {
        let mut value = self.approximate();
        for _ in 0..8 {
            if Self::from_f64(value)?.compare(self) != Ordering::Greater {
                return Ok(value);
            }
            value = value.next_down();
        }
        Err("Exact emission cannot bound binary64 conversion".into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Point([Rational; 2]);

#[derive(Clone, Copy)]
struct Interval {
    lower: f64,
    upper: f64,
}

impl Interval {
    const ZERO: Self = Self {
        lower: 0.0,
        upper: 0.0,
    };

    fn new(lower: f64, upper: f64) -> Result<Self, String> {
        if !lower.is_finite() || !upper.is_finite() || lower > upper {
            return Err("Exact emission cannot certify a finite conversion interval".into());
        }
        Ok(Self { lower, upper })
    }

    fn enclose(value: Rational) -> Result<Self, String> {
        Self::new(value.lower()?, value.upper()?)
    }

    fn add(self, other: Self) -> Result<Self, String> {
        Self::new(
            (self.lower + other.lower).next_down(),
            (self.upper + other.upper).next_up(),
        )
    }

    fn sub(self, other: Self) -> Result<Self, String> {
        Self::new(
            (self.lower - other.upper).next_down(),
            (self.upper - other.lower).next_up(),
        )
    }

    fn mul(self, other: Self) -> Result<Self, String> {
        let products = [
            self.lower * other.lower,
            self.lower * other.upper,
            self.upper * other.lower,
            self.upper * other.upper,
        ];
        if products.iter().any(|value| !value.is_finite()) {
            return Err("Exact emission cannot certify a finite conversion interval".into());
        }
        Self::new(
            products
                .into_iter()
                .fold(f64::INFINITY, f64::min)
                .next_down(),
            products
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        )
    }

    fn neg(self) -> Self {
        Self {
            lower: -self.upper,
            upper: -self.lower,
        }
    }
}

fn point_difference(a: Point, b: Point) -> Result<Point, String> {
    Ok(Point([a.0[0].sub(b.0[0])?, a.0[1].sub(b.0[1])?]))
}

fn point_interval(point: Point) -> Result<[Interval; 2], String> {
    Ok([
        Interval::enclose(point.0[0])?,
        Interval::enclose(point.0[1])?,
    ])
}

fn interval_cross(a: [Interval; 2], b: [Interval; 2]) -> Result<Interval, String> {
    a[0].mul(b[1])?.sub(a[1].mul(b[0])?)
}

fn conversion_error(
    original: &[Point],
    candidate: &[Point],
    orientation: Ordering,
    distance_squared_lower: Rational,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<f64, String> {
    if original.len() != candidate.len() || original.len() < 3 {
        return Err("Exact emission conversion lost matching polygon vertices".into());
    }
    work.charge(original.len() * 4, budget)?;
    if original == candidate {
        return Ok(0.0);
    }
    let mut loss = Interval::ZERO;
    let zero = Point([Rational::ZERO; 2]);
    for index in 0..original.len() {
        work.charge(192, budget)?;
        let next = (index + 1) % original.len();
        let delta = point_difference(original[index], candidate[index])?;
        let next_delta = point_difference(original[next], candidate[next])?;
        if delta == zero && next_delta == zero {
            continue;
        }
        loss = loss.add(
            interval_cross(point_interval(delta)?, point_interval(original[next])?)?.add(
                interval_cross(
                    point_interval(candidate[index])?,
                    point_interval(next_delta)?,
                )?,
            )?,
        )?;
    }
    let loss = match orientation {
        Ordering::Less => loss.neg(),
        Ordering::Greater => loss,
        Ordering::Equal => {
            return Err("Exact emission conversion lost its source orientation".into());
        }
    };
    if loss.upper < 0.0 {
        return Err("Exact emission conversion enlarged a visible fragment".into());
    }
    work.charge(64, budget)?;
    let distance_lower = distance_squared_lower.lower()?;
    if !distance_lower.is_finite() || distance_lower <= 0.0 {
        return Err("Exact emission cannot certify a positive kernel distance".into());
    }
    let bound = (loss.upper / distance_lower).next_up();
    if !bound.is_finite() || bound < 0.0 {
        return Err("Exact emission cannot certify a finite conversion bound".into());
    }
    Ok(bound)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Halfplane([I1024; 3]);

impl Halfplane {
    fn new(
        values: [Rational; 3],
        work: &mut Work,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        work.charge(64, budget)?;
        let mut denominator = n!(1I1024);
        for value in values {
            denominator = (denominator / gcd(denominator, value.denominator))
                .checked_mul(value.denominator)
                .ok_or(ARITHMETIC_ERROR)?;
        }
        let mut coefficients = [n!(0I1024); 3];
        let mut divisor = n!(0I1024);
        for (coefficient, value) in coefficients.iter_mut().zip(values) {
            *coefficient = value
                .numerator
                .checked_mul(denominator / value.denominator)
                .filter(|value| *value != I1024::MIN)
                .ok_or(ARITHMETIC_ERROR)?;
            divisor = gcd(divisor, coefficient.abs());
        }
        if !divisor.is_zero() {
            coefficients = coefficients.map(|value| value / divisor);
        }
        Ok(Self(coefficients))
    }

    fn evaluate(self, point: Point) -> Result<Rational, String> {
        Rational::ratio(self.0[0], n!(1))?
            .mul(point.0[0])?
            .add(Rational::ratio(self.0[1], n!(1))?.mul(point.0[1])?)?
            .add(Rational::ratio(self.0[2], n!(1))?)
    }

    fn intersection(
        self,
        other: Self,
        work: &mut Work,
        budget: &mut Budget<'_>,
    ) -> Result<Point, String> {
        work.charge(192, budget)?;
        let determinant = integer_minor(self.0[0], other.0[1], self.0[1], other.0[0])?;
        if determinant.is_zero() {
            return Err("Exact emission crossing edges have parallel boundary equations".into());
        }
        let point = Point([
            Rational::ratio(
                integer_minor(self.0[1], other.0[2], self.0[2], other.0[1])?,
                determinant,
            )?,
            Rational::ratio(
                integer_minor(self.0[2], other.0[0], self.0[0], other.0[2])?,
                determinant,
            )?,
        ]);
        if self.evaluate(point)? != Rational::ZERO || other.evaluate(point)? != Rational::ZERO {
            return Err("Exact emission intersection failed its exact boundary equations".into());
        }
        Ok(point)
    }

    fn neg(self) -> Self {
        Self(self.0.map(|value| -value))
    }

    fn constant(self) -> bool {
        self.0[0].is_zero() && self.0[1].is_zero()
    }
}

fn integer_minor(a: I1024, b: I1024, c: I1024, d: I1024) -> Result<I1024, String> {
    a.checked_mul(b)
        .and_then(|left| c.checked_mul(d).and_then(|right| left.checked_sub(right)))
        .filter(|value| *value != I1024::MIN)
        .ok_or_else(|| ARITHMETIC_ERROR.into())
}

#[derive(Clone, Copy)]
struct TopoVertex {
    point: Point,
    incoming: Halfplane,
}

struct Storage<T> {
    values: Vec<T>,
    _allocation: Allocation,
}

impl<T> Storage<T> {
    fn new(capacity: usize, budget: &mut Budget<'_>) -> Result<Self, String> {
        budget.consume(1)?;
        let allocation = budget.reserve::<T>(capacity)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(capacity)
            .map_err(|_| "Exact emission allocation failed")?;
        Ok(Self {
            values,
            _allocation: allocation,
        })
    }

    fn push(&mut self, value: T) -> Result<(), String> {
        if self.values.len() == self.values.capacity() {
            return Err("Exact emission exceeded its charged storage capacity".into());
        }
        self.values.push(value);
        Ok(())
    }
}

type Polygon = Storage<Point>;
type TopoPolygon = Storage<TopoVertex>;

impl TopoPolygon {
    fn push_vertex(&mut self, vertex: TopoVertex) -> Result<(), String> {
        if self
            .values
            .last()
            .is_none_or(|last| last.point != vertex.point)
        {
            self.push(vertex)?;
        }
        Ok(())
    }

    fn remove_collinear(&mut self, work: &mut Work, budget: &mut Budget<'_>) -> Result<(), String> {
        let count = self.values.len();
        let first = self.values[0].incoming;
        let mut retained = 0;
        for index in 0..count {
            work.charge(8, budget)?;
            let next = self
                .values
                .get(index + 1)
                .map_or(first, |vertex| vertex.incoming);
            if self.values[index].incoming != next {
                self.values[retained] = self.values[index];
                retained += 1;
            }
        }
        if retained < 3 {
            return Err("Exact emission collinear cleanup lost a positive fragment".into());
        }
        self.values.truncate(retained);
        Ok(())
    }

    fn merged(
        &self,
        other: &Self,
        work: &mut Work,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Self>, String> {
        for (left, a) in self.values.iter().enumerate() {
            let previous_left = self.values[(left + self.values.len() - 1) % self.values.len()];
            let next_left = self.values[(left + 1) % self.values.len()];
            for (right, b) in other.values.iter().enumerate() {
                work.charge(1, budget)?;
                if a.incoming != b.incoming.neg() {
                    continue;
                }
                work.charge(4, budget)?;
                let previous_right =
                    other.values[(right + other.values.len() - 1) % other.values.len()];
                let next_right = other.values[(right + 1) % other.values.len()];
                if a.point != previous_right.point || b.point != previous_left.point {
                    continue;
                }
                // Convex inputs meet across a complete edge; only the new joins can turn outward.
                work.charge(96, budget)?;
                if previous_right.incoming.evaluate(next_left.point)?.sign() == Ordering::Greater
                    || previous_left.incoming.evaluate(next_right.point)?.sign()
                        == Ordering::Greater
                {
                    return Ok(None);
                }
                let capacity = self.values.len() + other.values.len() - 2;
                if capacity > MAX_POLYGON_VERTICES {
                    return Ok(None);
                }
                let mut merged = Self::new(capacity, budget)?;
                work.charge(capacity * 8, budget)?;
                merged.push(TopoVertex {
                    point: a.point,
                    incoming: previous_right.incoming,
                })?;
                for offset in 1..self.values.len() {
                    merged.push(self.values[(left + offset) % self.values.len()])?;
                }
                for offset in 1..other.values.len() - 1 {
                    merged.push(other.values[(right + offset) % other.values.len()])?;
                }
                merged.remove_collinear(work, budget)?;
                return Ok(Some(merged));
            }
        }
        Ok(None)
    }
}

struct Polygons {
    polygons: Storage<TopoPolygon>,
    vertices: usize,
}

impl Polygons {
    fn new(budget: &mut Budget<'_>) -> Result<Self, String> {
        Ok(Self {
            polygons: Storage::new(MAX_FRAGMENTS, budget)?,
            vertices: 0,
        })
    }

    fn push(&mut self, polygon: TopoPolygon) -> Result<(), String> {
        if self.polygons.values.len() >= MAX_FRAGMENTS {
            return Err("Exact emission exceeds 4096 visible fragments".into());
        }
        let vertices = self
            .vertices
            .checked_add(polygon.values.len())
            .filter(|vertices| *vertices <= MAX_VERTICES)
            .ok_or("Exact emission exceeds 65536 visible vertices")?;
        self.polygons.push(polygon)?;
        self.vertices = vertices;
        Ok(())
    }

    fn coalesce(&mut self, work: &mut Work, budget: &mut Budget<'_>) -> Result<(), String> {
        for polygon in &mut self.polygons.values {
            let previous = polygon.values.len();
            polygon.remove_collinear(work, budget)?;
            self.vertices -= previous - polygon.values.len();
        }
        loop {
            let mut changed = false;
            'pairs: for left in 0..self.polygons.values.len() {
                for right in left + 1..self.polygons.values.len() {
                    work.charge(1, budget)?;
                    if let Some(merged) = self.polygons.values[left].merged(
                        &self.polygons.values[right],
                        work,
                        budget,
                    )? {
                        self.vertices -= self.polygons.values[left].values.len()
                            + self.polygons.values[right].values.len()
                            - merged.values.len();
                        self.polygons.values[left] = merged;
                        work.charge((self.polygons.values.len() - right) * 4, budget)?;
                        self.polygons.values.remove(right);
                        changed = true;
                        break 'pairs;
                    }
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }
}

struct Work {
    used: usize,
    operation: &'static str,
}

impl Work {
    fn new(operation: &'static str) -> Self {
        Self { used: 0, operation }
    }

    fn charge(&mut self, amount: usize, budget: &mut Budget<'_>) -> Result<(), String> {
        let amount = amount
            .checked_mul(ARITHMETIC_WORK_SCALE)
            .ok_or("Exact emission work charge overflow")?;
        budget.consume(amount)?;
        self.used = self
            .used
            .checked_add(amount)
            .filter(|work| *work <= MAX_WORK)
            .ok_or_else(|| {
                format!(
                    "Exact emission exceeds {MAX_WORK} work units for one {}",
                    self.operation
                )
            })?;
        Ok(())
    }
}

fn vector(value: DVec3) -> Result<[Rational; 3], String> {
    Ok([
        Rational::from_f64(value.x)?,
        Rational::from_f64(value.y)?,
        Rational::from_f64(value.z)?,
    ])
}

fn dot(a: [Rational; 3], b: [Rational; 3]) -> Result<Rational, String> {
    a[0].mul(b[0])?.add(a[1].mul(b[1])?)?.add(a[2].mul(b[2])?)
}

fn triangle_normal(points: [[Rational; 3]; 3]) -> Result<[Rational; 3], String> {
    let first = [
        points[1][0].sub(points[0][0])?,
        points[1][1].sub(points[0][1])?,
        points[1][2].sub(points[0][2])?,
    ];
    let second = [
        points[2][0].sub(points[0][0])?,
        points[2][1].sub(points[0][1])?,
        points[2][2].sub(points[0][2])?,
    ];
    hull_cross(first, second)
}

fn normalized_normal(actual: [Rational; 3]) -> Result<([Rational; 3], usize), String> {
    let mut axis = 0;
    for (candidate, component) in actual.into_iter().enumerate().skip(1) {
        if component.abs().compare(actual[axis].abs()) == Ordering::Greater {
            axis = candidate;
        }
    }
    if actual[axis] == Rational::ZERO {
        return Err("Exact emission requires a positive-area source triangle".into());
    }
    let mut normal = [Rational::ZERO; 3];
    for (component, value) in normal.iter_mut().zip(actual) {
        *component = value.div(actual[axis])?;
    }
    Ok((normal, axis))
}

pub(super) fn source_support(
    points: [DVec3; 3],
    plane: Plane,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    budget.consume(192 * ARITHMETIC_WORK_SCALE)?;
    let normal = vector(plane.normal)?;
    let distance = Rational::from_f64(plane.dist)?;
    for point in points {
        if dot(normal, vector(point)?)? != distance {
            return Err(
                "Exact emission requires zero support residual on the actual source vertices"
                    .into(),
            );
        }
    }
    Ok(())
}

pub(super) fn triangle_front(
    points: [DVec3; 3],
    point: DVec3,
    budget: &mut Budget<'_>,
) -> Result<Ordering, String> {
    budget.consume(256 * ARITHMETIC_WORK_SCALE)?;
    let source = [vector(points[0])?, vector(points[1])?, vector(points[2])?];
    let normal = triangle_normal(source)?;
    if normal.iter().all(|value| *value == Rational::ZERO) {
        return Err("Exact emission source facing requires a positive actual triangle".into());
    }
    let point = vector(point)?;
    let delta = [
        point[0].sub(source[0][0])?,
        point[1].sub(source[0][1])?,
        point[2].sub(source[0][2])?,
    ];
    Ok(dot(normal, delta)?.sign())
}

pub(super) struct SourceFace {
    normal: [Rational; 3],
    distance: Rational,
    axis: usize,
    orientation: Ordering,
    planar: bool,
    retained_support: bool,
    fan: Storage<bool>,
    _allocation: Allocation,
}

impl SourceFace {
    pub(super) fn new(
        points: &[DVec3],
        retained_plane: Plane,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        if points.len() < 3 {
            return Err(
                "Exact emission requires at least three actual native source corners".into(),
            );
        }
        let allocation = budget.reserve_bytes(size_of::<Self>() + 2 * size_of::<usize>())?;
        let mut work = Work::new("source face admission");
        let mut source = Storage::new(points.len(), budget)?;
        for &point in points {
            work.charge(32, budget)?;
            source.push(vector(point)?)?;
        }
        let mut actual = [Rational::ZERO; 3];
        for pair in source.values[1..].windows(2) {
            work.charge(128, budget)?;
            actual = triangle_normal([source.values[0], pair[1], pair[0]])?;
            if actual.iter().any(|value| *value != Rational::ZERO) {
                break;
            }
        }
        work.charge(128, budget)?;
        let (normal, axis) = normalized_normal(actual)?;
        let orientation = actual[axis].sign();
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
        let distance = dot(normal, source.values[0])?;
        let retained_normal = vector(retained_plane.normal)?;
        let retained_distance = Rational::from_f64(retained_plane.dist)?;
        work.charge(64, budget)?;
        if dot(normal, retained_normal)?.sign() != orientation {
            return Err("Exact emission actual native face must point outward relative to its retained plane".into());
        }
        let mut planar = true;
        let mut retained_support = true;
        for &point in &source.values {
            work.charge(128, budget)?;
            if dot(normal, point)? != distance {
                planar = false;
            }
            if dot(retained_normal, point)? != retained_distance {
                retained_support = false;
            }
        }
        for index in 0..source.values.len() {
            let a = Point(axes.map(|axis| source.values[index][axis]));
            let b = Point(axes.map(|axis| source.values[(index + 1) % source.values.len()][axis]));
            for (other, &point) in source.values.iter().enumerate() {
                work.charge(64, budget)?;
                let point = Point(axes.map(|axis| point[axis]));
                if other != index && a == point {
                    return Err(
                        "Exact emission actual native face repeats a projected source corner"
                            .into(),
                    );
                }
                let side = turn(a, b, point)?.sign();
                if side != Ordering::Equal && side != orientation.reverse() {
                    return Err(
                        "Exact emission requires a simple convex actual native source projection"
                            .into(),
                    );
                }
            }
        }
        let mut fan = Storage::new(points.len() - 2, budget)?;
        let first = Point(axes.map(|axis| source.values[0][axis]));
        for pair in source.values[1..].windows(2) {
            work.charge(64, budget)?;
            let side = turn(
                first,
                Point(axes.map(|axis| pair[1][axis])),
                Point(axes.map(|axis| pair[0][axis])),
            )?
            .sign();
            if !planar {
                work.charge(192, budget)?;
                let actual = triangle_normal([source.values[0], pair[1], pair[0]])?;
                let positive = actual.iter().any(|value| *value != Rational::ZERO);
                if positive != (side != Ordering::Equal) {
                    return Err("Exact emission native fan projection must preserve every positive actual triangle".into());
                }
                if positive && dot(actual, retained_normal)?.sign() != Ordering::Greater {
                    return Err("Exact emission requires each actual native fan triangle to point outward relative to its retained plane".into());
                }
            }
            if side != Ordering::Equal && side != orientation {
                return Err(
                    "Exact emission actual native fan does not cover its outward face".into(),
                );
            }
            fan.push(side != Ordering::Equal)?;
        }
        Ok(Self {
            normal,
            distance,
            axis,
            orientation,
            planar,
            retained_support,
            fan,
            _allocation: allocation,
        })
    }

    pub(super) fn supports_retained_plane(&self) -> bool {
        self.retained_support
    }

    pub(super) fn planar(&self) -> bool {
        self.planar
    }

    fn require_planar(&self) -> Result<(), String> {
        if !self.planar {
            return Err(
                "Exact emission requires a planar certificate for whole-face source operations"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn fan_triangle(
        &self,
        corner: usize,
        budget: &mut Budget<'_>,
    ) -> Result<bool, String> {
        budget.consume(1)?;
        self.fan
            .values
            .get(corner)
            .copied()
            .ok_or_else(|| "Exact emission actual native fan lost its source corner".into())
    }

    pub(super) fn front(&self, point: DVec3, budget: &mut Budget<'_>) -> Result<Ordering, String> {
        budget.consume(64 * ARITHMETIC_WORK_SCALE)?;
        self.require_planar()?;
        let side = dot(self.normal, vector(point)?)?.sub(self.distance)?.sign();
        Ok(if self.orientation == Ordering::Less {
            side.reverse()
        } else {
            side
        })
    }

    pub(super) fn support(
        &self,
        points: [DVec3; 3],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        budget.consume(192 * ARITHMETIC_WORK_SCALE)?;
        self.require_planar()?;
        for point in points {
            self.supported_point(point)?;
        }
        Ok(())
    }

    fn supported_point(&self, point: DVec3) -> Result<[Rational; 3], String> {
        let point = vector(point)?;
        if dot(self.normal, point)? != self.distance {
            return Err("Exact emission converted source vertices must remain on the certified actual native face".into());
        }
        Ok(point)
    }

    pub(super) fn partition(
        &self,
        points: &[DVec3],
        triangles: impl IntoIterator<Item = [DVec3; 3]>,
        triangle_count: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        self.require_planar()?;
        if points.len() != self.fan.values.len() + 2 || triangle_count == 0 {
            return Err(
                "Exact emission texture partition lost its actual native face or positive pieces"
                    .into(),
            );
        }
        let mut work = Work::new("texture partition admission");
        let axes = [(self.axis + 1) % 3, (self.axis + 2) % 3];
        let mut source = Polygon::new(points.len(), budget)?;
        for &point in points {
            work.charge(128, budget)?;
            let point = self.supported_point(point)?;
            source.push(Point(axes.map(|axis| point[axis])))?;
        }
        work.charge(points.len() * 64, budget)?;
        let source_area = double_area(source.values.iter().copied())?;
        if source_area.sign() != self.orientation.reverse() {
            return Err(
                "Exact emission texture partition lost its actual native face winding".into(),
            );
        }
        let mut pieces = Storage::new(triangle_count, budget)?;
        let mut area = Rational::ZERO;
        for triangle in triangles {
            if pieces.values.len() == triangle_count {
                return Err(
                    "Exact emission texture partition disagrees with its charged piece count"
                        .into(),
                );
            }
            let mut piece = [Point([Rational::ZERO; 2]); 3];
            for (output, point) in piece.iter_mut().zip(triangle) {
                work.charge(128, budget)?;
                let point = self.supported_point(point)?;
                *output = Point(axes.map(|axis| point[axis]));
            }
            work.charge(128, budget)?;
            let piece_area = turn(piece[0], piece[1], piece[2])?;
            if piece_area.sign() != self.orientation {
                return Err(
                    "Exact emission texture partition requires positive outward actual triangles"
                        .into(),
                );
            }
            for index in 0..source.values.len() {
                let a = source.values[index];
                let b = source.values[(index + 1) % source.values.len()];
                for point in piece {
                    work.charge(64, budget)?;
                    let side = turn(a, b, point)?.sign();
                    if side != Ordering::Equal && side != self.orientation.reverse() {
                        return Err("Exact emission texture partition crosses its original native face boundary".into());
                    }
                }
            }
            area = area.add(piece_area.abs())?;
            pieces.push(piece)?;
        }
        work.charge(64, budget)?;
        if pieces.values.len() != triangle_count {
            return Err(
                "Exact emission texture partition disagrees with its charged piece count".into(),
            );
        }
        if area != source_area.abs() {
            return Err("Exact emission texture partition does not exactly cover its original native face area".into());
        }
        for (index, piece) in pieces.values.iter().enumerate() {
            for other in &pieces.values[index + 1..] {
                work.charge(1, budget)?;
                if !disjoint_triangles(piece, other, self.orientation, &mut work, budget)? {
                    return Err("Exact emission texture partition has overlapping actual triangle interiors".into());
                }
            }
        }
        Ok(())
    }
}

fn disjoint_triangles(
    a: &[Point; 3],
    b: &[Point; 3],
    orientation: Ordering,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    for (triangle, other) in [(a, b), (b, a)] {
        for index in 0..3 {
            let mut separates = true;
            for &point in other {
                work.charge(64, budget)?;
                if turn(triangle[index], triangle[(index + 1) % 3], point)?.sign() == orientation {
                    separates = false;
                    break;
                }
            }
            if separates {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn receiver_normal(normal: DVec3, budget: &mut Budget<'_>) -> Result<(), String> {
    budget.consume(32 * ARITHMETIC_WORK_SCALE)?;
    let normal = vector(normal)?;
    let bound = normal[0].abs().add(normal[1].abs())?.add(normal[2].abs())?;
    if bound.compare(Rational::new(2, 1)?) == Ordering::Greater {
        return Err("Exact emission receiver normal exceeds its certified kernel bound".into());
    }
    Ok(())
}

fn turn(a: Point, b: Point, c: Point) -> Result<Rational, String> {
    b.0[0]
        .sub(a.0[0])?
        .mul(c.0[1].sub(a.0[1])?)?
        .sub(b.0[1].sub(a.0[1])?.mul(c.0[0].sub(a.0[0])?)?)
}

fn double_area(points: impl IntoIterator<Item = Point>) -> Result<Rational, String> {
    let mut points = points.into_iter();
    let Some(first) = points.next() else {
        return Ok(Rational::ZERO);
    };
    let Some(mut previous) = points.next() else {
        return Ok(Rational::ZERO);
    };
    let mut area = Rational::ZERO;
    for point in points {
        area = area.add(turn(first, previous, point)?)?;
        previous = point;
    }
    Ok(area)
}

fn incoming_edge(
    a: Point,
    b: Point,
    boundary: Halfplane,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Halfplane, String> {
    work.charge(128, budget)?;
    if a == b
        || boundary.constant()
        || boundary.evaluate(a)? != Rational::ZERO
        || boundary.evaluate(b)? != Rational::ZERO
    {
        return Err("Exact emission edge lost its exact incoming boundary equation".into());
    }
    Ok(boundary)
}

fn world_boundary(
    boundary: Halfplane,
    source: &[Point; 3],
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Halfplane, String> {
    work.charge(192, budget)?;
    let dx1 = source[1].0[0].sub(source[0].0[0])?;
    let dx2 = source[2].0[0].sub(source[0].0[0])?;
    let dy1 = source[1].0[1].sub(source[0].0[1])?;
    let dy2 = source[2].0[1].sub(source[0].0[1])?;
    let determinant = dx1.mul(dy2)?.sub(dx2.mul(dy1)?)?;
    if determinant == Rational::ZERO {
        return Err("Exact emission source chart lost its invertible world mapping".into());
    }
    let [a, b, c] = boundary.0.map(|numerator| Rational {
        numerator,
        denominator: n!(1),
    });
    let x = a.mul(dy2)?.sub(b.mul(dy1)?)?;
    let y = b.mul(dx1)?.sub(a.mul(dx2)?)?;
    let constant = c
        .mul(determinant)?
        .sub(x.mul(source[0].0[0])?)?
        .sub(y.mul(source[0].0[1])?)?;
    let mut coefficients = [x, y, constant];
    if determinant.sign() == Ordering::Less {
        coefficients = coefficients.map(Rational::neg);
    }
    Halfplane::new(coefficients, work, budget)
}

enum Split {
    Inside,
    Outside,
    Cut {
        inside: TopoPolygon,
        outside: TopoPolygon,
    },
}

fn split(
    polygon: &TopoPolygon,
    halfplane: Halfplane,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Split, String> {
    work.charge(1, budget)?;
    if halfplane.constant() {
        return Err("Exact emission clipping requires a nonconstant boundary equation".into());
    }
    let mut signs = Storage::new(polygon.values.len(), budget)?;
    let mut negative = false;
    let mut positive = false;
    for vertex in &polygon.values {
        work.charge(48, budget)?;
        let sign = halfplane.evaluate(vertex.point)?.sign();
        negative |= sign == Ordering::Less;
        positive |= sign == Ordering::Greater;
        signs.push(sign)?;
    }
    if !positive {
        return Ok(Split::Inside);
    }
    if !negative {
        return Ok(Split::Outside);
    }
    let capacity = polygon
        .values
        .len()
        .checked_add(1)
        .filter(|capacity| *capacity <= MAX_POLYGON_VERTICES)
        .ok_or("Exact emission exceeds its convex polygon vertex bound")?;
    let mut inside = TopoPolygon::new(capacity, budget)?;
    let mut outside = TopoPolygon::new(capacity, budget)?;
    for index in 0..polygon.values.len() {
        work.charge(1, budget)?;
        let next = (index + 1) % polygon.values.len();
        let a = polygon.values[index];
        let b = polygon.values[next];
        let inside_a = signs.values[index] != Ordering::Greater;
        let inside_b = signs.values[next] != Ordering::Greater;
        let outside_a = signs.values[index] != Ordering::Less;
        let outside_b = signs.values[next] != Ordering::Less;
        if inside_a != inside_b || outside_a != outside_b {
            let edge = incoming_edge(a.point, b.point, b.incoming, work, budget)?;
            let point = if signs.values[index] == Ordering::Equal {
                a.point
            } else if signs.values[next] == Ordering::Equal {
                b.point
            } else {
                edge.intersection(halfplane, work, budget)?
            };
            if inside_a != inside_b {
                inside.push_vertex(TopoVertex {
                    point,
                    incoming: if inside_a { edge } else { halfplane },
                })?;
            }
            if outside_a != outside_b {
                outside.push_vertex(TopoVertex {
                    point,
                    incoming: if outside_a { edge } else { halfplane.neg() },
                })?;
            }
        }
        if inside_b {
            inside.push_vertex(b)?;
        }
        if outside_b {
            outside.push_vertex(b)?;
        }
    }
    for result in [&mut inside, &mut outside] {
        if result.values.first().map(|vertex| vertex.point)
            == result.values.last().map(|vertex| vertex.point)
        {
            let last = result
                .values
                .pop()
                .ok_or("Exact emission split lost its vertices")?;
            result.values[0].incoming = last.incoming;
        }
        if result.values.len() < 3 {
            return Err("Exact emission split lost a positive-area half polygon".into());
        }
    }
    Ok(Split::Cut { inside, outside })
}

fn triangle_planes(
    points: [DVec3; 3],
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<[([Rational; 3], Rational); TRIANGLE_PLANES], String> {
    work.charge(256, budget)?;
    let points = [vector(points[0])?, vector(points[1])?, vector(points[2])?];
    let mut edges = [[Rational::ZERO; 3]; 3];
    for index in 0..3 {
        for coordinate in 0..3 {
            edges[index][coordinate] =
                points[(index + 1) % 3][coordinate].sub(points[index][coordinate])?;
        }
    }
    let normal = hull_cross(edges[0], edges[1])?;
    if normal.iter().all(|value| *value == Rational::ZERO) {
        return Err("Exact emission requires a positive-area opaque triangle".into());
    }
    let distance = dot(normal, points[0])?;
    let mut planes = [([Rational::ZERO; 3], Rational::ZERO); TRIANGLE_PLANES];
    planes[0] = (normal, distance);
    planes[1] = (normal.map(Rational::neg), distance.neg());
    for index in 0..3 {
        work.charge(128, budget)?;
        let outward = hull_cross(edges[index], normal)?;
        planes[index + 2] = (outward, dot(outward, points[index])?);
    }
    Ok(planes)
}

pub(super) fn admit_triangle(points: [DVec3; 3], budget: &mut Budget<'_>) -> Result<(), String> {
    let mut work = Work::new("triangle admission");
    triangle_planes(points, &mut work, budget)?;
    Ok(())
}

fn dyadic_precision(
    source: &[[Rational; 3]; 3],
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Option<u32>, String> {
    work.charge(128, budget)?;
    let power = source
        .iter()
        .flatten()
        .map(|value| value.denominator.trailing_zeros())
        .max()
        .unwrap_or(0);
    let mut bits = 0;
    for value in source.iter().flatten() {
        if !value.numerator.is_zero() {
            bits = bits.max(
                I1024::BITS - value.numerator.abs().leading_zeros() + power
                    - value.denominator.trailing_zeros(),
            );
        }
    }
    Ok(f64::MANTISSA_DIGITS
        .checked_sub(bits)
        .filter(|precision| *precision > 0))
}

fn world_grid_precision(
    source: &[[Rational; 3]; 3],
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<u32, String> {
    work.charge(128, budget)?;
    let mut exponent = 0;
    for value in source.iter().flatten() {
        if !value.numerator.is_zero() {
            let bits = I1024::BITS - value.numerator.abs().leading_zeros();
            exponent = exponent.max(bits as i32 - value.denominator.trailing_zeros() as i32);
        }
    }
    Ok((f64::MANTISSA_DIGITS as i32 - exponent - 2).clamp(0, f64::MANTISSA_DIGITS as i32) as u32)
}

pub(super) struct Visibility {
    source: [[Rational; 3]; 3],
    origin: [Rational; 3],
    orientation: Ordering,
    axis: usize,
    axes: [usize; 2],
    normal: [Rational; 3],
    normal_squared: Rational,
    area_scale_upper: f64,
    fragments: Polygons,
    work: Work,
}

pub(super) struct Fragments {
    pub(super) triangles: Vec<[DVec3; 3]>,
    pub(super) conversion_error_upper: f64,
    _allocation: Allocation,
}

type Equation = ([Rational; 3], Rational);

struct PatchProjection {
    a: [Rational; 3],
    b: Rational,
}

struct MaskMapping([[Rational; 4]; 2]);

impl MaskMapping {
    fn transform(
        rows: [[Rational; 4]; 2],
        mask: &super::super::props::alpha::BinaryMask<'_>,
    ) -> Result<Self, String> {
        let mut result = [[Rational::ZERO; 4]; 2];
        for (axis, row) in result.iter_mut().enumerate() {
            let transform = mask.transform[axis];
            let scale = Rational::new(mask.dimensions[axis] as i128, 1)?;
            for (coordinate, value) in row.iter_mut().enumerate() {
                *value = rows[0][coordinate]
                    .mul(Rational::from_f64(transform[0])?)?
                    .add(rows[1][coordinate].mul(Rational::from_f64(transform[1])?)?)?;
                if coordinate == 3 {
                    *value = value.add(Rational::from_f64(transform[2])?)?;
                }
                *value = value.mul(scale)?;
            }
        }
        Ok(Self(result))
    }

    fn triangle(
        points: [DVec3; 3],
        coordinates: [DVec2; 3],
        mask: &super::super::props::alpha::BinaryMask<'_>,
    ) -> Result<Self, String> {
        let coordinates = [
            [
                Rational::from_f64(coordinates[0].x)?,
                Rational::from_f64(coordinates[0].y)?,
            ],
            [
                Rational::from_f64(coordinates[1].x)?,
                Rational::from_f64(coordinates[1].y)?,
            ],
            [
                Rational::from_f64(coordinates[2].x)?,
                Rational::from_f64(coordinates[2].y)?,
            ],
        ];
        Self::triangle_coordinates(points, coordinates, mask)
    }

    fn terrain(
        points: [DVec3; 3],
        texture_points: [DVec3; 3],
        projection: [[f32; 4]; 2],
        mask: &super::super::props::alpha::BinaryMask<'_>,
    ) -> Result<Self, String> {
        let mut coordinates = [[Rational::ZERO; 2]; 3];
        for (coordinate, point) in coordinates.iter_mut().zip(texture_points) {
            for (axis, value) in coordinate.iter_mut().enumerate() {
                let row = projection[axis];
                *value = dot(
                    vector(DVec3::new(
                        f64::from(row[0]),
                        f64::from(row[1]),
                        f64::from(row[2]),
                    ))?,
                    vector(point)?,
                )?
                .add(Rational::from_f64(f64::from(row[3]))?)?
                .div(Rational::new(mask.dimensions[axis] as i128, 1)?)?;
            }
        }
        Self::triangle_coordinates(points, coordinates, mask)
    }

    fn triangle_coordinates(
        points: [DVec3; 3],
        coordinates: [[Rational; 2]; 3],
        mask: &super::super::props::alpha::BinaryMask<'_>,
    ) -> Result<Self, String> {
        let points = [vector(points[0])?, vector(points[1])?, vector(points[2])?];
        let mut first = [Rational::ZERO; 3];
        let mut second = first;
        for (axis, (first, second)) in first.iter_mut().zip(&mut second).enumerate() {
            *first = points[1][axis].sub(points[0][axis])?;
            *second = points[2][axis].sub(points[0][axis])?;
        }
        let normal = hull_cross(first, second)?;
        let squared = dot(normal, normal)?;
        let first_gradient = hull_cross(second, normal)?;
        let second_gradient = hull_cross(normal, first)?;
        let mut rows = [[Rational::ZERO; 4]; 2];
        for (axis, row) in rows.iter_mut().enumerate() {
            let base = coordinates[0][axis];
            let a = coordinates[1][axis].sub(base)?;
            let b = coordinates[2][axis].sub(base)?;
            for (coordinate, value) in row.iter_mut().take(3).enumerate() {
                *value = first_gradient[coordinate]
                    .mul(a)?
                    .add(second_gradient[coordinate].mul(b)?)?
                    .div(squared)?;
            }
            row[3] = base.sub(dot([row[0], row[1], row[2]], points[0])?)?;
        }
        Self::transform(rows, mask)
    }

    fn brush(
        projection: [[f32; 4]; 2],
        pose: Option<crate::brush_model_pose::Pose>,
        mask: &super::super::props::alpha::BinaryMask<'_>,
    ) -> Result<Self, String> {
        let mut rows = [[Rational::ZERO; 4]; 2];
        for (axis, row) in rows.iter_mut().enumerate() {
            let projection = projection[axis];
            for (local, coefficient) in projection.iter().take(3).enumerate() {
                let coefficient = Rational::from_f64(f64::from(*coefficient))?;
                let direction = pose.and_then(|pose| pose.rotation).map_or_else(
                    || [DVec3::X, DVec3::Y, DVec3::Z][local],
                    |rotation| rotation.col(local),
                );
                let direction = vector(direction)?;
                for (world, value) in row.iter_mut().take(3).enumerate() {
                    *value = value.add(coefficient.mul(direction[world])?)?;
                }
            }
            row[3] = Rational::from_f64(f64::from(projection[3]))?;
            if let Some(pose) = pose {
                row[3] = row[3].sub(dot([row[0], row[1], row[2]], vector(pose.offset)?)?)?;
            }
            let dimension = Rational::new(mask.dimensions[axis] as i128, 1)?;
            for coefficient in row {
                *coefficient = coefficient.div(dimension)?;
            }
        }
        Self::transform(rows, mask)
    }

    fn bounds(
        &self,
        points: impl IntoIterator<Item = Result<[Rational; 3], String>>,
    ) -> Result<[[Rational; 2]; 2], String> {
        let mut bounds = [[Rational::ZERO; 2]; 2];
        let limit = Rational::from_f64(super::super::texture_regions::MAX_ABS_UV)?;
        for (index, point) in points.into_iter().enumerate() {
            let point = point?;
            for (axis, row) in self.0.iter().enumerate() {
                let value = dot([row[0], row[1], row[2]], point)?.add(row[3])?;
                if value.abs().compare(limit) == Ordering::Greater {
                    return Err("Exact cutout coordinates exceed ±16777216 texels".into());
                }
                if index == 0 {
                    bounds[axis] = [value; 2];
                } else {
                    if value.compare(bounds[axis][0]) == Ordering::Less {
                        bounds[axis][0] = value;
                    }
                    if value.compare(bounds[axis][1]) == Ordering::Greater {
                        bounds[axis][1] = value;
                    }
                }
            }
        }
        Ok(bounds)
    }

    fn boundary(&self, axis: usize, boundary: i64, lower: bool) -> Result<Equation, String> {
        let row = self.0[axis];
        let mut normal = [row[0], row[1], row[2]];
        let mut distance = Rational::new(i128::from(boundary), 1)?.sub(row[3])?;
        if lower {
            normal = normal.map(Rational::neg);
            distance = distance.neg();
        }
        Ok((normal, distance))
    }
}

fn integer_floor(value: Rational) -> Result<i64, String> {
    let mut quotient = value.numerator / value.denominator;
    if value.numerator.is_negative() && !(value.numerator % value.denominator).is_zero() {
        quotient -= n!(1);
    }
    if !(n!(-16777216)..=n!(16777216)).contains(&quotient) {
        return Err("Exact cutout texel index exceeds its coordinate limit".into());
    }
    Ok(quotient.as_::<i64>())
}

impl Visibility {
    pub(super) fn new(
        points: [DVec3; 3],
        origin: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        budget.consume(256 * ARITHMETIC_WORK_SCALE)?;
        let source = [vector(points[0])?, vector(points[1])?, vector(points[2])?];
        let aligned_axis = (0..3)
            .find(|&axis| source[0][axis] == source[1][axis] && source[0][axis] == source[2][axis]);
        let mut normal = [Rational::ZERO; 3];
        let axis = if let Some(axis) = aligned_axis {
            normal[axis] = Rational::ONE;
            axis
        } else {
            budget.consume(512 * ARITHMETIC_WORK_SCALE)?;
            let (actual, axis) = normalized_normal(triangle_normal(source)?)?;
            normal = actual;
            axis
        };
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
        let normal_squared = dot(normal, normal)?;
        // The normalized L1 norm bounds the projected-to-surface area scale above.
        let area_scale_upper = normal[0]
            .abs()
            .add(normal[1].abs())?
            .add(normal[2].abs())?
            .upper()?;
        let orientation = turn(
            Point([source[0][axes[0]], source[0][axes[1]]]),
            Point([source[1][axes[0]], source[1][axes[1]]]),
            Point([source[2][axes[0]], source[2][axes[1]]]),
        )?
        .sign();
        if orientation == Ordering::Equal {
            return Err("Exact emission requires a positive-area source triangle".into());
        }
        let mut triangle = TopoPolygon::new(3, budget)?;
        triangle.push(TopoVertex {
            point: Point([Rational::ZERO, Rational::ZERO]),
            incoming: Halfplane([n!(-1), n!(0), n!(0)]),
        })?;
        triangle.push(TopoVertex {
            point: Point([Rational::ONE, Rational::ZERO]),
            incoming: Halfplane([n!(0), n!(-1), n!(0)]),
        })?;
        triangle.push(TopoVertex {
            point: Point([Rational::ZERO, Rational::ONE]),
            incoming: Halfplane([n!(1), n!(1), n!(-1)]),
        })?;
        let mut fragments = Polygons::new(budget)?;
        fragments.push(triangle)?;
        Ok(Self {
            source,
            origin: vector(origin)?,
            orientation,
            axis,
            axes,
            normal,
            normal_squared,
            area_scale_upper,
            fragments,
            work: Work::new("source triangle"),
        })
    }

    pub(super) fn subtract(
        &mut self,
        planes: &[Plane],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let count = planes.len();
        if count == 0 || count > MAX_HULL_PLANES {
            return Err("Exact emission requires 1 to 128 planes for one original hull".into());
        }
        self.subtract_equations(
            planes.iter().map(|plane| {
                Ok((
                    (vector(plane.normal)?, Rational::from_f64(plane.dist)?),
                    false,
                ))
            }),
            budget,
        )
    }

    pub(super) fn subtract_triangle(
        &mut self,
        points: [DVec3; 3],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        self.work.charge(1, budget)?;
        if self.fragments.polygons.values.is_empty() {
            return Ok(());
        }
        let planes = triangle_planes(points, &mut self.work, budget)?;
        let boundaries = [(planes[2], false), (planes[3], false), (planes[4], false)];
        self.subtract_patch(planes[0], &boundaries, budget)
    }

    pub(super) fn subtract_masked_triangle(
        &mut self,
        points: [DVec3; 3],
        coordinates: [DVec2; 3],
        material: &super::super::props::alpha::Material,
        regions: &super::cutout::Regions,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        self.work.charge(1024, budget)?;
        if self.fragments.polygons.values.is_empty() {
            return Ok(());
        }
        let planes = triangle_planes(points, &mut self.work, budget)?;
        let mask = material.binary_mask()?;
        let mapping = MaskMapping::triangle(points, coordinates, &mask)?;
        let bounds = mapping.bounds(points.into_iter().map(vector))?;
        let boundaries = [(planes[2], false), (planes[3], false), (planes[4], false)];
        self.subtract_masked_patch(
            planes[0],
            &boundaries,
            &mapping,
            bounds,
            &mask,
            regions,
            budget,
        )
    }

    pub(super) fn subtract_terrain(
        &mut self,
        provider: &super::super::terrain_shadows::TerrainTriangle,
        masks: &super::cutout::Cache,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let Some(face) = provider.exact_alpha() else {
            return self.subtract_triangle(provider.positions(), budget);
        };
        self.work.charge(2048, budget)?;
        if self.fragments.polygons.values.is_empty() {
            return Ok(());
        }
        let (material, projection) = face.exact_mapping();
        let points = provider.positions();
        let planes = triangle_planes(points, &mut self.work, budget)?;
        let mask = material.binary_mask()?;
        let mapping =
            MaskMapping::terrain(points, provider.texture_positions(), projection, &mask)?;
        let bounds = mapping.bounds(points.into_iter().map(vector))?;
        let boundaries = [(planes[2], false), (planes[3], false), (planes[4], false)];
        self.subtract_masked_patch(
            planes[0],
            &boundaries,
            &mapping,
            bounds,
            &mask,
            masks.get(material)?,
            budget,
        )
    }

    pub(super) fn subtract_hull(
        &mut self,
        hull: &super::super::Occluder,
        masks: &super::cutout::Cache,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let Some(optical) = hull
            .optical
            .as_ref()
            .filter(|optical| !optical.faces.is_empty())
        else {
            return self.subtract(&hull.planes, budget);
        };
        if optical.faces.len() != hull.planes.len() {
            return Err("Exact cutout hull lost its original boundary correspondence".into());
        }
        for (index, face) in optical.faces.iter().enumerate() {
            self.work.charge(256, budget)?;
            if self.fragments.polygons.values.is_empty() {
                break;
            }
            let plane = hull.planes[index];
            let plane = (vector(plane.normal)?, Rational::from_f64(plane.dist)?);
            let (_, b) = self.segment_values(plane.0, plane.1)?;
            if b == Rational::ZERO {
                continue;
            }
            let mut boundaries = Storage::new(hull.planes.len(), budget)?;
            for boundary in &hull.planes {
                self.work.charge(256, budget)?;
                let boundary = (vector(boundary.normal)?, Rational::from_f64(boundary.dist)?);
                let (_, other) = self.segment_values(boundary.0, boundary.1)?;
                let strict = other != Rational::ZERO && other.sign() != b.sign();
                boundaries.push((boundary, strict))?;
            }
            if let Some(face) = face {
                self.work.charge(2048, budget)?;
                let (material, projection) = face.exact_mapping();
                let mask = material.binary_mask()?;
                let mapping = MaskMapping::brush(projection, optical.pose, &mask)?;
                let corners = masks.hull_bounds(hull)?.corners();
                let bounds = mapping.bounds(corners.into_iter().map(Ok))?;
                self.subtract_masked_patch(
                    plane,
                    &boundaries.values,
                    &mapping,
                    bounds,
                    &mask,
                    masks.get(material)?,
                    budget,
                )?;
            } else {
                self.subtract_patch(plane, &boundaries.values, budget)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn subtract_masked_patch(
        &mut self,
        plane: Equation,
        boundaries: &[(Equation, bool)],
        mapping: &MaskMapping,
        bounds: [[Rational; 2]; 2],
        mask: &super::super::props::alpha::BinaryMask<'_>,
        regions: &super::cutout::Regions,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        use super::super::texture_regions::{self, MAX_RECTANGLE_VISITS, MAX_VISITED_TEXELS};
        self.work.charge(128, budget)?;
        let mut tiles = [[0i64; 2]; 2];
        let mut footprint = 1usize;
        for axis in 0..2 {
            let first = integer_floor(bounds[axis][0])?;
            let last = integer_floor(bounds[axis][1])?;
            let dimension = mask.dimensions[axis] as i64;
            let [first_cell, last_cell] = if mask.clamp[axis] {
                [first.clamp(0, dimension - 1), last.clamp(0, dimension - 1)]
            } else {
                tiles[axis] = [
                    first.div_euclid(dimension),
                    integer_floor(bounds[axis][1])?.div_euclid(dimension),
                ];
                [first, last]
            };
            footprint = footprint
                .checked_mul((last_cell - first_cell + 1) as usize)
                .filter(|count| *count <= MAX_VISITED_TEXELS)
                .ok_or("Exact cutout exceeds 8388608 projected texel visits")?;
        }
        let candidates = (tiles[0][1] - tiles[0][0] + 1) as usize;
        let candidates = candidates
            .checked_mul((tiles[1][1] - tiles[1][0] + 1) as usize)
            .and_then(|count| count.checked_mul(regions.rectangles.len()))
            .filter(|count| *count <= MAX_RECTANGLE_VISITS)
            .ok_or("Exact cutout exceeds 1048576 candidate rectangle visits")?;
        self.work.charge(candidates, budget)?;
        let projection = self.patch_projection(plane, budget)?;
        let base = if let Some(projection) = &projection {
            self.project_constraints(projection, boundaries, budget)?
        } else {
            Storage::new(0, budget)?
        };
        for y in tiles[1][0]..=tiles[1][1] {
            for x in tiles[0][0]..=tiles[0][1] {
                for rectangle in &regions.rectangles {
                    self.work.charge(16, budget)?;
                    if self.fragments.polygons.values.is_empty() {
                        return Ok(());
                    }
                    if !rectangle.value {
                        continue;
                    }
                    let mut region = [[None; 2]; 2];
                    let mut overlap = true;
                    for axis in 0..2 {
                        let extent = if axis == 0 { rectangle.x } else { rectangle.y };
                        let offset = [x, y][axis] * mask.dimensions[axis] as i64;
                        region[axis] = [
                            (!(mask.clamp[axis] && extent[0] == 0))
                                .then_some(extent[0] as i64 + offset),
                            (!(mask.clamp[axis] && extent[1] == mask.dimensions[axis]))
                                .then_some(extent[1] as i64 + offset),
                        ];
                        let low = match region[axis][0] {
                            Some(value) => Rational::new(i128::from(value), 1)?,
                            None => bounds[axis][0].sub(Rational::ONE)?,
                        };
                        let high = match region[axis][1] {
                            Some(value) => Rational::new(i128::from(value), 1)?,
                            None => bounds[axis][1].add(Rational::ONE)?,
                        };
                        overlap &= texture_regions::axis_overlap(
                            bounds[axis],
                            [low, high],
                            true,
                            Rational::compare,
                        );
                    }
                    if !overlap {
                        continue;
                    }
                    let mut clipped = Storage::new(4, budget)?;
                    for (axis, region) in region.into_iter().enumerate() {
                        for (edge, boundary) in region.into_iter().enumerate() {
                            if let Some(boundary) = boundary {
                                self.work.charge(16, budget)?;
                                let value = Rational::new(i128::from(boundary), 1)?;
                                let redundant = if edge == 0 {
                                    value.compare(bounds[axis][0]) != Ordering::Greater
                                } else {
                                    value.compare(bounds[axis][1]) == Ordering::Greater
                                };
                                if redundant {
                                    continue;
                                }
                                self.work.charge(64, budget)?;
                                clipped.push((
                                    mapping.boundary(axis, boundary, edge == 0)?,
                                    edge == 1,
                                ))?;
                            }
                        }
                    }
                    if let Some(projection) = &projection {
                        let mut constraints =
                            Storage::new(base.values.len() + clipped.values.len(), budget)?;
                        for &constraint in &base.values {
                            self.work.charge(1, budget)?;
                            constraints.push(constraint)?;
                        }
                        for &(boundary, strict) in &clipped.values {
                            let constraint = self.project_boundary(projection, boundary, budget)?;
                            constraints.push((constraint, strict))?;
                        }
                        self.subtract_constraints(&constraints.values, budget)?;
                    } else {
                        let mut equations =
                            Storage::new(boundaries.len() + clipped.values.len(), budget)?;
                        for &boundary in boundaries {
                            equations.push(boundary)?;
                        }
                        for &boundary in &clipped.values {
                            equations.push(boundary)?;
                        }
                        self.subtract_coplanar_patch(plane, &equations.values, budget)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn subtract_patch(
        &mut self,
        plane: Equation,
        boundaries: &[(Equation, bool)],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let Some(projection) = self.patch_projection(plane, budget)? else {
            return self.subtract_coplanar_patch(plane, boundaries, budget);
        };
        let constraints = self.project_constraints(&projection, boundaries, budget)?;
        self.subtract_constraints(&constraints.values, budget)
    }

    fn subtract_coplanar_patch(
        &mut self,
        plane: Equation,
        boundaries: &[(Equation, bool)],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let mut equations = Storage::new(boundaries.len() + 2, budget)?;
        equations.push((plane, false))?;
        equations.push(((plane.0.map(Rational::neg), plane.1.neg()), false))?;
        for &boundary in boundaries {
            equations.push(boundary)?;
        }
        self.subtract_equations(equations.values.into_iter().map(Ok), budget)
    }

    fn patch_projection(
        &mut self,
        plane: Equation,
        budget: &mut Budget<'_>,
    ) -> Result<Option<PatchProjection>, String> {
        self.work.charge(256, budget)?;
        let (values, b) = self.segment_values(plane.0, plane.1)?;
        if b == Rational::ZERO {
            return Ok(None);
        }
        let a = [
            values[1].sub(values[0])?,
            values[2].sub(values[0])?,
            values[0],
        ];
        Ok(Some(PatchProjection { a, b }))
    }

    fn project_constraints(
        &mut self,
        projection: &PatchProjection,
        boundaries: &[(Equation, bool)],
        budget: &mut Budget<'_>,
    ) -> Result<Storage<(Halfplane, bool)>, String> {
        let PatchProjection { a, b } = *projection;
        let negative = b.sign() == Ordering::Less;
        let mut endpoint = [a[0].neg(), a[1].neg(), b.sub(a[2])?];
        if negative {
            endpoint = endpoint.map(Rational::neg);
        }
        let mut constraints = Storage::new(boundaries.len() + 1, budget)?;
        constraints.push((Halfplane::new(endpoint, &mut self.work, budget)?, true))?;
        for &(boundary, strict) in boundaries {
            let constraint = self.project_boundary(projection, boundary, budget)?;
            constraints.push((constraint, strict))?;
        }
        Ok(constraints)
    }

    fn project_boundary(
        &mut self,
        projection: &PatchProjection,
        (normal, distance): Equation,
        budget: &mut Budget<'_>,
    ) -> Result<Halfplane, String> {
        self.work.charge(384, budget)?;
        let PatchProjection { a, b } = *projection;
        let (values, scale) = self.segment_values(normal, distance)?;
        let other = [
            values[1].sub(values[0])?,
            values[2].sub(values[0])?,
            values[0],
        ];
        let mut coefficients = [
            b.mul(other[0])?.sub(scale.mul(a[0])?)?,
            b.mul(other[1])?.sub(scale.mul(a[1])?)?,
            b.mul(other[2])?.sub(scale.mul(a[2])?)?,
        ];
        if b.sign() == Ordering::Less {
            coefficients = coefficients.map(Rational::neg);
        }
        Halfplane::new(coefficients, &mut self.work, budget)
    }

    fn segment_values(
        &self,
        normal: [Rational; 3],
        distance: Rational,
    ) -> Result<([Rational; 3], Rational), String> {
        let origin = dot(normal, self.origin)?;
        Ok((
            [
                dot(normal, self.source[0])?.sub(origin)?,
                dot(normal, self.source[1])?.sub(origin)?,
                dot(normal, self.source[2])?.sub(origin)?,
            ],
            distance.sub(origin)?,
        ))
    }

    fn subtract_equations(
        &mut self,
        planes: impl ExactSizeIterator<Item = Result<(Equation, bool), String>>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let count = planes.len();
        let mut lower = Storage::new(count, budget)?;
        let mut upper = Storage::new(count, budget)?;
        let mut parallel = Storage::new(count, budget)?;
        for plane in planes {
            self.work.charge(256, budget)?;
            let ((normal, distance), strict) = plane?;
            let (values, b) = self.segment_values(normal, distance)?;
            self.work.charge(32, budget)?;
            if (b.sign() == Ordering::Less
                && values
                    .iter()
                    .all(|value| value.compare(b) != Ordering::Less))
                || (b == Rational::ZERO
                    && values
                        .iter()
                        .all(|value| value.compare(b) == Ordering::Greater))
            {
                return Ok(());
            }
            let a = [
                values[1].sub(values[0])?,
                values[2].sub(values[0])?,
                values[0],
            ];
            match b.sign() {
                Ordering::Less => upper.push((a, b, strict))?,
                Ordering::Equal => {
                    parallel.push((Halfplane::new(a, &mut self.work, budget)?, strict))?;
                }
                Ordering::Greater => lower.push((a, b, strict))?,
            }
        }
        let constraint_count =
            upper.values.len() * (lower.values.len() + 1) + parallel.values.len();
        let mut constraints = Storage::new(constraint_count, budget)?;
        for halfplane in parallel.values {
            constraints.push(halfplane)?;
        }
        for &(a, b, strict) in &upper.values {
            self.work.charge(32, budget)?;
            constraints.push((
                Halfplane::new([a[0], a[1], a[2].sub(b)?], &mut self.work, budget)?,
                true,
            ))?;
            for &(other, scale, other_strict) in &lower.values {
                self.work.charge(128, budget)?;
                constraints.push((
                    Halfplane::new(
                        [
                            scale.mul(a[0])?.sub(b.mul(other[0])?)?,
                            scale.mul(a[1])?.sub(b.mul(other[1])?)?,
                            scale.mul(a[2])?.sub(b.mul(other[2])?)?,
                        ],
                        &mut self.work,
                        budget,
                    )?,
                    strict || other_strict,
                ))?;
            }
        }
        self.subtract_constraints(&constraints.values, budget)
    }

    fn subtract_constraints(
        &mut self,
        constraints: &[(Halfplane, bool)],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        for &(halfplane, strict) in constraints {
            self.work.charge(1, budget)?;
            if halfplane.constant()
                && (halfplane.0[2] > n!(0) || (strict && halfplane.0[2].is_zero()))
            {
                return Ok(());
            }
        }
        let mut next = Polygons::new(budget)?;
        for polygon in self.fragments.polygons.values.drain(..) {
            let start = next.polygons.values.len();
            let vertices = next.vertices;
            let mut inside: Option<TopoPolygon> = None;
            for &(halfplane, _) in constraints {
                self.work.charge(1, budget)?;
                if halfplane.constant() {
                    continue;
                }
                let current = inside.as_ref().unwrap_or(&polygon);
                match split(current, halfplane, &mut self.work, budget)? {
                    Split::Inside => (),
                    Split::Outside => {
                        next.polygons.values.truncate(start);
                        next.vertices = vertices;
                        next.push(polygon)?;
                        break;
                    }
                    Split::Cut {
                        inside: cut,
                        outside,
                    } => {
                        next.push(outside)?;
                        inside = Some(cut);
                    }
                }
            }
        }
        self.fragments = next;
        Ok(())
    }

    fn coordinate(&self, point: Point, axis: usize) -> Result<Rational, String> {
        self.source[0][axis]
            .add(
                self.source[1][axis]
                    .sub(self.source[0][axis])?
                    .mul(point.0[0])?,
            )?
            .add(
                self.source[2][axis]
                    .sub(self.source[0][axis])?
                    .mul(point.0[1])?,
            )
    }

    fn world(&self, point: Point) -> Result<Point, String> {
        Ok(Point([
            self.coordinate(point, self.axes[0])?,
            self.coordinate(point, self.axes[1])?,
        ]))
    }

    fn lift(&self, point: Point) -> Result<[Rational; 3], String> {
        let mut result = self.source[0];
        for (coordinate, axis) in self.axes.into_iter().enumerate() {
            result[axis] = point.0[coordinate];
            if self.normal[axis] != Rational::ZERO {
                result[self.axis] = result[self.axis]
                    .sub(self.normal[axis].mul(point.0[coordinate].sub(self.source[0][axis])?)?)?;
            }
        }
        Ok(result)
    }

    pub(super) fn finish(
        mut self,
        sample_point: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<Fragments, String> {
        self.fragments.coalesce(&mut self.work, budget)?;
        self.work.charge(128, budget)?;
        let sample = vector(sample_point)?;
        let oblique = self.normal_squared != Rational::ONE;
        let mut height = sample[self.axis].sub(self.source[0][self.axis])?;
        if oblique {
            self.work.charge(128, budget)?;
            for axis in self.axes {
                height =
                    height.add(self.normal[axis].mul(sample[axis].sub(self.source[0][axis])?)?)?;
            }
        }
        if height == Rational::ZERO {
            return Err("Exact emission kernel point lies on the source plane".into());
        }
        let height_squared = height.mul(height)?.div(self.normal_squared)?;
        let mut distance_squared_lower = if oblique {
            Rational::ZERO
        } else {
            height_squared
        };
        for axis in self.axes.into_iter().chain(oblique.then_some(self.axis)) {
            self.work.charge(128, budget)?;
            let mut coordinates = self.source.map(|point| point[axis]);
            coordinates.sort_unstable_by(|left, right| left.compare(*right));
            let gap = if sample[axis].compare(coordinates[0]) == Ordering::Less {
                coordinates[0].sub(sample[axis])?
            } else if sample[axis].compare(coordinates[2]) == Ordering::Greater {
                sample[axis].sub(coordinates[2])?
            } else {
                Rational::ZERO
            };
            distance_squared_lower = distance_squared_lower.add(gap.mul(gap)?)?;
        }
        if distance_squared_lower.compare(height_squared) == Ordering::Less {
            distance_squared_lower = height_squared;
        }
        let source = self
            .source
            .map(|point| Point(self.axes.map(|axis| point[axis])));
        let triangle_capacity = self.fragments.vertices;
        let mut result = Storage::new(triangle_capacity, budget)?;
        let mut coarse_error = 0.0;
        let mut error = 0.0;
        for index in 0..self.fragments.polygons.values.len() {
            let polygon = &self.fragments.polygons.values[index];
            self.work.charge(polygon.values.len() * 64, budget)?;
            let mut world = Polygon::new(polygon.values.len(), budget)?;
            for vertex in &polygon.values {
                world.push(self.world(vertex.point)?)?;
            }
            let mut boundaries = Storage::new(polygon.values.len(), budget)?;
            for (index, vertex) in polygon.values.iter().enumerate() {
                let boundary = world_boundary(vertex.incoming, &source, &mut self.work, budget)?;
                let previous = (index + world.values.len() - 1) % world.values.len();
                boundaries.push(incoming_edge(
                    world.values[previous],
                    world.values[index],
                    boundary,
                    &mut self.work,
                    budget,
                )?)?;
            }
            let orientation = self.orientation;
            let mut center = None;
            let mut center_index = 0;
            for index in 1..world.values.len() - 1 {
                self.work.charge(64, budget)?;
                if turn(
                    world.values[0],
                    world.values[index],
                    world.values[index + 1],
                )? != Rational::ZERO
                {
                    let divisor = Rational::new(3, 1)?;
                    center = Some(Point([
                        world.values[0].0[0]
                            .add(world.values[index].0[0])?
                            .add(world.values[index + 1].0[0])?
                            .div(divisor)?,
                        world.values[0].0[1]
                            .add(world.values[index].0[1])?
                            .add(world.values[index + 1].0[1])?
                            .div(divisor)?,
                    ]));
                    center_index = index;
                    break;
                }
            }
            let center = center.ok_or("Exact emission cannot condition a positive fragment")?;
            let mut converted = None;
            let mut rejection = "no conversion candidate";
            for family in 0..3 {
                if family != 0 && !oblique {
                    break;
                }
                let world_power = if family == 1 {
                    Some(world_grid_precision(&self.source, &mut self.work, budget)?)
                } else {
                    None
                };
                let barycentric = if family != 2 {
                    None
                } else {
                    let Some(precision) = dyadic_precision(&self.source, &mut self.work, budget)?
                    else {
                        rejection =
                            "native source coordinates leave no dyadic barycentric precision";
                        break;
                    };
                    self.work.charge(128, budget)?;
                    let points = [
                        polygon.values[0].point,
                        polygon.values[center_index].point,
                        polygon.values[center_index + 1].point,
                    ];
                    let divisor = Rational::new(3, 1)?;
                    let center = Point([
                        points[0].0[0]
                            .add(points[1].0[0])?
                            .add(points[2].0[0])?
                            .div(divisor)?,
                        points[0].0[1]
                            .add(points[1].0[1])?
                            .add(points[2].0[1])?
                            .div(divisor)?,
                    ]);
                    Some((center, precision))
                };
                'conversion: for contraction in 0..18 {
                    self.work.charge(world.values.len() * 96, budget)?;
                    let fraction = if contraction == 0 {
                        Rational::ZERO
                    } else if barycentric.is_some() {
                        Rational::new(1, 1i128 << (21 - contraction))?
                    } else {
                        Rational::new(1, 1i128 << (49 - contraction))?
                    };
                    let mut candidate = Polygon::new(world.values.len(), budget)?;
                    if let Some((center, precision)) = barycentric {
                        for vertex in &polygon.values {
                            self.work.charge(256, budget)?;
                            let point = vertex.point;
                            let weight = Point([
                                point.0[0]
                                    .add(center.0[0].sub(point.0[0])?.mul(fraction)?)?
                                    .round_dyadic(precision)?,
                                point.0[1]
                                    .add(center.0[1].sub(point.0[1])?.mul(fraction)?)?
                                    .round_dyadic(precision)?,
                            ]);
                            if weight.0[0].sign() == Ordering::Less
                                || weight.0[1].sign() == Ordering::Less
                                || weight.0[0].add(weight.0[1])?.compare(Rational::ONE)
                                    == Ordering::Greater
                            {
                                rejection =
                                    "dyadic barycentric weights leave the actual source triangle";
                                continue 'conversion;
                            }
                            let native = [
                                self.coordinate(weight, 0)?,
                                self.coordinate(weight, 1)?,
                                self.coordinate(weight, 2)?,
                            ];
                            for coordinate in native {
                                if Rational::from_f64(coordinate.approximate())? != coordinate {
                                    rejection = "dyadic source mapping is not exactly representable in binary64";
                                    continue 'conversion;
                                }
                            }
                            candidate.push(Point(self.axes.map(|axis| native[axis])))?;
                        }
                    } else if let Some(power) = world_power {
                        for &point in &world.values {
                            self.work.charge(128, budget)?;
                            let point = Point([
                                point.0[0]
                                    .add(center.0[0].sub(point.0[0])?.mul(fraction)?)?
                                    .round_signed_dyadic(power)?,
                                point.0[1]
                                    .add(center.0[1].sub(point.0[1])?.mul(fraction)?)?
                                    .round_signed_dyadic(power)?,
                            ]);
                            for coordinate in self.lift(point)? {
                                if Rational::from_f64(coordinate.approximate())? != coordinate {
                                    rejection = "common world-grid source lift is not exactly representable in binary64";
                                    continue 'conversion;
                                }
                            }
                            candidate.push(point)?;
                        }
                    } else {
                        for &point in &world.values {
                            let x = point.0[0]
                                .add(center.0[0].sub(point.0[0])?.mul(fraction)?)?
                                .approximate();
                            let y = point.0[1]
                                .add(center.0[1].sub(point.0[1])?.mul(fraction)?)?
                                .approximate();
                            let point = Point([Rational::from_f64(x)?, Rational::from_f64(y)?]);
                            if oblique {
                                self.work.charge(128, budget)?;
                                let coordinate = self.lift(point)?[self.axis];
                                if Rational::from_f64(coordinate.approximate())? != coordinate {
                                    rejection = "source-plane lift is not exactly representable in binary64";
                                    continue 'conversion;
                                }
                            }
                            candidate.push(point)?;
                        }
                    }
                    if let Some(reason) = conversion_rejection(
                        &boundaries.values,
                        &candidate.values,
                        orientation,
                        &mut self.work,
                        budget,
                    )? {
                        rejection = reason;
                        continue;
                    }
                    self.work.charge(candidate.values.len() * 64, budget)?;
                    let converted_area = double_area(candidate.values.iter().copied())?;
                    if converted_area.sign() != orientation {
                        rejection = "collapsed or reversed area";
                        continue;
                    }
                    let bound = conversion_error(
                        &world.values,
                        &candidate.values,
                        orientation,
                        distance_squared_lower,
                        &mut self.work,
                        budget,
                    )?;
                    let bound = if self.area_scale_upper == 1.0 {
                        bound
                    } else {
                        (bound * self.area_scale_upper).next_up()
                    };
                    converted = Some((candidate, bound));
                    break;
                }
                if converted.is_some() {
                    break;
                }
            }
            let Some((candidate, bound)) = converted else {
                let preview: [[f64; 2]; 4] = std::array::from_fn(|index| {
                    world
                        .values
                        .get(index)
                        .map_or([0.0; 2], |point| point.0.map(Rational::approximate))
                });
                return Err(format!(
                    "Exact emission cannot certify a native binary64 conversion of a positive fragment within its numeric scope: {rejection}; shadow origin {:?}, source triangle {:?}, fragment {index}/{} with {} vertices, first world source-plane vertices {:?}",
                    self.origin.map(Rational::approximate),
                    self.source.map(|point| point.map(Rational::approximate)),
                    self.fragments.polygons.values.len(),
                    world.values.len(),
                    &preview[..world.values.len().min(preview.len())],
                ));
            };
            if bound > 0.0 {
                coarse_error = (coarse_error + bound).next_up();
            }
            error = coarse_error;
            if coarse_error > MAX_CONVERSION_ERROR {
                self.work.charge(256, budget)?;
                let scale = Rational::from_f64(self.area_scale_upper)?;
                if self.normal_squared.compare(scale.mul(scale)?) == Ordering::Greater {
                    return Err(
                        "Exact emission source area lost its certified scale upper bound".into(),
                    );
                }
                let jacobian_upper = self.normal_squared.upper()?.sqrt().next_up();
                let exact_jacobian = Rational::from_f64(jacobian_upper)?;
                if exact_jacobian
                    .mul(exact_jacobian)?
                    .compare(self.normal_squared)
                    == Ordering::Less
                {
                    return Err("Exact emission cannot certify an outward source area bound".into());
                }
                let jacobian_ratio = (jacobian_upper / self.area_scale_upper).next_up().min(1.0);
                let ratio = height_squared.div(distance_squared_lower)?;
                if ratio.compare(Rational::ONE) == Ordering::Greater {
                    return Err(
                        "Exact emission source cosine lost its certified distance lower bound"
                            .into(),
                    );
                }
                let cosine_upper = ratio.upper()?.sqrt().next_up().min(1.0);
                let exact_cosine = Rational::from_f64(cosine_upper)?;
                if exact_cosine.mul(exact_cosine)?.compare(ratio) == Ordering::Less {
                    return Err(
                        "Exact emission cannot certify an outward source cosine bound".into(),
                    );
                }
                error = ((coarse_error * jacobian_ratio).next_up() * cosine_upper).next_up();
            }
            if !error.is_finite() || error > MAX_CONVERSION_ERROR {
                return Err(format!(
                    "Exact emission geometric conversion exceeds 1e-12 integral error for one source triangle: bound {error}, coarse bound {coarse_error}; source triangle {:?}",
                    self.source.map(|point| point.map(Rational::approximate)),
                ));
            }
            for index in 1..candidate.values.len() - 1 {
                self.work.charge(256, budget)?;
                let points = [
                    candidate.values[0],
                    candidate.values[index],
                    candidate.values[index + 1],
                ];
                let triangle_area = turn(points[0], points[1], points[2])?;
                if triangle_area == Rational::ZERO {
                    continue;
                }
                if triangle_area.sign() != orientation {
                    return Err("Exact emission conversion reversed a visible triangle".into());
                }
                if oblique {
                    self.work.charge(384, budget)?;
                }
                let lifted = [
                    self.lift(points[0])?,
                    self.lift(points[1])?,
                    self.lift(points[2])?,
                ];
                condition(
                    lifted,
                    sample,
                    [self.axes[0], self.axes[1], self.axis],
                    height_squared,
                    triangle_area.abs(),
                    &mut self.work,
                    budget,
                )?;
                let mut triangle = [DVec3::ZERO; 3];
                for (output, point) in triangle.iter_mut().zip(lifted) {
                    *output = DVec3::from_array(point.map(Rational::approximate));
                }
                let native_normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                if !native_normal.is_finite()
                    || native_normal[self.axis] == 0.0
                    || native_normal[self.axis].is_sign_positive()
                        != (orientation == Ordering::Greater)
                {
                    return Err("Exact emission kernel conversion lost triangle orientation".into());
                }
                if oblique {
                    self.work.charge(64, budget)?;
                    let facing = native_normal.dot(sample_point - triangle[0]);
                    if !facing.is_finite()
                        || facing == 0.0
                        || facing.is_sign_positive() != (height.sign() == orientation)
                    {
                        return Err(
                            "Exact emission kernel conversion cannot certify source facing".into(),
                        );
                    }
                }
                result.push(triangle)?;
            }
        }
        Ok(Fragments {
            triangles: result.values,
            conversion_error_upper: error,
            _allocation: result._allocation,
        })
    }
}

fn conversion_rejection(
    boundaries: &[Halfplane],
    candidate: &[Point],
    orientation: Ordering,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Option<&'static str>, String> {
    for &boundary in boundaries {
        for &point in candidate {
            work.charge(64, budget)?;
            if boundary.evaluate(point)?.sign() == Ordering::Greater {
                return Ok(Some("rounded vertex outside exact fragment"));
            }
        }
    }
    for index in 0..candidate.len() {
        let a = candidate[index];
        let b = candidate[(index + 1) % candidate.len()];
        for (other, &point) in candidate.iter().enumerate() {
            work.charge(64, budget)?;
            if other != index && a == point {
                return Ok(Some("rounded vertices collapsed"));
            }
            let sign = turn(a, b, point)?.sign();
            if sign != Ordering::Equal && sign != orientation {
                return Ok(Some("rounded fragment lost convex support"));
            }
        }
    }
    Ok(None)
}

fn condition(
    points: [[Rational; 3]; 3],
    sample: [Rational; 3],
    axes: [usize; 3],
    height_squared: Rational,
    projected_area: Rational,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let [x, y, z] = axes;
    let mut maximum_edge_squared = 0.0f64;
    let mut minimum_edge_squared = f64::INFINITY;
    let mut maximum_distance_squared = 0.0f64;
    for index in 0..3 {
        work.charge(128, budget)?;
        let a = points[index];
        let b = points[(index + 1) % 3];
        let edge = [b[x].sub(a[x])?, b[y].sub(a[y])?, b[z].sub(a[z])?];
        let edge_squared = dot(edge, edge)?;
        maximum_edge_squared = maximum_edge_squared.max(edge_squared.upper()?);
        minimum_edge_squared = minimum_edge_squared.min(edge_squared.lower()?);
        let distance = [
            a[x].sub(sample[x])?,
            a[y].sub(sample[y])?,
            a[z].sub(sample[z])?,
        ];
        maximum_distance_squared = maximum_distance_squared.max(dot(distance, distance)?.upper()?);
    }
    let condition_lower = (projected_area.lower()? / maximum_edge_squared).next_down();
    let span_lower = (minimum_edge_squared / maximum_distance_squared).next_down();
    let plane_span_lower = (height_squared.lower()? / maximum_distance_squared).next_down();
    if condition_lower < MIN_TRIANGLE_CONDITION
        || span_lower < MIN_ANGULAR_SPAN_SQUARED
        || plane_span_lower < MIN_ANGULAR_SPAN_SQUARED
    {
        return Err("Exact emission refuses a visible sliver or source span outside its kernel conditioning scope".into());
    }
    Ok(())
}

fn hull_cross(a: [Rational; 3], b: [Rational; 3]) -> Result<[Rational; 3], String> {
    Ok([
        a[1].mul(b[2])?.sub(a[2].mul(b[1])?)?,
        a[2].mul(b[0])?.sub(a[0].mul(b[2])?)?,
        a[0].mul(b[1])?.sub(a[1].mul(b[0])?)?,
    ])
}

pub(super) struct HullBounds([[Rational; 2]; 3]);

impl HullBounds {
    fn from_certificates(certified: [Option<Rational>; 6]) -> Result<Self, String> {
        let mut bounds = [[Rational::ZERO; 2]; 3];
        for (axis, bound) in bounds.iter_mut().enumerate() {
            let upper =
                certified[axis * 2].ok_or("Exact emission hull lost its upper axis certificate")?;
            let lower = certified[axis * 2 + 1]
                .ok_or("Exact emission hull lost its lower axis certificate")?
                .neg();
            if lower.compare(upper) != Ordering::Less {
                return Err("Exact emission hull axis certificates do not enclose volume".into());
            }
            *bound = [lower, upper];
        }
        Ok(Self(bounds))
    }

    fn corners(&self) -> [[Rational; 3]; 8] {
        std::array::from_fn(|corner| std::array::from_fn(|axis| self.0[axis][(corner >> axis) & 1]))
    }
}

fn hull_axis_certificate(
    planes: [([Rational; 3], Rational); 3],
    coefficients: [Rational; 3],
    axis: usize,
    negative: bool,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Option<Rational>, String> {
    work.charge(16, budget)?;
    if coefficients
        .iter()
        .any(|coefficient| coefficient.sign() == Ordering::Less)
    {
        return Ok(None);
    }
    work.charge(96, budget)?;
    for coordinate in 0..3 {
        let expected = if coordinate != axis {
            Rational::ZERO
        } else if negative {
            Rational::ONE.neg()
        } else {
            Rational::ONE
        };
        let actual = dot(
            coefficients,
            [
                planes[0].0[coordinate],
                planes[1].0[coordinate],
                planes[2].0[coordinate],
            ],
        )?;
        if actual != expected {
            return Err("Exact emission hull certificate failed exact axis reconstruction".into());
        }
    }
    let upper = dot(coefficients, [planes[0].1, planes[1].1, planes[2].1])?;
    Ok(Some(upper))
}

pub(super) fn closed_hull(
    planes: &[Plane],
    interior: DVec3,
    budget: &mut Budget<'_>,
) -> Result<HullBounds, String> {
    budget.consume(0)?;
    if !(4..=MAX_HULL_PLANES).contains(&planes.len()) {
        return Err("Exact emission hull certification requires 4 to 128 final planes".into());
    }
    let mut work = Work::new("hull certificate");
    work.charge(64, budget)?;
    let interior = vector(interior)?;
    work.charge(1, budget)?;
    let mut exact = Storage::new(planes.len(), budget)?;
    for plane in planes {
        work.charge(128, budget)?;
        let normal = vector(plane.normal)?;
        let distance = Rational::from_f64(plane.dist)?;
        if dot(normal, interior)?.compare(distance) != Ordering::Less {
            return Err("Exact emission hull requires a certified strict interior point behind every final plane".into());
        }
        exact.push((normal, distance))?;
    }
    let mut certified = [None; 6];
    let mut invertible = false;
    for first in 0..exact.values.len() - 2 {
        for second in first + 1..exact.values.len() - 1 {
            for third in second + 1..exact.values.len() {
                work.charge(128, budget)?;
                let selected = [
                    exact.values[first],
                    exact.values[second],
                    exact.values[third],
                ];
                let cofactors = [
                    hull_cross(selected[1].0, selected[2].0)?,
                    hull_cross(selected[2].0, selected[0].0)?,
                    hull_cross(selected[0].0, selected[1].0)?,
                ];
                let determinant = dot(selected[0].0, cofactors[0])?;
                if determinant == Rational::ZERO {
                    continue;
                }
                invertible = true;
                for (target, complete) in certified.iter_mut().enumerate() {
                    if complete.is_some() {
                        continue;
                    }
                    work.charge(64, budget)?;
                    let axis = target / 2;
                    let negative = target % 2 != 0;
                    let mut coefficients = [
                        cofactors[0][axis].div(determinant)?,
                        cofactors[1][axis].div(determinant)?,
                        cofactors[2][axis].div(determinant)?,
                    ];
                    if negative {
                        coefficients = coefficients.map(Rational::neg);
                    }
                    *complete = hull_axis_certificate(
                        selected,
                        coefficients,
                        axis,
                        negative,
                        &mut work,
                        budget,
                    )?;
                }
                if certified.iter().all(Option::is_some) {
                    work.charge(96, budget)?;
                    return HullBounds::from_certificates(certified);
                }
            }
        }
    }
    if !invertible {
        return Err("Exact emission hull normals do not span three dimensions".into());
    }
    Err("Exact emission hull lacks an exact nonnegative axis-bound certificate; its final halfspaces are not certified bounded".into())
}
