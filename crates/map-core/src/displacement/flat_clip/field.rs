use glam::DVec3;

use super::{Exact, invalid, numeric, same};
use crate::EditError;

#[derive(Clone, Copy)]
pub(crate) struct PlanarField {
    axis: usize,
    base: Exact,
    normal: [Exact; 3],
    distance: Exact,
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
        let axis = (0..3)
            .find(|&axis| base.iter().all(|point| point[axis] == base[0][axis]))
            .ok_or_else(|| invalid("Planar terrain requires a world-axis base plane"))?;
        let first = difference(world[1], world[0])?;
        let second = difference(world[2], world[0])?;
        let normal = [
            first[1].mul(second[2])?.sub(first[2].mul(second[1])?)?,
            first[2].mul(second[0])?.sub(first[0].mul(second[2])?)?,
            first[0].mul(second[1])?.sub(first[1].mul(second[0])?)?,
        ];
        if normal[axis].value == 0 {
            return Err(invalid(
                "Planar terrain must have a nonzero height coefficient over its base plane",
            ));
        }
        let mut distance = Exact::new(0, 0);
        for (coefficient, coordinate) in normal.into_iter().zip(world[0].to_array()) {
            distance = distance.add(coefficient.mul(Exact::float(coordinate)?)?)?;
        }
        let result = Self {
            axis,
            base: Exact::float(base[0][axis])?,
            normal,
            distance,
        };
        for (base, world) in base.into_iter().zip(world) {
            if !same(result.position(base)?, world) {
                return Err(invalid(
                    "Planar terrain corners must preserve base tangent coordinates on one exact sheet plane",
                ));
            }
        }
        Ok(result)
    }

    pub(crate) fn axis(&self) -> usize {
        self.axis
    }

    pub(crate) fn base_position(&self, world: DVec3) -> Result<DVec3, EditError> {
        Exact::float(world[self.axis])?;
        let mut base = world;
        base[self.axis] = self.base.value()?;
        if !same(self.position(base)?, world) {
            return Err(invalid(
                "Terrain point must belong to its exact planar field",
            ));
        }
        Ok(base)
    }

    pub(crate) fn position(&self, base: DVec3) -> Result<DVec3, EditError> {
        if Exact::float(base[self.axis])? != self.base {
            return Err(invalid(
                "Planar terrain samples must remain on their original base plane",
            ));
        }
        let mut height = self.distance;
        for (coordinate, coefficient) in self.normal.into_iter().enumerate() {
            if coordinate != self.axis {
                height = height.sub(coefficient.mul(Exact::float(base[coordinate])?)?)?;
            }
        }
        let mut result = base;
        result[self.axis] = height.div(self.normal[self.axis])?.value()?;
        Ok(result)
    }

    pub(crate) fn offset(&self, base: DVec3) -> Result<DVec3, EditError> {
        let position = self.position(base)?;
        let mut result = DVec3::ZERO;
        result[self.axis] = Exact::float(position[self.axis])?
            .sub(Exact::float(base[self.axis])?)?
            .value()?;
        Ok(result)
    }

    pub(crate) fn matches(&self, other: &Self) -> Result<bool, EditError> {
        if self.axis != other.axis || self.base != other.base {
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

fn difference(first: DVec3, second: DVec3) -> Result<[Exact; 3], EditError> {
    Ok([
        Exact::float(first.x)?.sub(Exact::float(second.x)?)?,
        Exact::float(first.y)?.sub(Exact::float(second.y)?)?,
        Exact::float(first.z)?.sub(Exact::float(second.z)?)?,
    ])
}
