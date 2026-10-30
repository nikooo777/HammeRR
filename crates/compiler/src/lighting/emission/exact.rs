use std::cmp::Ordering;

use bnum::{cast::As, n, types::I512};
use glam::DVec3;
use map_core::Plane;

use super::super::bake::{Allocation, Budget};

const MAX_PLANES: usize = 128;
const MAX_CONSTRAINTS: usize = MAX_PLANES * MAX_PLANES / 4 + MAX_PLANES;
const MAX_FRAGMENTS: usize = 4096;
const MAX_VERTICES: usize = 65_536;
const MAX_POLYGON_VERTICES: usize = MAX_CONSTRAINTS + 3;
const MAX_WORK: usize = 32_000_000;
const ARITHMETIC_WORK_SCALE: usize = 4;
const MAX_DYADIC_POWER: i32 = 510;
const MAX_CONVERSION_ERROR: f64 = 1e-12;
const MIN_TRIANGLE_CONDITION: f64 = 1.0 / (1u64 << 32) as f64;
const MIN_ANGULAR_SPAN_SQUARED: f64 = 1.0 / (1u64 << 48) as f64;
const ARITHMETIC_ERROR: &str =
    "Exact emission exceeds checked 511-bit rational arithmetic; sampled fallback is disabled";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rational {
    numerator: I512,
    denominator: I512,
}

