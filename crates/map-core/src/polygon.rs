use glam::DVec2;

use crate::EditError;

pub const MAX_CORNERS: usize = 64;
const EPSILON: f64 = 1e-8;

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn turn(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

fn on_segment(a: DVec2, b: DVec2, point: DVec2) -> bool {
    turn(a, b, point).abs() <= EPSILON
        && point.cmpge(a.min(b) - DVec2::splat(EPSILON)).all()
        && point.cmple(a.max(b) + DVec2::splat(EPSILON)).all()
}

fn intersects(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let ab = [turn(a, b, c), turn(a, b, d)];
    let cd = [turn(c, d, a), turn(c, d, b)];
    (ab[0] * ab[1] < 0.0 && cd[0] * cd[1] < 0.0)
        || on_segment(a, b, c)
        || on_segment(a, b, d)
        || on_segment(c, d, a)
        || on_segment(c, d, b)
}

pub fn parts(corners: &[DVec2]) -> Result<Vec<Vec<DVec2>>, EditError> {
    if !(3..=MAX_CORNERS).contains(&corners.len()) {
        return Err(invalid("Draw a polygon with 3–64 corners"));
    }
    if corners
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > 65536.0)
    {
        return Err(invalid(
            "Polygon corners must be finite and within ±65536 units",
        ));
    }
    let count = corners.len();
    for i in 0..count {
        if corners[i + 1..]
            .iter()
            .any(|point| corners[i].distance_squared(*point) < 1e-8)
        {
            return Err(invalid(
                "Polygon corners must be distinct; the closing corner is implicit",
            ));
        }
        if turn(
            corners[i],
            corners[(i + 1) % count],
            corners[(i + 2) % count],
        )
        .abs()
            <= EPSILON
        {
            return Err(invalid("Remove collinear corners from the polygon outline"));
        }
        for j in i + 1..count {
            if j == i + 1 || (i == 0 && j == count - 1) {
                continue;
            }
            if intersects(
                corners[i],
                corners[(i + 1) % count],
                corners[j],
                corners[(j + 1) % count],
            ) {
                return Err(invalid(
                    "Polygon edges must not cross or touch another edge",
                ));
            }
        }
    }
    let origin = corners[0];
    let area = (0..count)
        .map(|i| (corners[i] - origin).perp_dot(corners[(i + 1) % count] - origin))
        .sum::<f64>();
    if area.abs() <= EPSILON {
        return Err(invalid("Polygon must enclose a nonzero area"));
    }
    let mut remaining = corners.to_vec();
    if area < 0.0 {
        remaining.reverse();
    }
    if (0..count).all(|i| {
        turn(
            remaining[i],
            remaining[(i + 1) % count],
            remaining[(i + 2) % count],
        ) > EPSILON
    }) {
        return Ok(vec![remaining]);
    }
    let mut parts = Vec::with_capacity(count - 2);
    while remaining.len() > 3 {
        let size = remaining.len();
        let ear = (0..size)
            .find(|&i| {
                let a = remaining[(i + size - 1) % size];
                let b = remaining[i];
                let c = remaining[(i + 1) % size];
                turn(a, b, c) > EPSILON
                    && (0..size)
                        .filter(|&j| j != i && j != (i + size - 1) % size && j != (i + 1) % size)
                        .all(|j| {
                            let p = remaining[j];
                            turn(a, b, p) < -EPSILON
                                || turn(b, c, p) < -EPSILON
                                || turn(c, a, p) < -EPSILON
                        })
            })
            .ok_or_else(|| invalid("Polygon is too narrow or ambiguous to decompose safely"))?;
        parts.push(vec![
            remaining[(ear + size - 1) % size],
            remaining[ear],
            remaining[(ear + 1) % size],
        ]);
        remaining.remove(ear);
    }
    if turn(remaining[0], remaining[1], remaining[2]) <= EPSILON {
        return Err(invalid(
            "Polygon decomposition produced a collapsed triangle",
        ));
    }
    parts.push(remaining);
    Ok(parts)
}
