use glam::DVec3;

use super::{Exact, invalid, numeric, same};
use crate::EditError;

#[derive(Clone, Copy)]
pub(crate) struct BasePlane {
    axis: usize,
    normal: [Exact; 3],
    distance: Exact,
}

impl BasePlane {
    pub(crate) fn new(points: [DVec3; 3]) -> Result<Self, EditError> {
        let first = difference(points[1], points[0])?;
        let second = difference(points[2], points[0])?;
        let normal = [
            first[1].mul(second[2])?.sub(first[2].mul(second[1])?)?,
            first[2].mul(second[0])?.sub(first[0].mul(second[2])?)?,
            first[0].mul(second[1])?.sub(first[1].mul(second[0])?)?,
        ];
        let axis = normal
            .iter()
            .position(|coefficient| coefficient.value != 0)
            .ok_or_else(|| invalid("Planar terrain requires a noncollapsed plane"))?;
        let mut distance = Exact::new(0, 0);
        for (coefficient, coordinate) in normal.into_iter().zip(points[0].to_array()) {
            distance = distance.add(coefficient.mul(Exact::float(coordinate)?)?)?;
        }
        Ok(Self {
            axis,
            normal,
            distance,
        })
    }

    pub(crate) fn axis(&self) -> usize {
        self.axis
    }

    pub(crate) fn contains(&self, point: DVec3) -> Result<bool, EditError> {
        let mut distance = Exact::new(0, 0);
        for (coefficient, coordinate) in self.normal.into_iter().zip(point.to_array()) {
            distance = distance.add(coefficient.mul(Exact::float(coordinate)?)?)?;
        }
        Ok(distance == self.distance)
    }

    pub(crate) fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if self.axis != other.axis {
            return Ok(false);
        }
        let pivot = self.normal[self.axis];
        let other_pivot = other.normal[self.axis];
        for (first, second) in self
            .normal
            .iter()
            .chain(std::iter::once(&self.distance))
            .zip(other.normal.iter().chain(std::iter::once(&other.distance)))
        {
            if first.mul(other_pivot)? != second.mul(pivot)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone, Copy)]
struct Coefficients {
    numerator: [Exact; 3],
    denominator: Exact,
}

impl Coefficients {
    fn new(plane: BasePlane, base: [DVec3; 3], values: [f64; 3]) -> Result<Self, EditError> {
        let [u, v] = axes(plane.axis);
        let first = difference(base[1], base[0])?;
        let second = difference(base[2], base[0])?;
        let origin = Exact::float(values[0])?;
        let first_value = Exact::float(values[1])?.sub(origin)?;
        let second_value = Exact::float(values[2])?.sub(origin)?;
        let u_coefficient = first_value
            .mul(second[v])?
            .sub(first[v].mul(second_value)?)?;
        let v_coefficient = first[u]
            .mul(second_value)?
            .sub(first_value.mul(second[u])?)?;
        let denominator = plane.normal[plane.axis];
        let constant = origin
            .mul(denominator)?
            .sub(u_coefficient.mul(Exact::float(base[0][u])?)?)?
            .sub(v_coefficient.mul(Exact::float(base[0][v])?)?)?;
        Ok(Self {
            numerator: [u_coefficient, v_coefficient, constant],
            denominator,
        })
    }

    fn numerator_at(&self, point: DVec3, axis: usize) -> Result<Exact, EditError> {
        let [u, v] = axes(axis);
        let [u_coefficient, v_coefficient, constant] = self.numerator;
        constant
            .add(u_coefficient.mul(Exact::float(point[u])?)?)?
            .add(v_coefficient.mul(Exact::float(point[v])?)?)
    }

    fn value(&self, point: DVec3, axis: usize) -> Result<f64, EditError> {
        self.numerator_at(point, axis)?
            .div(self.denominator)?
            .value()
    }

    fn matches_value(&self, point: DVec3, axis: usize, value: f64) -> Result<bool, EditError> {
        Ok(self.numerator_at(point, axis)? == Exact::float(value)?.mul(self.denominator)?)
    }