fn gcd(mut a: I512, mut b: I512) -> I512 {
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
        let numerator = numerator.as_::<I512>();
        let denominator = denominator.as_::<I512>();
        if denominator <= n!(0) {
            return Err(ARITHMETIC_ERROR.into());
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
        let mut numerator = mantissa.as_::<I512>();
        let denominator = if power >= 0 {
            if power > MAX_DYADIC_POWER || numerator > (I512::MAX >> power) {
                return Err(ARITHMETIC_ERROR.into());
            }
            numerator <<= power;
            n!(1)
        } else {
            if power < -MAX_DYADIC_POWER {
                return Err(ARITHMETIC_ERROR.into());
            }
            n!(1I512) << -power
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
        if numerator == I512::MIN {
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
        if numerator == I512::MIN {
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
struct Halfplane([Rational; 3]);

impl Halfplane {
    fn evaluate(self, point: Point) -> Result<Rational, String> {
        self.0[0]
            .mul(point.0[0])?
            .add(self.0[1].mul(point.0[1])?)?
            .add(self.0[2])
    }

    fn neg(self) -> Self {
        Self(self.0.map(Rational::neg))
    }

    fn constant(self) -> bool {
        self.0[0] == Rational::ZERO && self.0[1] == Rational::ZERO
    }
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

struct Polygons {
    polygons: Storage<Polygon>,
    vertices: usize,
}

impl Polygons {
    fn new(budget: &mut Budget<'_>) -> Result<Self, String> {
        Ok(Self {
            polygons: Storage::new(MAX_FRAGMENTS, budget)?,
            vertices: 0,
        })
    }

    fn push(&mut self, polygon: Polygon) -> Result<(), String> {
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

#[derive(Default)]
struct Work(usize);

impl Work {
    fn charge(&mut self, amount: usize, budget: &mut Budget<'_>) -> Result<(), String> {
        let amount = amount
            .checked_mul(ARITHMETIC_WORK_SCALE)
            .ok_or("Exact emission work charge overflow")?;
        budget.consume(amount)?;
        self.0 = self
            .0
            .checked_add(amount)
            .filter(|work| *work <= MAX_WORK)
            .ok_or("Exact emission exceeds 32000000 work units for one source triangle")?;
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

fn double_area(points: &[Point]) -> Result<Rational, String> {
    let mut area = Rational::ZERO;
    for index in 1..points.len() - 1 {
        area = area.add(turn(points[0], points[index], points[index + 1])?)?;
    }
    Ok(area)
}

fn clip(
    polygon: &Polygon,
    halfplane: Halfplane,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Option<Polygon>, String> {
    work.charge(1, budget)?;
    let capacity = polygon
        .values
        .len()
        .checked_add(1)
        .filter(|capacity| *capacity <= MAX_POLYGON_VERTICES)
        .ok_or("Exact emission exceeds its convex polygon vertex bound")?;
    let mut result = Polygon::new(capacity, budget)?;
    for index in 0..polygon.values.len() {
        work.charge(96, budget)?;
        let a = polygon.values[index];
        let b = polygon.values[(index + 1) % polygon.values.len()];
        let da = halfplane.evaluate(a)?;
        let db = halfplane.evaluate(b)?;
        if da.sign() != Ordering::Greater && result.values.last() != Some(&a) {
            result.push(a)?;
        }
        if (da.sign() == Ordering::Less && db.sign() == Ordering::Greater)
            || (da.sign() == Ordering::Greater && db.sign() == Ordering::Less)
        {
            let fraction = da.div(da.sub(db)?)?;
            let intersection = Point([
                a.0[0].add(b.0[0].sub(a.0[0])?.mul(fraction)?)?,
                a.0[1].add(b.0[1].sub(a.0[1])?.mul(fraction)?)?,
            ]);
            if result.values.last() != Some(&intersection) {
                result.push(intersection)?;
            }
        }
    }
    if result.values.first() == result.values.last() {
        result.values.pop();
    }
    if result.values.len() < 3 {
        return Ok(None);
    }
    work.charge(result.values.len() * 32, budget)?;
    let area = double_area(&result.values)?;
    match area.sign() {
        Ordering::Less => Err("Exact emission clipping reversed a convex fragment".into()),
        Ordering::Equal => Ok(None),
        Ordering::Greater => Ok(Some(result)),
    }
}

pub(super) struct Visibility {
    source: [[Rational; 3]; 3],
    origin: [Rational; 3],
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
        if turn(
            Point([source[0][axes[0]], source[0][axes[1]]]),
            Point([source[1][axes[0]], source[1][axes[1]]]),
            Point([source[2][axes[0]], source[2][axes[1]]]),
        )? == Rational::ZERO
        {
            return Err("Exact emission requires a positive-area source triangle".into());
        }
        let mut triangle = Polygon::new(3, budget)?;
        triangle.push(Point([Rational::ZERO, Rational::ZERO]))?;
        triangle.push(Point([Rational::ONE, Rational::ZERO]))?;
        triangle.push(Point([Rational::ZERO, Rational::ONE]))?;
        let mut fragments = Polygons::new(budget)?;
        fragments.push(triangle)?;
        Ok(Self {
            source,
            origin: vector(origin)?,
            axis,
            axes,
            fragments,
            work: Work::default(),
        })
    }

    pub(super) fn subtract(
        &mut self,
        planes: impl IntoIterator<Item = Plane>,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        let mut lower = Storage::new(MAX_PLANES, budget)?;
        let mut upper = Storage::new(MAX_PLANES, budget)?;
        let mut parallel = Storage::new(MAX_PLANES, budget)?;
        let mut count = 0usize;
        for plane in planes {
            self.work.charge(256, budget)?;
            count += 1;
            if count > MAX_PLANES {
                return Err("Exact emission exceeds 128 planes for one original hull".into());
            }
            let normal = vector(plane.normal)?;
            let origin = dot(normal, self.origin)?;
            let b = Rational::from_f64(plane.dist)?.sub(origin)?;
            let values = [
                dot(normal, self.source[0])?.sub(origin)?,
                dot(normal, self.source[1])?.sub(origin)?,
                dot(normal, self.source[2])?.sub(origin)?,
            ];
            let a = Halfplane([
                values[1].sub(values[0])?,
                values[2].sub(values[0])?,
                values[0],
            ]);
            match b.sign() {
                Ordering::Less => upper.push((a, b))?,
                Ordering::Equal => parallel.push(a)?,
                Ordering::Greater => lower.push((a, b))?,
            }
        }
        if count == 0 {
            return Err("Exact emission received an empty original hull".into());
        }
        let constraint_count =
            upper.values.len() * (lower.values.len() + 1) + parallel.values.len();
        let mut constraints = Storage::new(constraint_count, budget)?;
        for halfplane in parallel.values {
            constraints.push((halfplane, false))?;
        }
        for &(a, b) in &upper.values {
            self.work.charge(32, budget)?;
            constraints.push((Halfplane([a.0[0], a.0[1], a.0[2].sub(b)?]), true))?;
            for &(other, scale) in &lower.values {
                self.work.charge(128, budget)?;
                constraints.push((
                    Halfplane([
                        scale.mul(a.0[0])?.sub(b.mul(other.0[0])?)?,
                        scale.mul(a.0[1])?.sub(b.mul(other.0[1])?)?,
                        scale.mul(a.0[2])?.sub(b.mul(other.0[2])?)?,
                    ]),
                    false,
                ))?;
            }
        }
        for &(halfplane, strict) in &constraints.values {
            self.work.charge(1, budget)?;
            if halfplane.constant()
                && (halfplane.0[2].sign() == Ordering::Greater
                    || (strict && halfplane.0[2] == Rational::ZERO))
            {
                return Ok(());
            }
        }
        let mut next = Polygons::new(budget)?;
        for polygon in self.fragments.polygons.values.drain(..) {
            let mut inside = Some(polygon);
            for &(halfplane, _) in &constraints.values {
                self.work.charge(1, budget)?;
                if halfplane.constant() {
                    continue;
                }
                let Some(polygon) = inside.take() else {
                    break;
                };
                if let Some(outside) = clip(&polygon, halfplane.neg(), &mut self.work, budget)? {
                    next.push(outside)?;
                }
                inside = clip(&polygon, halfplane, &mut self.work, budget)?;
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
        let triangle_capacity = self.fragments.vertices;
        let mut result = Storage::new(triangle_capacity, budget)?;
        let mut error = 0.0;
        for index in 0..self.fragments.polygons.values.len() {
            let polygon = &self.fragments.polygons.values[index];
            self.work.charge(polygon.values.len() * 64, budget)?;
            let mut world = Polygon::new(polygon.values.len(), budget)?;
            for &point in &polygon.values {
                world.push(self.world(point)?)?;
            }
            let area = double_area(&world.values)?;
            if area == Rational::ZERO {
                return Err("Exact emission lost a positive-area source fragment".into());
            }
            let orientation = area.sign();
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
                    &world.values,
                    &candidate.values,
                    orientation,
                    &mut self.work,
                    budget,
                )? {
                    rejection = reason;
                    continue;
                }
                self.work.charge(candidate.values.len() * 64, budget)?;
                let converted_area = double_area(&candidate.values)?;
                if converted_area.sign() != orientation {
                    rejection = "collapsed or reversed area";
                    continue;
                }
                let omitted = area.abs().sub(converted_area.abs())?;
                if omitted.sign() == Ordering::Less {
                    return Err("Exact emission conversion enlarged a visible fragment".into());
                }
                let bound = omitted.div(height_squared)?.upper()?;
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
    polygon: &[Point],
    candidate: &[Point],
    orientation: Ordering,
    work: &mut Work,
    budget: &mut Budget<'_>,
) -> Result<Option<&'static str>, String> {
    for index in 0..polygon.len() {
        let a = polygon[index];
        let b = polygon[(index + 1) % polygon.len()];
        for &point in candidate {
            work.charge(64, budget)?;
            let sign = turn(a, b, point)?.sign();
            if sign != Ordering::Equal && sign != orientation {
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
