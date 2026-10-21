use super::*;
use crate::physics::native::GEOMETRY_EPSILON;

fn minimum_distances(points: &[DVec3], triangles: &[[DVec3; 3]]) -> Vec<f64> {
    let mut nearest = vec![f64::INFINITY; points.len()];
    for &triangle in triangles {
        let triangle = Triangle::new(triangle).unwrap();
        for (&point, nearest) in points.iter().zip(&mut nearest) {
            *nearest = nearest.min(triangle.distance_squared(point).unwrap());
        }
    }
    nearest
}

#[test]
fn bounded_triangle_distance_handles_face_edge_vertex_and_reversed_order() {
    let corners = [
        DVec3::ZERO,
        DVec3::new(4.0, 0.0, 0.0),
        DVec3::new(0.0, 3.0, 0.0),
    ];
    for corners in [corners, [corners[2], corners[1], corners[0]]] {
        let triangle = Triangle::new(corners).unwrap();
        for (point, expected) in [
            (DVec3::new(1.0, 1.0, 2.0), 4.0),
            (DVec3::new(2.0, -1.0, 0.0), 1.0),
            (DVec3::new(-2.0, -3.0, 6.0), 49.0),
            (DVec3::new(4.0, 3.0, 2.0), 9.76),
        ] {
            assert!((triangle.distance_squared(point).unwrap() - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn original_149_skinny_face_keeps_zero_distance_without_plane_extrapolation() {
    let points = [
        [382.714_f32, 29.57029, 250.39615],
        [382.7129, 29.56619, 250.39726],
        [371.74753, 18.600805, 256.93896],
        [365.94635, 17.046387, 259.74124],
        [354.05545, 18.33474, 264.79132],
    ]
    .map(|point| DVec3::from_array(point.map(f64::from)));
    let triangles = [
        [points[0], points[1], points[2]],
        [points[0], points[2], points[3]],
        [points[0], points[3], points[4]],
    ];
    let skinny = Triangle::new(triangles[0]).unwrap();
    let extrapolated = skinny.normal().dot(points[4] - points[0]);
    assert!((extrapolated.abs() - 0.03573880727265255).abs() < 1e-9);
    assert!(extrapolated.abs() > GEOMETRY_EPSILON);
    assert!(skinny.distance_squared(points[4]).unwrap() > GEOMETRY_EPSILON * GEOMETRY_EPSILON);
    let distances = minimum_distances(&points, &triangles);
    assert!(distances.iter().all(|&distance| distance < 1e-20));
}

fn box_triangles(max_x: f64) -> Vec<[DVec3; 3]> {
    let points = (0..8)
        .map(|bits| {
            DVec3::new(
                (bits & 1) as f64 * max_x,
                ((bits >> 1) & 1) as f64,
                ((bits >> 2) & 1) as f64,
            )
        })
        .collect::<Vec<_>>();
    let faces = [
        [1, 3, 7, 5],
        [0, 4, 6, 2],
        [2, 6, 7, 3],
        [0, 1, 5, 4],
        [4, 5, 7, 6],
        [0, 2, 3, 1],
    ];
    faces
        .into_iter()
        .flat_map(|face| {
            [
                [points[face[0]], points[face[1]], points[face[2]]],
                [points[face[0]], points[face[2]], points[face[3]]],
            ]
        })
        .collect()
}

#[test]
fn reciprocal_distance_refuses_real_crop_beyond_the_unchanged_tolerance() {
    let points = [DVec3::new(1.0, 0.0, 0.0), DVec3::new(1.0, 0.5, 0.5)];
    let distances = minimum_distances(&points, &box_triangles(0.97));
    assert!(
        distances
            .iter()
            .all(|&distance| (distance - 0.0009).abs() < 1e-15)
    );
    assert!(
        distances
            .iter()
            .all(|&distance| distance > GEOMETRY_EPSILON * GEOMETRY_EPSILON)
    );
    let distances = minimum_distances(&points, &box_triangles(0.981));
    assert!(
        distances
            .iter()
            .all(|&distance| distance < GEOMETRY_EPSILON * GEOMETRY_EPSILON)
    );
}

#[test]
fn nonfinite_or_degenerate_triangles_and_nonfinite_source_points_still_refuse() {
    for triangle in [
        [DVec3::ZERO; 3],
        [DVec3::ZERO, DVec3::ONE, DVec3::splat(2.0)],
        [DVec3::ZERO, DVec3::X, DVec3::new(0.0, f64::NAN, 0.0)],
    ] {
        assert!(Triangle::new(triangle).is_err());
    }
    let triangle = Triangle::new([DVec3::ZERO, DVec3::X, DVec3::Y]).unwrap();
    assert!(
        triangle
            .distance_squared(DVec3::new(0.0, 0.0, f64::INFINITY))
            .is_err()
    );
}
