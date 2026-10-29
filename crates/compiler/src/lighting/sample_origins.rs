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
const BOUNDARY_TRACKING_WORK: usize = 4;
const CANDIDATE_WORK: usize = 64;
const CANDIDATE_LIMIT: usize = 8;
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

#[derive(Clone, Copy)]
struct HullLimit {
    slot: usize,
    fraction: f64,
}

enum Placement {
    Placed(DVec3),
    NoPlacement { hint: Option<DVec3> },
}

#[derive(Clone, Copy)]
struct Candidate {
    bias: DVec3,
    direction: DVec3,
}

impl Clearance {
    fn constrain(&mut self, fraction: f64) -> bool {
        let previous = self.first;
        let distance = fraction * self.length;
        if distance > 0.0 && distance <= self.maximum + self.rounding {
            let distance = distance.min(self.maximum);
            self.first = Some(self.first.map_or(distance, |first| first.min(distance)));
        }
        self.first != previous
    }

    fn limit(&self) -> f64 {
        self.first.unwrap_or(self.maximum) / self.length
    }
}

fn rounding(plane: Plane, point: DVec3) -> f64 {
    ((plane.normal * point).abs().element_sum() + plane.dist.abs() + 1.0)
        * (ROUNDING_OPERATIONS * f64::EPSILON)
}

fn exit_clearance(plane: Plane, point: DVec3, end: DVec3, distance: f64, slope: f64) -> f64 {
    let allowance = rounding(plane, point).max(rounding(plane, end));
    ((allowance - distance) / slope).max(0.0)
}

fn source_contact_plane(
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
    let mut coincident = None;
    for (index, (side, actual)) in identity.sides.iter().zip(&hull.planes).enumerate() {
        consume(lookup + 2)?;
        if side.plane == *plane
            && actual.normal == equation.normal
            && actual.dist == equation.dist
            && let Some(side) = side.side
        {
            if sides.binary_search(&side).is_ok() {
                return Ok(Some(index));
            }
            consume(2)?;
            coincident.get_or_insert(index);
        }
    }
    Ok(coincident)
}

fn hull_clearance(
    hulls: &ShadowIndex<Occluder>,
    source: &Source<'_>,
    point: DVec3,
    full_end: DVec3,
    direction: DVec3,
    clearance: &mut Clearance,
    budget: &mut Budget<'_>,
) -> Result<Option<HullLimit>, String> {
    let budget = RefCell::new(budget);
    let mut limiting = None;
    hulls.visit_indexed(
        point,
        full_end,
        &mut |work| budget.borrow_mut().consume(work),
        &mut |slot, hull| {
            let mut consume = |work| budget.borrow_mut().consume(work);
            let contact = source_contact_plane(source, hull, &mut consume)?;
            consume(hull.plane_count() * PLANE_WORK)?;
            let Some(interval) = hull.interval(point, full_end)? else {
                return Ok(Visit::Continue(clearance.limit()));
            };
            let mut exempt = None;
            if interval.entering == 0.0
                && let Some(index) = contact
            {
                let plane = hull.planes[index];
                let distance = plane.distance_to(point);
                let slope = plane.normal.dot(direction);
                if slope > 0.0 {
                    let crossing = -distance / slope;
                    if crossing >= 0.0
                        && interval.leaving < 1.0
                        && interval.leaving <= crossing / clearance.length + TRIANGLE_EPSILON
                    {
                        let end_distance = plane.distance_to(full_end);
                        let required = if end_distance > 0.0
                            && distance / (distance - end_distance) == interval.leaving
                        {
                            Some(exit_clearance(plane, point, full_end, distance, slope))
                        } else {
                            let mut required: Option<f64> = None;
                            for (candidate_index, &exit) in hull.planes.iter().enumerate() {
                                if candidate_index == index {
                                    continue;
                                }
                                consume(PLANE_WORK)?;
                                let a = exit.distance_to(point);
                                let b = exit.distance_to(full_end);
                                if a <= 0.0 && b > 0.0 && a / (a - b) == interval.leaving {
                                    consume(3 * PLANE_WORK)?;
                                    let slope = exit.normal.dot(direction);
                                    if slope > 0.0 {
                                        let candidate =
                                            exit_clearance(exit, point, full_end, a, slope);
                                        required = Some(
                                            required
                                                .map_or(candidate, |value| value.min(candidate)),
                                        );
                                    }
                                }
                            }
                            required
                        };
                        if let Some(required) = required {
                            clearance.required = clearance.required.max(required);
                            exempt = Some(interval.leaving);
                        }
                    }
                }
            }
            for fraction in [interval.entering, interval.leaving] {
                if fraction > 0.0 && fraction < 1.0 && Some(fraction) != exempt {
                    consume(BOUNDARY_TRACKING_WORK)?;
                    if clearance.constrain(fraction) {
                        limiting = Some(HullLimit { slot, fraction });
                    }
                }
            }
            Ok(Visit::Continue(clearance.limit()))
        },
    )?;
    Ok(limiting)
}

