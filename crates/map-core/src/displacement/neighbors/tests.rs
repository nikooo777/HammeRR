use super::*;

#[test]
fn terrain_border_smoothing_honors_both_angle_limits_and_exact_boundaries() {
    let a = patch(2, 0, 0.0, 0.0);
    let b = patch(4, 3, 128.0, 0.0);
    let planar = |points: &[DVec3]| {
        points
            .iter()
            .map(|point| DVec3::new(point.x, point.y, 16.0 + (point.x - 128.0).abs()))
            .collect::<Vec<_>>()
    };
    let pa = planar(&a.1);
    let pb = planar(&b.1);
    let grids = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &pa,
        },
        SurfaceGrid {
            power: 4,
            corners: b.0,
            positions: &pb,
        },
    ];
    let pairs = find_mixed(&grids, &mut |_| Ok(())).unwrap().pairs;
    let pair = pairs[0];
    let local = vertex_normals(&grids, &[], &mut |_| Ok(())).unwrap();
    for (angles, sewn) in [
        ([None, None], true),
        ([Some(0.0), None], false),
        ([Some(89.0), Some(180.0)], false),
        ([Some(90.0), Some(90.0)], true),
        ([Some(90.0), Some(89.0)], false),
    ] {
        let normals =
            vertex_normals_with_smoothing(&grids, &pairs, &angles, &mut |_| Ok(())).unwrap();
        for (i, j) in common_edge_indices(2, pair.first_edge, 4, pair.second_edge).unwrap() {
            if sewn {
                assert_eq!(normals[0][i], normals[1][j]);
            } else {
                assert_eq!(normals[0][i], local[0][i]);
                assert_eq!(normals[1][j], local[1][j]);
            }
        }
    }
    for angles in [
        vec![None],
        vec![Some(f64::NAN), None],
        vec![Some(181.0), None],
    ] {
        assert!(vertex_normals_with_smoothing(&grids, &pairs, &angles, &mut |_| Ok(())).is_err());
    }
}

#[test]
fn mixed_edges_match_only_shared_coarse_samples_and_share_their_normals() {
    let a = patch(2, 1, 0.0, 0.0);
    let b = patch(4, 3, 128.0, 0.0);
    let grids = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &a.1,
        },
        SurfaceGrid {
            power: 4,
            corners: b.0,
            positions: &b.1,
        },
    ];
    assert_eq!(find(&grids, &mut |_| Ok(())).unwrap().unequal_power, 1);
    let pairs = find_mixed(&grids, &mut |_| Ok(())).unwrap().pairs;
    assert_eq!(pairs.len(), 1);
    let pair = pairs[0];
    let normals = vertex_normals(&grids, &pairs, &mut |_| Ok(())).unwrap();
    for (i, j) in common_edge_indices(2, pair.first_edge, 4, pair.second_edge).unwrap() {
        assert_eq!(normals[0][i], normals[1][j]);
    }
    let mut shifted = b.1.clone();
    let (_, vertex) = common_edge_indices(2, pair.first_edge, 4, pair.second_edge).unwrap()[2];
    shifted[vertex].z += 1.0;
    let bad = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &a.1,
        },
        SurfaceGrid {
            power: 4,
            corners: b.0,
            positions: &shifted,
        },
    ];
    assert_eq!(find_mixed(&bad, &mut |_| Ok(())).unwrap().mismatched, 1);
}

fn patch(power: u32, rotation: usize, x: f64, y: f64) -> ([DVec3; 4], Vec<DVec3>) {
    let ring = [
        [x, y],
        [x, y + 128.0],
        [x + 128.0, y + 128.0],
        [x + 128.0, y],
    ]
    .map(|[x, y]| DVec3::new(x, y, 16.0));
    let corners = std::array::from_fn(|i| ring[(i + rotation) % 4]);
    let size = (1 << power) + 1;
    let positions = (0..size * size)
        .map(|index| {
            let t = (index / size) as f64 / (size - 1) as f64;
            let s = (index % size) as f64 / (size - 1) as f64;
            let mut point = corners[0]
                .lerp(corners[1], t)
                .lerp(corners[3].lerp(corners[2], t), s);
            point.z += ((point.x - 128.0).powi(2) + (point.y - 128.0).powi(2)) / 512.0;
            point
        })
        .collect();
    (corners, positions)
}

