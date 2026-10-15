use std::sync::Arc;

use super::{Control, DirectedPortal, Plane, VISIBILITY_EPSILON, Winding, Work, bit, set};

const MAX_STACK_POINTS: usize = 262_144;

struct Frame {
    cluster: usize,
    next: usize,
    source: Arc<Winding>,
    pass: Option<Arc<Winding>>,
}

pub(super) struct Context<'a> {
    pub work: &'a mut Work,
    pub control: &'a Control,
    pub possible: &'a [u8],
    pub bounds: &'a [Vec<u8>],
}

impl Frame {
    fn points(&self) -> usize {
        self.source.0.len() + self.pass.as_ref().map_or(0, |pass| pass.0.len())
    }
}

pub(super) fn refine(
    base: &DirectedPortal,
    portals: &[DirectedPortal],
    edges: &[Vec<usize>],
    eligible: &[u8],
    row: &mut [u8],
    context: Context,
) -> Result<(), String> {
    let Context {
        work,
        control,
        possible,
        bounds,
    } = context;
    control.checkpoint()?;
    set(row, base.to);
    work.consume(row.len().div_ceil(8))?;
    if complete(row, possible) {
        return Ok(());
    }
    let mut on_path = vec![false; edges.len()];
    on_path[base.from] = true;
    on_path[base.to] = true;
    let mut stack = vec![Frame {
        cluster: base.to,
        next: 0,
        source: Arc::new(base.winding.clone()),
        pass: None,
    }];
    let mut stack_points = stack[0].points();
    while let Some(frame) = stack.last_mut() {
        control.checkpoint()?;
        work.consume(row.len().div_ceil(8))?;
        if complete(row, possible) {
            break;
        }
        let Some(&index) = edges[frame.cluster].get(frame.next) else {
            let frame = stack.pop().unwrap();
            on_path[frame.cluster] = false;
            stack_points -= frame.points();
            continue;
        };
        frame.next += 1;
        work.consume(1)?;
        let target = &portals[index];
        if !bit(eligible, index) || on_path[target.to] {
            continue;
        }
        let Some(mut pass) = clip(target.winding, base.plane, work)? else {
            continue;
        };
        let back = Plane {
            normal: -target.plane.normal,
            dist: -target.plane.dist,
        };
        let Some(source) = clip(&frame.source, back, work)? else {
            continue;
        };
        if let Some(previous) = &frame.pass {
            let Some(clipped) = separators(&source, previous, pass, false, work, control)? else {
                continue;
            };
            pass = clipped;
            let Some(clipped) = separators(previous, &source, pass, true, work, control)? else {
                continue;
            };
            pass = clipped;
        }
        set(row, target.to);
        work.consume(row.len().div_ceil(8))?;
        if complete_intersection(row, possible, &bounds[index]) {
            continue;
        }
        let frame = Frame {
            cluster: target.to,
            next: 0,
            source: Arc::new(source),
            pass: Some(Arc::new(pass)),
        };
        let points = frame.points();
        if stack_points + points > MAX_STACK_POINTS {
            return Err("Visibility portal stack exceeds its 262,144-point allocation budget; use Fast visibility or simplify structural geometry".into());
        }
        stack_points += points;
        on_path[target.to] = true;
        stack.push(frame);
    }
    Ok(())
}

fn complete(row: &[u8], possible: &[u8]) -> bool {
    row.iter()
        .zip(possible)
        .all(|(visible, candidates)| visible & candidates == *candidates)
}

fn complete_intersection(row: &[u8], source: &[u8], target: &[u8]) -> bool {
    row.iter()
        .zip(source)
        .zip(target)
        .all(|((visible, source), target)| visible & (source & target) == source & target)
}

fn clip(winding: &Winding, plane: Plane, work: &mut Work) -> Result<Option<Winding>, String> {
    validate(winding)?;
    work.consume(winding.0.len())?;
    let distances = winding
        .0
        .iter()
        .map(|point| plane.distance_to(*point) + VISIBILITY_EPSILON)
        .collect::<Vec<_>>();
    if distances.iter().any(|distance| !distance.is_finite()) {
        return Err("Visibility aperture clipping produced a nonfinite distance".into());
    }
    if distances.iter().all(|distance| *distance >= 0.0) {
        return Ok(Some(winding.clone()));
    }
    if distances.iter().all(|distance| *distance < 0.0) {
        return Ok(None);
    }
    let mut points = Vec::with_capacity(winding.0.len() + 2);
    for index in 0..winding.0.len() {
        let next = (index + 1) % winding.0.len();
        let [a, b] = [winding.0[index], winding.0[next]];
        let [da, db] = [distances[index], distances[next]];
        if da >= 0.0 {
            points.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            points.push(a + (b - a) * (da / (da - db)));
        }
    }
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 {
        return Ok(None);
    }
    let clipped = Winding(points);
    validate(&clipped)?;
    Ok(Some(clipped))
}

pub(super) fn validate(winding: &Winding) -> Result<(), String> {
    if winding.0.len() < 3 || winding.0.iter().any(|point| !point.is_finite()) {
        return Err("Visibility aperture requires at least three finite points".into());
    }
    if winding.0.len() > MAX_STACK_POINTS {
        return Err("Visibility aperture exceeds its 262,144-point allocation budget".into());
    }
    Ok(())
}

