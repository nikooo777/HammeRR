use super::*;
use crate::{edit, primitive};

fn fixture(corners: u32, reflected: bool) -> (Document, u32, u32) {
    let mut document = Document::empty();
    let objects = document
        .edit("polygon source", |tx| {
            primitive::create(
                tx,
                &primitive::Spec {
                    shape: primitive::Shape::Cylinder,
                    segments: corners,
                    mins: DVec3::new(0.125, -7.25, 16.5),
                    maxs: DVec3::new(64.875, 48.125, 80.25),
                    material: "brick/test".into(),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let ObjectId::Solid(id) = objects[0] else {
        unreachable!()
    };
    document
        .edit("affine source", |tx| {
            edit::transform(
                tx,
                &objects,
                edit::Transform {
                    rotation: DVec3::new(17.0, -23.0, 31.0),
                    reflection: [reflected, false, false],
                    scale: DVec3::new(1.125, 0.75, 1.5),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let source = document.solid(id).unwrap();
    let side = source
        .sides
        .iter()
        .max_by(|a, b| {
            Plane::from_points(a.plane.0)
                .unwrap()
                .normal
                .z
                .total_cmp(&Plane::from_points(b.plane.0).unwrap().normal.z)
        })
        .unwrap()
        .id;
    (document, id, side)
}

#[test]
fn bounded_polygon_choices_capture_every_distinct_boundary_pair() {
    for corners in 3..=MAX_BOUNDARY_SOURCE_CORNERS as u32 {
        for reflected in [false, true] {
            for owned in [false, true] {
                let (mut document, id, side) = fixture(corners, reflected);
                if owned {
                    document
                        .edit("owner", |tx| {
                            edit::tie_to_entity(tx, &[id], "func_detail", &[])
                        })
                        .unwrap();
                }
                let (_, source) = solid(&document, id).unwrap();
                let shape = topology(source).unwrap();
                let face = shape.faces.iter().find(|face| face.side == side).unwrap();
                assert_eq!(face.vertices.len(), corners as usize);
                let choices = polygon_partition_pairs(source, side).unwrap();
                assert_eq!(choices, shape.polygon_partition_pairs(side).unwrap());
                assert_eq!(choices.len(), corners as usize * (corners as usize - 1) / 2);
                assert_eq!(
                    choices
                        .iter()
                        .map(|pair| pair.edges)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    choices.len()
                );
                for pair in &choices {
                    assert!(pair.edges[0] < pair.edges[1]);
                    assert!(!pair.edges.contains(&side));
                    for (neighbor, midpoint) in pair.edges.into_iter().zip(pair.midpoints) {
                        let edge = shape
                            .edges
                            .iter()
                            .find(|edge| {
                                edge.sides.contains(&side) && edge.sides.contains(&neighbor)
                            })
                            .unwrap();
                        let endpoints = edge.vertices.map(|index| shape.vertices[index].position);
                        assert_eq!(midpoint, (endpoints[0] + endpoints[1]) * 0.5);
                        assert!(
                            Plane::from_points(
                                source
                                    .sides
                                    .iter()
                                    .find(|source| source.id == side)
                                    .unwrap()
                                    .plane
                                    .0
                            )
                            .unwrap()
                            .distance_to(midpoint)
                            .abs()
                                < EPSILON
                        );
                    }
                }
                if corners == 4 {
                    assert_eq!(choices, partition_pairs(source, side).unwrap());
                    assert_eq!(choices[..2], crease_pairs(source, side).unwrap());
                } else {
                    assert!(partition_pairs(source, side).is_err());
                    assert!(
                        plan_partitions(
                            &document,
                            &[Crease {
                                solid: id,
                                side,
                                edges: choices[0].edges
                            }]
                        )
                        .is_err()
                    );
                    assert!(
                        choices
                            .windows(2)
                            .all(|pairs| pairs[0].edges < pairs[1].edges)
                    );
                }
            }
        }
    }
}

#[test]
fn oversized_or_missing_polygon_faces_reject_before_pair_capture() {
    for corners in [9, 16] {
        let (document, id, side) = fixture(corners, false);
        let source = document.solid(id).unwrap();
        assert!(
            polygon_partition_pairs(source, side)
                .unwrap_err()
                .to_string()
                .contains("3–8")
        );
        assert!(
            polygon_partition_pairs(source, 0)
                .unwrap_err()
                .to_string()
                .contains("partition face")
        );
    }
}
