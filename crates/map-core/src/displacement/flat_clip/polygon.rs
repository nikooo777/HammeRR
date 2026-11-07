use glam::DVec3;

use super::{FIELD_WORK, orient};
use crate::EditError;

pub(super) const ON_SEGMENT_WORK: usize = FIELD_WORK + 4;
pub(super) const SEGMENT_WORK: usize = 4 * FIELD_WORK + 16;

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
