use std::cell::RefCell;

use glam::DVec3;
use map_core::Plane;

use super::Occluder;
use super::bake::Budget;
use super::shadows::{ShadowIndex, TRIANGLE_EPSILON, Visit};
use super::transport::Scene;

const ROUNDING_OPERATIONS: f64 = 64.0;
const ORIGIN_WORK: usize = 32;
const PLANE_WORK: usize = 16;
const PROP_INTERSECTION_WORK: usize = 48;
const QUERY_SCALE: f64 = 2.0;

pub(super) enum Source<'a> {
    Plane {
        plane: usize,
        equation: Plane,
        sides: &'a [u32],
        owners: &'a [usize],
        error: f64,
    },
    Terrain {
        patch: usize,
        uv: [f64; 2],
    },
}

struct Clearance {
    required: f64,
    first: Option<f64>,
    length: f64,
    maximum: f64,
    rounding: f64,
}

impl Clearance {
    fn constrain(&mut self, fraction: f64) {
        let distance = fraction * self.length;
        if distance > 0.0 && distance <= self.maximum + self.rounding {
            let distance = distance.min(self.maximum);
            self.first = Some(self.first.map_or(distance, |first| first.min(distance)));
        }
    }

    fn limit(&self) -> f64 {
        self.first.unwrap_or(self.maximum) / self.length
    }
}

fn rounding(plane: Plane, point: DVec3) -> f64 {
    ((plane.normal * point).abs().element_sum() + plane.dist.abs() + 1.0)
        * (ROUNDING_OPERATIONS * f64::EPSILON)
}

