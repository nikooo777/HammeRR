use super::*;

fn turn(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

fn on_segment(a: DVec2, b: DVec2, point: DVec2) -> bool {
    let length = a.distance(b);
    let tolerance = roundoff_tolerance(a.abs().max(b.abs()).max(point.abs()).max_element());
    turn(a, b, point).abs() <= tolerance * length
        && (point - a).dot(b - a) >= -tolerance * length
        && (point - b).dot(a - b) >= -tolerance * length
}

fn key(mut vertices: [u32; 2]) -> [u32; 2] {
    vertices.sort_unstable();
    vertices
}

fn add_point(points: &mut Vec<DVec2>, point: DVec2, generated: bool) -> Result<usize, EditError> {
    if generated
        && let Some(index) = points.iter().position(|candidate| {
            candidate.distance(point)
                <= roundoff_tolerance(candidate.abs().max(point.abs()).max_element())
        })
    {
        return Ok(index);
    }
    if points.len() == MAX_FREE_PARTITION_VERTICES {
        return Err(invalid(
            "Free cuts exceed 64 vertices including source corners and crossings",
        ));
    }
    if points
        .iter()
        .any(|candidate| candidate.distance(point) <= MIN_EDGE)
    {
        return Err(invalid(
            "Free-cut vertices must be separated by more than 0.002 units",
        ));
    }
    points.push(point);
    Ok(points.len() - 1)
}

fn canonical_endpoint(
    point: DVec3,
    corners: &[DVec3],
    outline: &[DVec2],
    frame: &Frame,
) -> Result<DVec2, EditError> {
    if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE {
        return Err(invalid(
            "Free-cut endpoints must be finite and within ±65536 units",
        ));
    }
    let coordinates = frame.coordinates(point);
    let reconstructed =
        corners[0] * coordinates[0] + corners[1] * coordinates[1] + corners[2] * coordinates[2];
    let tolerance = roundoff_tolerance(point.abs().max(reconstructed.abs()).max_element());
    if reconstructed.distance(point) > tolerance {
        return Err(invalid("Free-cut endpoints must lie on the source face"));
    }
    let mut projected = frame.project(reconstructed);
    let orientation = polygon_area(outline).signum();
    for index in 0..outline.len() {
        let a = outline[index];
        let b = outline[(index + 1) % outline.len()];
        let edge = b - a;
        let distance = orientation * turn(a, b, projected) / edge.length();
        if distance < -tolerance {
            return Err(invalid(
                "Free-cut endpoints must lie inside the source face boundary",
            ));
        }
        if distance.abs() <= tolerance {
            let parameter = ((projected - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
            projected = a.lerp(b, parameter);
        }
    }
    Ok(projected)
}

fn polygon_area(points: &[DVec2]) -> f64 {
    let origin = points[0];
    (1..points.len() - 1)
        .map(|index| turn(origin, points[index], points[index + 1]))
        .sum::<f64>()
        * 0.5
}

fn insert_intersections(points: &mut Vec<DVec2>, segments: &[[u32; 2]]) -> Result<(), EditError> {
    for (index, segment) in segments.iter().enumerate() {
        let [a, b] = segment.map(|vertex| points[vertex as usize]);
        for other in &segments[index + 1..] {
            let [c, d] = other.map(|vertex| points[vertex as usize]);
            let ab = b - a;
            let cd = d - c;
            let denominator = ab.perp_dot(cd);
            if denominator.abs() <= f64::EPSILON * ab.length() * cd.length() * 32.0 {
                continue;
            }
            let parameter = (c - a).perp_dot(cd) / denominator;
            let other_parameter = (c - a).perp_dot(ab) / denominator;
            if (0.0..=1.0).contains(&parameter) && (0.0..=1.0).contains(&other_parameter) {
                let point = a.lerp(b, parameter);
                if !on_segment(c, d, point) {
                    return Err(invalid("Free-cut intersection is numerically ambiguous"));
                }
                add_point(points, point, true)?;
            }
        }
    }
    Ok(())
}

fn split_segment(points: &[DVec2], vertices: [u32; 2]) -> Vec<[u32; 2]> {
    let [a, b] = vertices.map(|vertex| points[vertex as usize]);
    let direction = b - a;
    let mut along = points
        .iter()
        .enumerate()
        .filter(|(_, point)| on_segment(a, b, **point))
        .map(|(index, point)| ((point - a).dot(direction), index as u32))
        .collect::<Vec<_>>();
    along.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    along
        .windows(2)
        .map(|pair| [pair[0].1, pair[1].1])
        .collect()
}

fn crosses(points: &[DVec2], first: [u32; 2], second: [u32; 2]) -> bool {
    if first.iter().any(|vertex| second.contains(vertex)) {
        return false;
    }
    let [a, b] = first.map(|vertex| points[vertex as usize]);
    let [c, d] = second.map(|vertex| points[vertex as usize]);
    let ab = [turn(a, b, c), turn(a, b, d)];
    let cd = [turn(c, d, a), turn(c, d, b)];
    (ab[0] * ab[1] < 0.0 && cd[0] * cd[1] < 0.0)
        || on_segment(a, b, c)
        || on_segment(a, b, d)
        || on_segment(c, d, a)
        || on_segment(c, d, b)
}

fn triangle_contains(points: &[DVec2], vertices: [u32; 3], point: DVec2) -> bool {
    (0..3).all(|index| {
        let a = points[vertices[index] as usize];
        let b = points[vertices[(index + 1) % 3] as usize];
        let tolerance = roundoff_tolerance(a.abs().max(b.abs()).max(point.abs()).max_element());
        turn(a, b, point) >= -tolerance * a.distance(b)
    })
}

fn triangulate(
    points: &[DVec2],
    edges: &mut BTreeMap<[u32; 2], FreePartitionEdge>,
) -> Result<Vec<FreePartitionPiece>, EditError> {
    for a in 0..points.len() {
        for b in a + 1..points.len() {
            let vertices = [a as u32, b as u32];
            if edges.contains_key(&vertices)
                || points.iter().enumerate().any(|(index, point)| {
                    index != a && index != b && on_segment(points[a], points[b], *point)
                })
                || edges.keys().any(|edge| crosses(points, vertices, *edge))
            {
                continue;
            }
            edges.insert(
                vertices,
                FreePartitionEdge {
                    vertices,
                    authored: false,
                    boundary: false,
                },
            );
        }
    }
    let mut pieces = Vec::new();
    for a in 0..points.len() {
        for b in a + 1..points.len() {
            if !edges.contains_key(&[a as u32, b as u32]) {
                continue;
            }
            for c in b + 1..points.len() {
                if !edges.contains_key(&[a as u32, c as u32])
                    || !edges.contains_key(&[b as u32, c as u32])
                {
                    continue;
                }
                let area = turn(points[a], points[b], points[c]);
                if area.abs() <= MIN_EDGE * MIN_EDGE {
                    continue;
                }
                let vertices = if area > 0.0 {
                    [a as u32, b as u32, c as u32]
                } else {
                    [a as u32, c as u32, b as u32]
                };
                if points.iter().enumerate().any(|(index, point)| {
                    ![a, b, c].contains(&index) && triangle_contains(points, vertices, *point)
                }) {
                    continue;
                }
                if pieces.len() == MAX_FREE_PARTITION_PIECES {
                    return Err(invalid("Free cuts exceed 120 triangular face pieces"));
                }
                pieces.push(FreePartitionPiece {
                    id: pieces.len() as u32,
                    vertices,
                });
            }
        }
    }
    Ok(pieces)
}

fn certify(
    points: &[DVec2],
    outline: &[DVec2],
    edges: &BTreeMap<[u32; 2], FreePartitionEdge>,
    pieces: &[FreePartitionPiece],
) -> Result<(), EditError> {
    let mut uses = BTreeMap::<[u32; 2], Vec<[u32; 2]>>::new();
    let mut area = 0.0;
    for piece in pieces {
        let vertices = piece.vertices;
        let triangle = vertices.map(|vertex| points[vertex as usize]);
        let size = turn(triangle[0], triangle[1], triangle[2]) * 0.5;
        if !size.is_finite() || size <= MIN_EDGE * MIN_EDGE * 0.5 {
            return Err(invalid(
                "Free cuts produce a collapsed or reversed triangle",
            ));
        }
        area += size;
        for index in 0..3 {
            let oriented = [vertices[index], vertices[(index + 1) % 3]];
            uses.entry(key(oriented)).or_default().push(oriented);
        }
    }
    let boundary_count = edges.values().filter(|edge| edge.boundary).count();
    if pieces.len() != 2 * points.len() - boundary_count - 2
        || edges.len() != uses.len()
        || edges.iter().any(|(key, edge)| {
            let Some(uses) = uses.get(key) else {
                return true;
            };
            if edge.boundary {
                uses.len() != 1
            } else {
                uses.len() != 2 || uses[0] != [uses[1][1], uses[1][0]]
            }
        })
        || (area - polygon_area(outline).abs()).abs() > area * 1e-10 + EPSILON
    {
        return Err(invalid(
            "Free cuts do not form an exact, nonoverlapping, oriented source-face cover",
        ));
    }
    Ok(())
}

pub(super) fn build(
    request: &FreePartitionCut,
    owner: ObjectId,
    plane: [DVec3; 3],
    corners: Vec<Vec<u32>>,
    positions: Vec<DVec3>,
    frame: &Frame,
) -> Result<FreePartition, EditError> {
    if request.edges.is_empty()
        || request.edges.len() > MAX_FREE_PARTITION_EDGES
        || request.vertices.len() > MAX_FREE_PARTITION_VERTICES - positions.len()
    {
        return Err(invalid(
            "Draw 1–32 free-cut segments with at most 64 total source and endpoint vertices",
        ));
    }
    let outline = positions
        .iter()
        .map(|point| frame.project(*point))
        .collect::<Vec<_>>();
    let mut points = outline.clone();
    let mut mapping = Vec::with_capacity(request.vertices.len());
    for (index, point) in request.vertices.iter().enumerate() {
        if let Some(corner) = positions.iter().position(|corner| corner == point) {
            mapping.push(corner as u32);
        } else if let Some(previous) = request.vertices[..index]
            .iter()
            .position(|other| other == point)
        {
            mapping.push(mapping[previous]);
        } else {
            let point = canonical_endpoint(*point, &positions, &outline, frame)?;
            mapping.push(add_point(&mut points, point, false)? as u32);
        }
    }
    let mut used = BTreeSet::new();
    let mut segments = BTreeSet::new();
    for edge in &request.edges {
        if edge.iter().any(|vertex| *vertex as usize >= mapping.len()) || edge[0] == edge[1] {
            return Err(invalid(
                "Free-cut segments need two distinct existing endpoint indices",
            ));
        }
        used.extend(edge.iter().copied());
        let vertices = edge.map(|vertex| mapping[vertex as usize]);
        if vertices[0] == vertices[1] {
            return Err(invalid("A free-cut segment is collapsed"));
        }
        segments.insert(key(vertices));
    }
    if used.len() != request.vertices.len() {
        return Err(invalid("Every free-cut endpoint must belong to a segment"));
    }
    let segments = segments.into_iter().collect::<Vec<_>>();
    insert_intersections(&mut points, &segments)?;
    let mut edges = BTreeMap::new();
    for index in 0..outline.len() {
        for vertices in split_segment(
            &points,
            [index as u32, ((index + 1) % outline.len()) as u32],
        ) {
            let vertices = key(vertices);
            edges.insert(
                vertices,
                FreePartitionEdge {
                    vertices,
                    authored: false,
                    boundary: true,
                },
            );
        }
    }
    for segment in segments {
        for vertices in split_segment(&points, segment) {
            let vertices = key(vertices);
            edges
                .entry(vertices)
                .and_modify(|edge| edge.authored = true)
                .or_insert(FreePartitionEdge {
                    vertices,
                    authored: true,
                    boundary: false,
                });
        }
    }
    let constraints = edges.keys().copied().collect::<Vec<_>>();
    for (index, edge) in constraints.iter().enumerate() {
        if constraints[index + 1..]
            .iter()
            .any(|other| crosses(&points, *edge, *other))
        {
            return Err(invalid(
                "Free-cut intersections cannot be represented without ambiguous crossings",
            ));
        }
    }
    let pieces = triangulate(&points, &mut edges)?;
    certify(&points, &outline, &edges, &pieces)?;
    let vertices = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let position = if index < positions.len() {
                positions[index]
            } else {
                frame.origin + frame.u * point.x + frame.v * point.y
            };
            FreePartitionVertex {
                id: index as u32,
                position,
                coordinates: frame.coordinates(position),
            }
        })
        .collect::<Vec<_>>();
    let mut cut = request.clone();
    cut.corners.clone_from(&corners);
    let max_endpoint_correction = request
        .vertices
        .iter()
        .zip(&mapping)
        .map(|(point, vertex)| point.distance(vertices[*vertex as usize].position))
        .fold(0.0, f64::max);
    let numeric_tolerance = roundoff_tolerance(
        request
            .vertices
            .iter()
            .chain(&positions)
            .map(|point| point.abs().max_element())
            .fold(1.0, f64::max),
    );
    if max_endpoint_correction > numeric_tolerance {
        return Err(invalid(
            "Free-cut endpoint reconstruction exceeds floating-point roundoff",
        ));
    }
    Ok(FreePartition {
        solid: request.solid,
        side: request.side,
        owner,
        plane,
        corners,
        vertices,
        edges: edges.into_values().collect(),
        pieces,
        endpoint_vertices: mapping,
        max_endpoint_correction,
        numeric_tolerance,
        cut,
    })
}

pub(super) fn face_triangles(
    points: &[DVec3],
    ring: &[usize],
    normal: DVec3,
) -> Result<Vec<[usize; 3]>, EditError> {
    if !(3..=MAX_FREE_PARTITION_VERTICES).contains(&ring.len()) {
        return Err(invalid(
            "Free-cut materialization requires at most 64 vertices per exterior face",
        ));
    }
    let origin = points[ring[0]];
    let u = (points[ring[1]] - origin).normalize();
    let v = u.cross(normal);
    let projected = ring
        .iter()
        .map(|index| {
            let point = points[*index] - origin;
            DVec2::new(point.dot(u), point.dot(v))
        })
        .collect::<Vec<_>>();
    let mut edges = BTreeMap::new();
    for index in 0..ring.len() {
        let vertices = key([index as u32, ((index + 1) % ring.len()) as u32]);
        edges.insert(
            vertices,
            FreePartitionEdge {
                vertices,
                authored: false,
                boundary: true,
            },
        );
    }
    let pieces = triangulate(&projected, &mut edges)?;
    certify(&projected, &projected, &edges, &pieces)?;
    Ok(pieces
        .into_iter()
        .map(|piece| piece.vertices.map(|index| ring[index as usize]))
        .collect())
}
