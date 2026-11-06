use glam::DVec3;

use super::{FIELD_WORK, invalid, orient};
use crate::EditError;

pub(super) const MAX_VERTICES: usize = 64;
pub(super) const ON_SEGMENT_WORK: usize = FIELD_WORK + 4;
pub(super) const SEGMENT_WORK: usize = 4 * FIELD_WORK + 16;
pub(super) const CLASSIFY_WORK: usize = FIELD_WORK + 8;
pub(super) const CONVEX_WORK: usize = FIELD_WORK;
pub(super) const SIMPLE_WORK: usize = SEGMENT_WORK + 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SegmentContact {
    None,
    Crossing,
    Point(DVec3),
    Overlap([DVec3; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Location {
    Outside,
    Boundary,
    Inside,
}

pub(super) fn same_point(first: DVec3, second: DVec3, axes: [usize; 2]) -> bool {
    axes.into_iter().all(|axis| first[axis] == second[axis])
}

fn in_bounds(point: DVec3, [a, b]: [DVec3; 2], axes: [usize; 2]) -> bool {
    axes.into_iter()
        .all(|axis| point[axis] >= a[axis].min(b[axis]) && point[axis] <= a[axis].max(b[axis]))
}

pub(super) fn on_segment(
    point: DVec3,
    [a, b]: [DVec3; 2],
    axes: [usize; 2],
) -> Result<bool, EditError> {
    Ok(orient(a, b, point, axes)? == 0 && in_bounds(point, [a, b], axes))
}

pub(super) fn segment_contact(
    [a, b]: [DVec3; 2],
    [c, d]: [DVec3; 2],
    axes: [usize; 2],
) -> Result<SegmentContact, EditError> {
    let ab = [orient(a, b, c, axes)?, orient(a, b, d, axes)?];
    let cd = [orient(c, d, a, axes)?, orient(c, d, b, axes)?];
    if ab[0] * ab[1] < 0 && cd[0] * cd[1] < 0 {
        return Ok(SegmentContact::Crossing);
    }
    if ab == [0, 0] && cd == [0, 0] {
        let coordinate = axes
            .into_iter()
            .find(|&axis| a[axis] != b[axis] || c[axis] != d[axis]);
        let Some(coordinate) = coordinate else {
            return Ok(if same_point(a, c, axes) {
                SegmentContact::Point(a)
            } else {
                SegmentContact::None
            });
        };
        let first = if a[coordinate] <= b[coordinate] {
            [a, b]
        } else {
            [b, a]
        };
        let second = if c[coordinate] <= d[coordinate] {
            [c, d]
        } else {
            [d, c]
        };
        let low = if first[0][coordinate] >= second[0][coordinate] {
            first[0]
        } else {
            second[0]
        };
        let high = if first[1][coordinate] <= second[1][coordinate] {
            first[1]
        } else {
            second[1]
        };
        return Ok(if low[coordinate] > high[coordinate] {
            SegmentContact::None
        } else if low[coordinate] == high[coordinate] {
            SegmentContact::Point(low)
        } else {
            SegmentContact::Overlap([low, high])
        });
    }
    for (side, point, segment) in [
        (ab[0], c, [a, b]),
        (ab[1], d, [a, b]),
        (cd[0], a, [c, d]),
        (cd[1], b, [c, d]),
    ] {
        if side == 0 && in_bounds(point, segment, axes) {
            return Ok(SegmentContact::Point(point));
        }
    }
    Ok(SegmentContact::None)
}

fn bounded(ring: &[DVec3]) -> Result<(), EditError> {
    if !(3..=MAX_VERTICES).contains(&ring.len()) {
        return Err(invalid(
            "Terrain projected polygons require 3 to 64 vertices",
        ));
    }
    Ok(())
}

pub(super) fn simple(ring: &[DVec3], axes: [usize; 2]) -> Result<(), EditError> {
    bounded(ring)?;
    for (first, &a) in ring.iter().enumerate() {
        let next = (first + 1) % ring.len();
        let b = ring[next];
        if same_point(a, b, axes) {
            return Err(invalid(
                "Terrain projected boundary contains a collapsed edge",
            ));
        }
        if ring
            .iter()
            .skip(first + 1)
            .any(|&point| same_point(a, point, axes))
        {
            return Err(invalid(
                "Terrain projected boundary vertices must be unique",
            ));
        }
        for second in first + 1..ring.len() {
            let other_next = (second + 1) % ring.len();
            let contact = segment_contact([a, b], [ring[second], ring[other_next]], axes)?;
            let common = if next == second {
                Some(b)
            } else if other_next == first {
                Some(a)
            } else {
                None
            };
            let allowed = match (common, contact) {
                (Some(expected), SegmentContact::Point(point)) => same_point(expected, point, axes),
                (None, SegmentContact::None) => true,
                _ => false,
            };
            if !allowed {
                return Err(invalid(
                    "Original curved terrain projected boundary must be simple without crossings, touches or backtracking",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn classify(
    point: DVec3,
    ring: &[DVec3],
    axes: [usize; 2],
) -> Result<Location, EditError> {
    bounded(ring)?;
    let mut winding = 0;
    for (index, &a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        let side = orient(a, b, point, axes)?;
        if side == 0 && in_bounds(point, [a, b], axes) {
            return Ok(Location::Boundary);
        }
        if a[axes[1]] <= point[axes[1]] {
            if b[axes[1]] > point[axes[1]] && side > 0 {
                winding += 1;
            }
        } else if b[axes[1]] <= point[axes[1]] && side < 0 {
            winding -= 1;
        }
    }
    Ok(if winding == 0 {
        Location::Outside
    } else {
        Location::Inside
    })
}

pub(super) fn convex(ring: &[DVec3], axes: [usize; 2], sign: i128) -> Result<bool, EditError> {
    bounded(ring)?;
    for (index, &a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        for &point in ring {
            let side = orient(a, b, point, axes)?;
            if side != 0 && side != sign {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