fn source_plane(
    source: &Source<'_>,
    hull: &Occluder,
    consume: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<Option<usize>, String> {
    let Source::Plane {
        plane,
        equation,
        sides,
        owners,
        ..
    } = source
    else {
        return Ok(None);
    };
    let Some(identity) = &hull.source else {
        return Ok(None);
    };
    consume(owners.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
    if owners.binary_search(&identity.brush).is_err() {
        return Ok(None);
    }
    let lookup = sides.len().checked_ilog2().unwrap_or(0) as usize + 1;
    for (index, (side, actual)) in identity.sides.iter().zip(&hull.planes).enumerate() {
        consume(lookup + 2)?;
        if side.plane == *plane
            && side
                .side
                .is_some_and(|side| sides.binary_search(&side).is_ok())
            && actual.normal == equation.normal
            && actual.dist == equation.dist
        {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn hull_clearance(
    hulls: &ShadowIndex<Occluder>,
    source: &Source<'_>,
    point: DVec3,
    full_end: DVec3,
    direction: DVec3,
    clearance: &mut Clearance,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let budget = RefCell::new(budget);
    hulls.visit(
        point,
        full_end,
        &mut |work| budget.borrow_mut().consume(work),
        &mut |hull| {
            let mut consume = |work| budget.borrow_mut().consume(work);
            let own = source_plane(source, hull, &mut consume)?;
            consume(hull.plane_count() * PLANE_WORK)?;
            let Some(interval) = hull.interval(point, full_end)? else {
                return Ok(Visit::Continue(clearance.limit()));
            };
            let mut exempt = None;
            if interval.entering == 0.0
                && let Some(index) = own
            {
                let plane = hull.planes[index];
                let distance = plane.distance_to(point);
                let slope = plane.normal.dot(direction);
                if slope > 0.0 {
                    let crossing = -distance / slope;
                    if crossing >= 0.0
                        && (crossing / clearance.length - interval.leaving).abs()
                            <= TRIANGLE_EPSILON
                    {
                        let allowance = rounding(plane, point).max(rounding(plane, full_end));
                        clearance.required = clearance
                            .required
                            .max(((allowance - distance) / slope).max(0.0));
                        exempt = Some(interval.leaving);
                    }
                }
            }
            for fraction in [interval.entering, interval.leaving] {
                if fraction > 0.0 && fraction < 1.0 && Some(fraction) != exempt {
                    clearance.constrain(fraction);
                }
            }
            Ok(Visit::Continue(clearance.limit()))
        },
    )?;
    Ok(())
}

fn certify_source_hulls(
    hulls: &ShadowIndex<Occluder>,
    source: &Source<'_>,
    point: DVec3,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let budget = RefCell::new(budget);
    hulls.visit(
        point,
        point,
        &mut |work| budget.borrow_mut().consume(work),
        &mut |hull| {
            let mut consume = |work| budget.borrow_mut().consume(work);
            if source_plane(source, hull, &mut consume)?.is_none() {
                return Ok(Visit::Continue(1.0));
            }
            consume(hull.plane_count() * PLANE_WORK)?;
            if !hull
                .planes
                .iter()
                .any(|&plane| plane.distance_to(point) > rounding(plane, point))
            {
                return Err("Sample origin cannot leave its certified source hull".into());
            }
            Ok(Visit::Continue(1.0))
        },
    )?;
    Ok(())
}

pub(super) fn origin(
    scene: &Scene<'_>,
    source: &Source<'_>,
    point: DVec3,
    bias: DVec3,
    maximum: f64,
    budget: &mut Budget<'_>,
) -> Result<DVec3, String> {
    budget.consume(ORIGIN_WORK)?;
    if !point.is_finite()
        || !bias.is_finite()
        || !bias.length_squared().is_finite()
        || bias.length_squared() == 0.0
        || !maximum.is_finite()
        || maximum <= 0.0
    {
        return Err("Sample origin needs a finite point, direction and positive maximum".into());
    }
    let direction = bias.normalize();
    let length = maximum * QUERY_SCALE;
    let full_end = point + direction * length;
    if !length.is_finite() || !full_end.is_finite() || full_end == point {
        return Err("Sample origin clearance ray must be finite and noncollapsed".into());
    }
    if let Source::Plane {
        equation,
        sides,
        error,
        ..
    } = source
    {
        budget.consume(sides.len() + PLANE_WORK)?;
        let distance = equation.distance_to(point);
        let slope = equation.normal.dot(direction);
        let allowance = rounding(*equation, point);
        if !error.is_finite()
            || *error < 0.0
            || sides.windows(2).any(|pair| pair[0] >= pair[1])
            || !equation.normal.is_finite()
            || !equation.dist.is_finite()
            || !distance.is_finite()
            || !slope.is_finite()
            || !allowance.is_finite()
            || slope <= 0.0
            || distance.abs() > *error + allowance
        {
            return Err("Sample origin does not match its certified native source plane".into());
        }
    }
    let numerical =
        (point.abs().max_element() + maximum + 1.0) * (ROUNDING_OPERATIONS * f64::EPSILON);
    let mut clearance = Clearance {
        required: numerical,
        first: None,
        length,
        maximum,
        rounding: numerical,
    };
    let terrain_contact = if let Source::Terrain { patch, uv } = source {
        let contact = scene
            .terrain
            .receiver_contact(*patch, *uv, point, &mut |work| budget.consume(work))?;
        clearance.required = clearance
            .required
            .max(scene.terrain.contact_required_offset(
                &contact,
                point,
                direction,
                &mut |work| budget.consume(work),
            )?);
        Some(contact)
    } else {
        None
    };
    for hulls in std::iter::once(scene.shadows).chain(scene.model) {
        hull_clearance(
            hulls,
            source,
            point,
            full_end,
            direction,
            &mut clearance,
            budget,
        )?;
    }
    if let Some(fraction) = scene.terrain.geometric_clearance(
        point,
        full_end,
        terrain_contact.as_ref(),
        &mut |work| budget.consume(work),
    )? {
        clearance.constrain(fraction);
    }
    {
        let budget = RefCell::new(&mut *budget);
        scene.props.visit(
            point,
            full_end,
            &mut |work| budget.borrow_mut().consume(work),
            &mut |triangle| {
                budget.borrow_mut().consume(PROP_INTERSECTION_WORK)?;
                if let Some(fraction) = triangle.geometric_fraction(point, full_end)? {
                    clearance.constrain(fraction);
                }
                Ok(Visit::Continue(clearance.limit()))
            },
        )?;
    }
    let upper = clearance.first.unwrap_or(maximum);
    if !clearance.required.is_finite() || clearance.required >= upper {
        return Err(
            "Sample origin has no clearance between its source and another boundary".into(),
        );
    }
    let distance = if clearance.first.is_some() {
        clearance.required + (upper - clearance.required) * 0.5
    } else {
        maximum
    };
    let origin = point + direction * distance;
    if !origin.is_finite() || origin == point || distance <= clearance.required {
        return Err("Sample origin has no representable certified clearance".into());
    }
    if let Some(contact) = &terrain_contact
        && !scene
            .terrain
            .contact_origin_clear(contact, origin, direction, &mut |work| budget.consume(work))?
    {
        return Err("Sample origin cannot leave its certified native terrain source".into());
    }
    for hulls in std::iter::once(scene.shadows).chain(scene.model) {
        certify_source_hulls(hulls, source, origin, budget)?;
    }
    Ok(origin)
}
