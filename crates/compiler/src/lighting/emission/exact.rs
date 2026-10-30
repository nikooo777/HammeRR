use std::cmp::Ordering;

use bnum::{cast::As, n, types::I1024};
use glam::DVec3;
use map_core::Plane;

use super::super::bake::{Allocation, Budget};

pub(super) const MAX_HULL_PLANES: usize = 128;
const TRIANGLE_PLANES: usize = 5;
const TRIANGLE_CONSTRAINTS: usize = 4;
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

#[derive(Clone, Copy)]
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

pub(super) fn front(
    point: DVec3,
    plane: Plane,
    budget: &mut Budget<'_>,
) -> Result<Ordering, String> {
    budget.consume(64 * ARITHMETIC_WORK_SCALE)?;
    Ok(dot(vector(plane.normal)?, vector(point)?)?
        .sub(Rational::from_f64(plane.dist)?)?
        .sign())
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

pub(super) struct Visibility {
    source: [[Rational; 3]; 3],
    origin: [Rational; 3],
    orientation: Ordering,
    axis: usize,
    axes: [usize; 2],
    fragments: Polygons,
    work: Work,
}

pub(super) struct Fragments {
    pub(super) triangles: Vec<[DVec3; 3]>,
    pub(super) conversion_error_upper: f64,
    _allocation: Allocation,
}

impl Visibility {
    pub(super) fn new(
        points: [DVec3; 3],
        origin: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        budget.consume(256 * ARITHMETIC_WORK_SCALE)?;
        let source = [vector(points[0])?, vector(points[1])?, vector(points[2])?];
        let axis = (0..3)
            .find(|&axis| source[0][axis] == source[1][axis] && source[0][axis] == source[2][axis])
            .ok_or("Exact emission currently requires an axis-aligned source plane")?;
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
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
            planes
                .iter()
                .map(|plane| Ok((vector(plane.normal)?, Rational::from_f64(plane.dist)?))),
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
        let (normal, distance) = planes[0];
        self.work.charge(256, budget)?;
        let (values, b) = self.segment_values(normal, distance)?;
        if b == Rational::ZERO {
            return self.subtract_equations(planes.into_iter().map(Ok), budget);
        }
        let a = [
            values[1].sub(values[0])?,
            values[2].sub(values[0])?,
            values[0],
        ];
        let negative = b.sign() == Ordering::Less;
        let mut endpoint = [a[0].neg(), a[1].neg(), b.sub(a[2])?];
        if negative {
            endpoint = endpoint.map(Rational::neg);
        }
        let mut constraints = Storage::new(TRIANGLE_CONSTRAINTS, budget)?;
        constraints.push((Halfplane::new(endpoint, &mut self.work, budget)?, true))?;
        for &(normal, distance) in &planes[2..] {
            self.work.charge(384, budget)?;
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
            if negative {
                coefficients = coefficients.map(Rational::neg);
            }
            constraints.push((Halfplane::new(coefficients, &mut self.work, budget)?, false))?;
        }
        self.subtract_constraints(&constraints.values, budget)
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
        planes: impl ExactSizeIterator<Item = Result<([Rational; 3], Rational), String>>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let count = planes.len();
        let mut lower = Storage::new(count, budget)?;
        let mut upper = Storage::new(count, budget)?;
        let mut parallel = Storage::new(count, budget)?;
        for plane in planes {
            self.work.charge(256, budget)?;
            let (normal, distance) = plane?;
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
                Ordering::Less => upper.push((a, b))?,
                Ordering::Equal => {
                    parallel.push(Halfplane::new(a, &mut self.work, budget)?)?;
                }
                Ordering::Greater => lower.push((a, b))?,
            }
        }
        let constraint_count =
            upper.values.len() * (lower.values.len() + 1) + parallel.values.len();
        let mut constraints = Storage::new(constraint_count, budget)?;
        for halfplane in parallel.values {
            constraints.push((halfplane, false))?;
        }
        for &(a, b) in &upper.values {
            self.work.charge(32, budget)?;
            constraints.push((
                Halfplane::new([a[0], a[1], a[2].sub(b)?], &mut self.work, budget)?,
                true,
            ))?;
            for &(other, scale) in &lower.values {
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
                    false,
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

    fn world(&self, point: Point) -> Result<Point, String> {
        let mut result = [Rational::ZERO; 2];
        for (index, &axis) in self.axes.iter().enumerate() {
            result[index] = self.source[0][axis]
                .add(
                    self.source[1][axis]
                        .sub(self.source[0][axis])?
                        .mul(point.0[0])?,
                )?
                .add(
                    self.source[2][axis]
                        .sub(self.source[0][axis])?
                        .mul(point.0[1])?,
                )?;
        }
        Ok(Point(result))
    }

    pub(super) fn finish(
        mut self,
        sample_point: DVec3,
        budget: &mut Budget<'_>,
    ) -> Result<Fragments, String> {
        self.work.charge(128, budget)?;
        let sample = vector(sample_point)?;
        let height = sample[self.axis].sub(self.source[0][self.axis])?.abs();
        if height == Rational::ZERO {
            return Err("Exact emission kernel point lies on the source plane".into());
        }
        let height_squared = height.mul(height)?;
        let mut distance_squared_lower = height_squared;
        for axis in self.axes {
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
        let source = self
            .source
            .map(|point| Point(self.axes.map(|axis| point[axis])));
        let triangle_capacity = self.fragments.vertices;
        let mut result = Storage::new(triangle_capacity, budget)?;
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
                    break;
                }
            }
            let center = center.ok_or("Exact emission cannot condition a positive fragment")?;
            let mut converted = None;
            let mut rejection = "no conversion candidate";
            for contraction in 0..18 {
                self.work.charge(world.values.len() * 96, budget)?;
                let fraction = if contraction == 0 {
                    Rational::ZERO
                } else {
                    Rational::new(1, 1i128 << (49 - contraction))?
                };
                let mut candidate = Polygon::new(world.values.len(), budget)?;
                for &point in &world.values {
                    let x = point.0[0]
                        .add(center.0[0].sub(point.0[0])?.mul(fraction)?)?
                        .approximate();
                    let y = point.0[1]
                        .add(center.0[1].sub(point.0[1])?.mul(fraction)?)?
                        .approximate();
                    candidate.push(Point([Rational::from_f64(x)?, Rational::from_f64(y)?]))?;
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
                converted = Some((candidate, bound));
                break;
            }
            let Some((candidate, bound)) = converted else {
                let preview: [[f64; 2]; 4] = std::array::from_fn(|index| {
                    world
                        .values
                        .get(index)
                        .map_or([0.0; 2], |point| point.0.map(Rational::approximate))
                });
                return Err(format!(
                    "Exact emission cannot round a positive fragment without crossing a shadow boundary: {rejection}; shadow origin {:?}, source triangle {:?}, fragment {index}/{} with {} vertices, first world source-plane vertices {:?}",
                    self.origin.map(Rational::approximate),
                    self.source.map(|point| point.map(Rational::approximate)),
                    self.fragments.polygons.values.len(),
                    world.values.len(),
                    &preview[..world.values.len().min(preview.len())],
                ));
            };
            if bound > 0.0 {
                error = (error + bound).next_up();
            }
            if error > MAX_CONVERSION_ERROR {
                return Err("Exact emission geometric conversion exceeds 1e-12 integral error for one source triangle".into());
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
                condition(
                    points,
                    sample,
                    self.axes,
                    height_squared,
                    triangle_area.abs(),
                    &mut self.work,
                    budget,
                )?;
                let mut triangle = [DVec3::ZERO; 3];
                for (output, point) in triangle.iter_mut().zip(points) {
                    output[self.axis] = self.source[0][self.axis].approximate();
                    output[self.axes[0]] = point.0[0].approximate();
                    output[self.axes[1]] = point.0[1].approximate();
                }
                let native_normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                if !native_normal.is_finite()
                    || native_normal[self.axis] == 0.0
                    || native_normal[self.axis].is_sign_positive()
                        != (orientation == Ordering::Greater)
                {
                    return Err("Exact emission kernel conversion lost triangle orientation".into());
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
    points: [Point; 3],
    sample: [Rational; 3],
    axes: [usize; 2],
    height_squared: Rational,
    area: Rational,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let mut maximum_edge_squared = 0.0f64;
    let mut minimum_edge_squared = f64::INFINITY;
    let mut maximum_distance_squared = 0.0f64;
    for index in 0..3 {
        work.charge(128, budget)?;
        let a = points[index];
        let b = points[(index + 1) % 3];
        let dx = b.0[0].sub(a.0[0])?;
        let dy = b.0[1].sub(a.0[1])?;
        let edge_squared = dx.mul(dx)?.add(dy.mul(dy)?)?;
        maximum_edge_squared = maximum_edge_squared.max(edge_squared.upper()?);
        minimum_edge_squared = minimum_edge_squared.min(edge_squared.lower()?);
        let dx = a.0[0].sub(sample[axes[0]])?;
        let dy = a.0[1].sub(sample[axes[1]])?;
        maximum_distance_squared = maximum_distance_squared
            .max(dx.mul(dx)?.add(dy.mul(dy)?)?.add(height_squared)?.upper()?);
    }
    let condition_lower = (area.lower()? / maximum_edge_squared).next_down();
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

fn hull_axis_certificate(
    planes: [([Rational; 3], Rational); 3],
    coefficients: [Rational; 3],
    axis: usize,
    negative: bool,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    work.charge(16, budget)?;
    if coefficients
        .iter()
        .any(|coefficient| coefficient.sign() == Ordering::Less)
    {
        return Ok(false);
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
    let _upper = dot(coefficients, [planes[0].1, planes[1].1, planes[2].1])?;
    Ok(true)
}

pub(super) fn closed_hull(
    planes: &[Plane],
    interior: DVec3,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
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
    let mut certified = [false; 6];
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
                    if *complete {
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
                if certified.iter().all(|complete| *complete) {
                    return Ok(());
                }
            }
        }
    }
    if !invertible {
        return Err("Exact emission hull normals do not span three dimensions".into());
    }
    Err("Exact emission hull lacks an exact nonnegative axis-bound certificate; its final halfspaces are not certified bounded".into())
}