#[test]
fn rotated_curved_edges_share_area_weighted_normals_at_every_power() {
    for power in [2, 3, 4] {
        for first in 0..4 {
            for second in 0..4 {
                let a = patch(power, first, 0.0, 0.0);
                let b = patch(power, second, 128.0, 0.0);
                let grids = [
                    SurfaceGrid {
                        power,
                        corners: a.0,
                        positions: &a.1,
                    },
                    SurfaceGrid {
                        power,
                        corners: b.0,
                        positions: &b.1,
                    },
                ];
                let neighbors = find(&grids, &mut |_| Ok(())).unwrap();
                assert_eq!(neighbors.pairs.len(), 1);
                let pair = neighbors.pairs[0];
                let normals = vertex_normals(&grids, &neighbors.pairs, &mut |_| Ok(())).unwrap();
                let local = grids
                    .iter()
                    .map(|grid| {
                        DisplacementInfo {
                            power: power as i32,
                            ..Default::default()
                        }
                        .vertex_normal_sums(
                            &grid
                                .positions
                                .iter()
                                .map(|point| point.to_array())
                                .collect::<Vec<_>>(),
                        )
                        .unwrap()
                    })
                    .collect::<Vec<_>>();
                let size = (1 << power) + 1;
                for (i, j) in edge_indices(size, pair.first_edge)
                    .unwrap()
                    .into_iter()
                    .zip(
                        edge_indices(size, pair.second_edge)
                            .unwrap()
                            .into_iter()
                            .rev(),
                    )
                {
                    assert_eq!(normals[pair.first][i], normals[pair.second][j]);
                    let expected = (DVec3::from_array(local[pair.first][i])
                        + DVec3::from_array(local[pair.second][j]))
                    .normalize();
                    assert!(expected.distance(DVec3::from_array(normals[pair.first][i])) < 1e-12);
                    assert!((expected.length() - 1.0).abs() < 1e-12);
                    assert!(expected.x.abs() < 1e-12);
                }
            }
        }
    }
}

#[test]
fn connected_edge_endpoints_share_one_normal_without_double_counting() {
    let patches = [
        patch(2, 0, 0.0, 0.0),
        patch(2, 1, 128.0, 0.0),
        patch(2, 2, 128.0, 128.0),
        patch(2, 3, 0.0, 128.0),
    ];
    let grids = patches
        .iter()
        .map(|(corners, positions)| SurfaceGrid {
            power: 2,
            corners: *corners,
            positions,
        })
        .collect::<Vec<_>>();
    let links = find(&grids, &mut |_| Ok(())).unwrap();
    assert_eq!(links.pairs.len(), 4);
    let normals = vertex_normals(&grids, &links.pairs, &mut |_| Ok(())).unwrap();
    for (grid, normals) in grids.iter().zip(normals) {
        let index = grid
            .positions
            .iter()
            .position(|point| *point == DVec3::new(128.0, 128.0, 16.0))
            .unwrap();
        assert!(DVec3::from_array(normals[index]).distance(DVec3::Z) < 1e-12);
    }
}

#[test]
fn invalid_ambiguous_and_mismatched_edges_do_not_sew_and_work_is_cancellable() {
    let a = patch(2, 0, 0.0, 0.0);
    let mut b = patch(2, 0, 128.0, 0.0);
    b.1[0].z += 1.0;
    let grids = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &a.1,
        },
        SurfaceGrid {
            power: 2,
            corners: b.0,
            positions: &b.1,
        },
    ];
    let links = find(&grids, &mut |_| Ok(())).unwrap();
    assert!(links.pairs.is_empty());
    assert_eq!(links.mismatched, 1);
    assert!(
        vertex_normals(
            &grids,
            &[NeighborPair {
                first: 0,
                first_edge: 2,
                second: 1,
                second_edge: 0,
                ..Default::default()
            }],
            &mut |_| Ok(())
        )
        .is_err()
    );
    let b = patch(3, 0, 128.0, 0.0);
    let mixed = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &a.1,
        },
        SurfaceGrid {
            power: 3,
            corners: b.0,
            positions: &b.1,
        },
    ];
    let links = find(&mixed, &mut |_| Ok(())).unwrap();
    assert!(links.pairs.is_empty());
    assert_eq!(links.unequal_power, 1);
    let b = patch(2, 0, 128.0, 0.0);
    let duplicate = [
        SurfaceGrid {
            power: 2,
            corners: a.0,
            positions: &a.1,
        },
        SurfaceGrid {
            power: 2,
            corners: b.0,
            positions: &b.1,
        },
        SurfaceGrid {
            power: 2,
            corners: b.0,
            positions: &b.1,
        },
    ];
    assert!(find(&duplicate, &mut |_| Ok(())).unwrap().pairs.is_empty());
    assert!(find(&duplicate, &mut |_| Err("cancelled".into())).is_err());
    assert!(vertex_normals(&duplicate, &[], &mut |_| Err("cancelled".into())).is_err());
    assert!(
        find(
            &[SurfaceGrid {
                power: 32,
                corners: a.0,
                positions: &[]
            }],
            &mut |_| Ok(())
        )
        .is_err()
    );
}