fn boundary_hint(
    hull: &Occluder,
    fraction: f64,
    point: DVec3,
    end: DVec3,
    budget: &mut Budget<'_>,
) -> Result<DVec3, String> {
    for &plane in &hull.planes {
        budget.consume(2 * PLANE_WORK)?;
        let a = plane.distance_to(point);
        let b = plane.distance_to(end);
        if !a.is_finite() || !b.is_finite() {
            return Err("Sample origin limiting plane has nonfinite support".into());
        }
        if (a > 0.0 && b <= 0.0) || (a <= 0.0 && b > 0.0) {
            let denominator = a - b;
            let crossing = a / denominator;
            if !denominator.is_finite() || !crossing.is_finite() {
                return Err("Sample origin limiting plane has invalid arithmetic".into());
            }
            if crossing == fraction {
                budget.consume(ORIGIN_WORK)?;
                let safe = if a > 0.0 { plane.normal } else { -plane.normal };
                return safe
                    .try_normalize()
                    .ok_or_else(|| "Sample origin limiting plane has an invalid normal".into());
            }
        }
    }
    Err("Sample origin limiting hull boundary cannot be certified".into())
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
            if source_contact_plane(source, hull, &mut consume)?.is_none() {
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

fn placement(
    scene: &Scene<'_>,
    source: &Source<'_>,
    point: DVec3,
    bias: DVec3,
    maximum: f64,
    budget: &mut Budget<'_>,
) -> Result<Placement, String> {
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
    let mut limiting_hull = None;
    for hulls in std::iter::once(scene.shadows).chain(scene.model) {
        if let Some(limit) = hull_clearance(
            hulls,
            source,
            point,
            full_end,
            direction,
            &mut clearance,
            budget,
        )? {
            limiting_hull = Some((hulls, limit));
        }
    }
    if let Some(fraction) = scene.terrain.geometric_clearance(
        point,
        full_end,
        terrain_contact.as_ref(),
        &mut |work| budget.consume(work),
    )? {
        budget.consume(BOUNDARY_TRACKING_WORK)?;
        if clearance.constrain(fraction) {
            limiting_hull = None;
        }
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
                    budget.borrow_mut().consume(BOUNDARY_TRACKING_WORK)?;
                    if clearance.constrain(fraction) {
                        limiting_hull = None;
                    }
                }
                Ok(Visit::Continue(clearance.limit()))
            },
        )?;
    }
    let upper = clearance.first.unwrap_or(maximum);
    if !clearance.required.is_finite() || !upper.is_finite() {
        return Err("Sample origin clearance bounds must be finite".into());
    }
    if clearance.required >= upper {
        let hint = if matches!(source, Source::Plane { .. })
            && let Some((hulls, limit)) = limiting_hull
        {
            let hull = hulls
                .objects()
                .get(limit.slot)
                .ok_or("Sample origin limiting hull is unavailable")?;
            Some(boundary_hint(
                hull,
                limit.fraction,
                point,
                full_end,
                budget,
            )?)
        } else {
            None
        };
        return Ok(Placement::NoPlacement { hint });
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
    Ok(Placement::Placed(origin))
}

fn enqueue_candidate(
    candidates: &mut [Candidate; CANDIDATE_LIMIT],
    count: &mut usize,
    base: DVec3,
    hint: DVec3,
    source_normal: DVec3,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    if *count == candidates.len() {
        return Ok(());
    }
    budget.consume(CANDIDATE_WORK)?;
    let bias = base + hint;
    let Some(direction) = bias.try_normalize() else {
        return Ok(());
    };
    let outward = source_normal.dot(direction);
    if !direction.is_finite() || !outward.is_finite() || outward <= 0.0 {
        return Ok(());
    }
    budget.consume(*count * 4)?;
    if candidates[..*count]
        .iter()
        .any(|candidate| candidate.direction == direction)
    {
        return Ok(());
    }
    candidates[*count] = Candidate { bias, direction };
    *count += 1;
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
    let mut hint = match placement(scene, source, point, bias, maximum, budget)? {
        Placement::Placed(origin) => return Ok(origin),
        Placement::NoPlacement { hint } => hint,
    };
    let Source::Plane { equation, .. } = source else {
        return Err(
            "Sample origin has no clearance between its source and another boundary".into(),
        );
    };
    budget.consume(CANDIDATE_WORK)?;
    let native = bias.normalize();
    let mut current = native;
    let mut candidates = [Candidate {
        bias: DVec3::ZERO,
        direction: DVec3::ZERO,
    }; CANDIDATE_LIMIT];
    candidates[0] = Candidate {
        bias,
        direction: native,
    };
    let mut count = 1;
    let mut next = 1;
    loop {
        if let Some(safe) = hint {
            enqueue_candidate(
                &mut candidates,
                &mut count,
                native,
                safe,
                equation.normal,
                budget,
            )?;
            enqueue_candidate(
                &mut candidates,
                &mut count,
                current,
                safe,
                equation.normal,
                budget,
            )?;
        }
        if next == count {
            return Err(
                "Sample origin has no certified planar contact escape within its direction budget"
                    .into(),
            );
        }
        let candidate = candidates[next];
        next += 1;
        current = candidate.direction;
        match placement(scene, source, point, candidate.bias, maximum, budget)? {
            Placement::Placed(origin) => return Ok(origin),
            Placement::NoPlacement { hint: blocked } => hint = blocked,
        }
    }
}