fn separators(
    source: &Winding,
    pass: &Winding,
    mut target: Winding,
    flip: bool,
    work: &mut Work,
    control: &Control,
) -> Result<Option<Winding>, String> {
    validate(source)?;
    validate(pass)?;
    validate(&target)?;
    for index in 0..source.0.len() {
        let start = source.0[index];
        let edge = source.0[(index + 1) % source.0.len()] - start;
        for &point in &pass.0 {
            control.checkpoint()?;
            work.consume(source.0.len() + pass.0.len())?;
            let normal = edge.cross(point - start);
            if !normal.is_finite() || !normal.length_squared().is_finite() {
                return Err("Visibility separator produced a nonfinite normal".into());
            }
            let Some(normal) = normal.try_normalize() else {
                continue;
            };
            let mut plane = Plane {
                normal,
                dist: normal.dot(point),
            };
            let (low, high) = distances(source, plane);
            if low < -VISIBILITY_EPSILON && high > VISIBILITY_EPSILON {
                continue;
            }
            if low >= -VISIBILITY_EPSILON && high <= VISIBILITY_EPSILON {
                continue;
            }
            if high > VISIBILITY_EPSILON {
                plane = Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                };
            }
            let (low, high) = distances(pass, plane);
            if low < -VISIBILITY_EPSILON || high <= VISIBILITY_EPSILON {
                continue;
            }
            if flip {
                plane = Plane {
                    normal: -plane.normal,
                    dist: -plane.dist,
                };
            }
            let Some(clipped) = clip(&target, plane, work)? else {
                return Ok(None);
            };
            target = clipped;
        }
    }
    Ok(Some(target))
}

fn distances(winding: &Winding, plane: Plane) -> (f64, f64) {
    winding
        .0
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| {
            let distance = plane.distance_to(*point);
            (low.min(distance), high.max(distance))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn clipping_keeps_tiny_apertures_and_cuts_polygons_beyond_128_points() {
        for count in [128, 256] {
            let winding = Winding(
                (0..count)
                    .map(|index| {
                        let angle = index as f64 * std::f64::consts::TAU / count as f64;
                        DVec3::new(angle.cos(), angle.sin(), 0.0)
                    })
                    .collect(),
            );
            let mut work = Work::Local(10_000);
            let clipped = clip(
                &winding,
                Plane {
                    normal: DVec3::X,
                    dist: -0.8999,
                },
                &mut work,
            )
            .unwrap()
            .unwrap();
            assert_eq!(clipped.0.len(), count + 1);
            assert_ne!(clipped, winding);
            assert!(clipped.0.iter().all(|point| point.x >= -0.9999 - 1e-12));
            assert!(
                clipped
                    .0
                    .iter()
                    .any(|point| (point.x + 0.9999).abs() < 1e-12)
            );
        }
        let mut work = Work::Local(10_000);
        let tiny = Winding(vec![
            DVec3::ZERO,
            DVec3::new(0.00001, 0.0, 0.0),
            DVec3::new(0.0, 0.00001, 0.0),
        ]);
        assert_eq!(
            clip(
                &tiny,
                Plane {
                    normal: DVec3::X,
                    dist: 0.0
                },
                &mut work,
            )
            .unwrap()
            .unwrap(),
            tiny
        );
        let mut exhausted = Work::Local(0);
        let touching = Winding(vec![
            DVec3::ZERO,
            DVec3::new(-1.0, -1.0, 0.0),
            DVec3::new(-1.0, 1.0, 0.0),
        ]);
        assert!(
            clip(
                &touching,
                Plane {
                    normal: DVec3::X,
                    dist: VISIBILITY_EPSILON
                },
                &mut work
            )
            .unwrap()
            .is_none()
        );
        assert!(
            clip(
                &tiny,
                Plane {
                    normal: DVec3::X,
                    dist: 0.0
                },
                &mut exhausted,
            )
            .is_err()
        );
    }

    #[test]
    fn large_separators_reject_occluded_apertures_and_invalid_data_errors() {
        let circle = |x| {
            Winding(
                (0..256)
                    .map(|index| {
                        let angle = index as f64 * std::f64::consts::TAU / 256.0;
                        DVec3::new(x, angle.cos(), angle.sin())
                    })
                    .collect(),
            )
        };
        let target = Winding(vec![
            DVec3::new(2.0, 9.0, -1.0),
            DVec3::new(2.0, 11.0, -1.0),
            DVec3::new(2.0, 11.0, 1.0),
            DVec3::new(2.0, 9.0, 1.0),
        ]);
        let mut work = Work::Local(MAX_STACK_POINTS * 1024);
        assert!(
            separators(
                &circle(0.0),
                &circle(1.0),
                target.clone(),
                false,
                &mut work,
                &Control::default()
            )
            .unwrap()
            .is_none()
        );
        let plane = Plane {
            normal: DVec3::X,
            dist: 0.0,
        };
        for winding in [
            Winding(vec![]),
            Winding(vec![DVec3::NAN; 3]),
            Winding(vec![DVec3::ZERO; MAX_STACK_POINTS + 1]),
        ] {
            assert!(clip(&winding, plane, &mut work).is_err());
        }
        assert!(
            clip(
                &target,
                Plane {
                    normal: DVec3::NAN,
                    dist: 0.0
                },
                &mut work
            )
            .is_err()
        );
    }
}
