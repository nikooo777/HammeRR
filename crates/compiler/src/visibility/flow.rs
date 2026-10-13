use std::sync::Arc;

use super::{Control, DirectedPortal, Plane, VISIBILITY_EPSILON, Winding, Work, set};

const MAX_APERTURE_POINTS: usize = 128;
const MAX_STACK_POINTS: usize = 262_144;

struct Frame {
    cluster: usize,
    next: usize,
    source: Arc<Winding>,
    pass: Option<Arc<Winding>>,
}

pub(super) struct Context<'a> {
    pub work: &'a mut Work,
    pub fallbacks: &'a mut usize,
    pub control: &'a Control,
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
    eligible: &[bool],
    row: &mut [u8],
    context: Context,
) -> Result<(), String> {
    let Context {
        work,
        fallbacks,
        control,
    } = context;
    let mut on_path = vec![false; edges.len()];
    on_path[base.from] = true;
    on_path[base.to] = true;
    set(row, base.to);
    let mut stack = vec![Frame {
        cluster: base.to,
        next: 0,
        source: Arc::new(base.winding.clone()),
        pass: None,
    }];
    let mut stack_points = stack[0].points();
    while let Some(frame) = stack.last_mut() {
        control.checkpoint()?;
        let Some(&index) = edges[frame.cluster].get(frame.next) else {
            let frame = stack.pop().unwrap();
            on_path[frame.cluster] = false;
            stack_points -= frame.points();
            continue;
        };
        frame.next += 1;
        work.consume(1)?;
        let target = &portals[index];
        if !eligible[index] || on_path[target.to] {
            continue;
        }
        let Some(mut pass) = clip(target.winding, base.plane, work, fallbacks)? else {
            continue;
        };
        let back = Plane {
            normal: -target.plane.normal,
            dist: -target.plane.dist,
        };
        let Some(source) = clip(&frame.source, back, work, fallbacks)? else {
            continue;
        };
        if let Some(previous) = &frame.pass {
            let Some(clipped) =
                separators(&source, previous, pass, false, work, fallbacks, control)?
            else {
                continue;
            };
            pass = clipped;
            let Some(clipped) =
                separators(previous, &source, pass, true, work, fallbacks, control)?
            else {
                continue;
            };
            pass = clipped;
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
        set(row, target.to);
        on_path[target.to] = true;
        stack.push(frame);
    }
    Ok(())
}

fn clip(
    winding: &Winding,
    plane: Plane,
    work: &mut Work,
    fallbacks: &mut usize,
) -> Result<Option<Winding>, String> {
    work.consume(winding.0.len())?;
    let distances = winding
        .0
        .iter()
        .map(|point| plane.distance_to(*point) + VISIBILITY_EPSILON)
        .collect::<Vec<_>>();
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
        if points.len() > MAX_APERTURE_POINTS {
            *fallbacks += 1;
            return Ok(Some(winding.clone()));
        }
    }
    if points.len() < 3 || points.iter().any(|point| !point.is_finite()) {
        *fallbacks += 1;
        return Ok(Some(winding.clone()));
    }
    Ok(Some(Winding(points)))
}

fn separators(
    source: &Winding,
    pass: &Winding,
    mut target: Winding,
    flip: bool,
    work: &mut Work,
    fallbacks: &mut usize,
    control: &Control,
) -> Result<Option<Winding>, String> {
    if source.0.len() > MAX_APERTURE_POINTS || pass.0.len() > MAX_APERTURE_POINTS {
        *fallbacks += 1;
        return Ok(Some(target));
    }
    for index in 0..source.0.len() {
        let start = source.0[index];
        let edge = source.0[(index + 1) % source.0.len()] - start;
        for &point in &pass.0 {
            control.checkpoint()?;
            work.consume(source.0.len() + pass.0.len())?;
            let normal = edge.cross(point - start);
            if normal.length_squared() < VISIBILITY_EPSILON {
                continue;
            }
            let normal = normal.normalize();
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
            let Some(clipped) = clip(&target, plane, work, fallbacks)? else {
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
    fn bounded_clip_preserves_tiny_apertures_and_falls_back_on_excess_points() {
        let winding = Winding(
            (0..MAX_APERTURE_POINTS)
                .map(|index| {
                    let angle = index as f64 * std::f64::consts::TAU / MAX_APERTURE_POINTS as f64;
                    DVec3::new(angle.cos(), angle.sin(), 0.0)
                })
                .collect(),
        );
        let mut work = Work { remaining: 10_000 };
        let mut fallbacks = 0;
        let clipped = clip(
            &winding,
            Plane {
                normal: DVec3::X,
                dist: -0.8999,
            },
            &mut work,
            &mut fallbacks,
        )
        .unwrap()
        .unwrap();
        assert_eq!(clipped, winding);
        assert_eq!(fallbacks, 1);
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
                &mut fallbacks
            )
            .unwrap()
            .unwrap(),
            tiny
        );
        let mut exhausted = Work { remaining: 0 };
        assert!(
            clip(
                &winding,
                Plane {
                    normal: DVec3::X,
                    dist: 0.0
                },
                &mut exhausted,
                &mut fallbacks
            )
            .is_err()
        );
    }
}