    fn matches(&self, other: &Self) -> Result<bool, EditError> {
        for (first, second) in self.numerator.into_iter().zip(other.numerator) {
            if first.mul(other.denominator)? != second.mul(self.denominator)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ScalarField {
    plane: BasePlane,
    coefficients: Coefficients,
}

impl ScalarField {
    pub(crate) fn new(base: [DVec3; 4], values: [f64; 4]) -> Result<Self, EditError> {
        let first = [base[0], base[1], base[2]];
        let plane = BasePlane::new(first)?;
        let coefficients = Coefficients::new(plane, first, [values[0], values[1], values[2]])?;
        let result = Self {
            plane,
            coefficients,
        };
        for (point, value) in base.into_iter().zip(values) {
            if result.value(point)? != value {
                return Err(invalid(
                    "Terrain scalar samples must belong to one exact affine field",
                ));
            }
        }
        Ok(result)
    }

    pub(crate) fn value(&self, base: DVec3) -> Result<f64, EditError> {
        if !self.plane.contains(base)? {
            return Err(invalid(
                "Terrain scalar samples must remain on their original base plane",
            ));
        }
        self.coefficients.value(base, self.plane.axis)
    }

    pub(crate) fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if !self.plane.matches(&other.plane)? {
            return Ok(false);
        }
        self.coefficients.matches(&other.coefficients)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PlanarField {
    base: BasePlane,
    world: BasePlane,
    forward: [Coefficients; 3],
    inverse: [Coefficients; 3],
}

impl PlanarField {
    pub(crate) fn triangle(base: [DVec3; 3], world: [DVec3; 3]) -> Result<Self, EditError> {
        Self::new(
            [base[0], base[1], base[2], base[0]],
            [world[0], world[1], world[2], world[0]],
        )
    }

    pub(crate) fn new(base: [DVec3; 4], world: [DVec3; 4]) -> Result<Self, EditError> {
        if base.iter().chain(&world).any(|point| !point.is_finite()) {
            return Err(numeric());
        }
        let base_triangle = [base[0], base[1], base[2]];
        let world_triangle = [world[0], world[1], world[2]];
        let base_plane = BasePlane::new(base_triangle)?;
        let world_plane = BasePlane::new(world_triangle)?;
        let result = Self {
            base: base_plane,
            world: world_plane,
            forward: coefficients(base_plane, base_triangle, world_triangle)?,
            inverse: coefficients(world_plane, world_triangle, base_triangle)?,
        };
        for (base, world) in base.into_iter().zip(world) {
            if !same(result.position(base)?, world) || !same(result.base_position(world)?, base) {
                return Err(invalid(
                    "Planar terrain corners must share one exact rank-two affine mapping",
                ));
            }
        }
        Ok(result)
    }

    pub(crate) fn axis(&self) -> usize {
        self.base.axis
    }

    pub(crate) fn world_axis(&self) -> usize {
        self.world.axis
    }

    pub(crate) fn base_position(&self, world: DVec3) -> Result<DVec3, EditError> {
        if !self.world.contains(world)? {
            return Err(invalid(
                "Terrain point must belong to its exact planar field",
            ));
        }
        let base = evaluate(&self.inverse, world, self.world.axis)?;
        if !same(self.position(base)?, world) {
            return Err(invalid(
                "Terrain point must belong to its exact planar field",
            ));
        }
        Ok(base)
    }

    pub(crate) fn position(&self, base: DVec3) -> Result<DVec3, EditError> {
        if !self.base.contains(base)? {
            return Err(invalid(
                "Planar terrain samples must remain on their original base plane",
            ));
        }
        evaluate(&self.forward, base, self.base.axis)
    }

    pub(crate) fn maps(&self, base: DVec3, world: DVec3) -> Result<bool, EditError> {
        if !self.base.contains(base)? {
            return Ok(false);
        }
        for (field, value) in self.forward.iter().zip(world.to_array()) {
            if !field.matches_value(base, self.base.axis, value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn offset(&self, base: DVec3) -> Result<DVec3, EditError> {
        let position = self.position(base)?;
        let mut result = DVec3::ZERO;
        for axis in 0..3 {
            result[axis] = Exact::float(position[axis])?
                .sub(Exact::float(base[axis])?)?
                .value()?;
        }
        Ok(result)
    }

    pub(crate) fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if !self.base.matches(&other.base)? {
            return Ok(false);
        }
        for (first, second) in self.forward.iter().zip(&other.forward) {
            if !first.matches(second)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn axes(axis: usize) -> [usize; 2] {
    [(axis + 1) % 3, (axis + 2) % 3]
}

fn coefficients(
    plane: BasePlane,
    base: [DVec3; 3],
    world: [DVec3; 3],
) -> Result<[Coefficients; 3], EditError> {
    Ok([
        Coefficients::new(plane, base, world.map(|point| point.x))?,
        Coefficients::new(plane, base, world.map(|point| point.y))?,
        Coefficients::new(plane, base, world.map(|point| point.z))?,
    ])
}

fn evaluate(
    coefficients: &[Coefficients; 3],
    point: DVec3,
    axis: usize,
) -> Result<DVec3, EditError> {
    Ok(DVec3::new(
        coefficients[0].value(point, axis)?,
        coefficients[1].value(point, axis)?,
        coefficients[2].value(point, axis)?,
    ))
}

fn difference(first: DVec3, second: DVec3) -> Result<[Exact; 3], EditError> {
    Ok([
        Exact::float(first.x)?.sub(Exact::float(second.x)?)?,
        Exact::float(first.y)?.sub(Exact::float(second.y)?)?,
        Exact::float(first.z)?.sub(Exact::float(second.z)?)?,
    ])
}
