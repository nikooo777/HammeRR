use super::super::props::PropTriangle;
use super::*;
use crate::Control;

fn floor(budget: &mut Budget<'_>) -> SurfaceIndex {
    let points = [
        DVec3::new(-2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, 2048.0, 0.0),
        DVec3::new(-2048.0, 2048.0, 0.0),
    ];
    let surfaces = [[0, 1, 2], [0, 2, 3]].map(|indices| Surface {
        triangle: Triangle::new(indices.map(|index| points[index])).unwrap(),
        normal: DVec3::Z,
        source: 0,
        coordinates: None,
        boundary_error: 0.0,
    });
    SurfaceIndex {
        geometry: ShadowIndex::new(surfaces.into(), &mut |work| budget.consume(work)).unwrap(),
        radiance: vec![Radiance {
            alpha: None,
            average: DVec3::new(32.0, 16.0, 8.0) / 255.0,
            reflectivity: DVec3::ONE,
            samples: Vec::new(),
            dimensions: [0; 2],
            projection: [[0.0; 4]; 2],
            mins: [0; 2],
            sky: false,
            extras: Vec::new(),
            boundary: None,
        }],
        _allocation: budget.reserve_bytes(0).unwrap(),
    }
}

fn plate(owner: Option<u32>) -> PropTriangle {
    let mut triangle = PropTriangle::new([
        DVec3::new(-4096.0, -4096.0, 8.0),
        DVec3::new(4096.0, -4096.0, 8.0),
        DVec3::new(0.0, 4096.0, 8.0),
    ])
    .unwrap();
    if let Some(owner) = owner {
        triangle.set_owner(owner);
    }
    triangle
}

pub(in crate::lighting) fn transmitted_source(
    budget: &mut Budget<'_>,
) -> (
    SurfaceIndex,
    ShadowIndex<Occluder>,
    ShadowIndex<PropTriangle>,
) {
    let mut surfaces = floor(budget);
    surfaces.radiance[0].average = DVec3::new(255.0, 128.0, 64.0) / 255.0;
    surfaces.radiance[0].extras.push(Layer {
        style: 7,
        average: DVec3::new(64.0, 128.0, 192.0) / 255.0,
        samples: Vec::new(),
    });
    let shadows = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(
        vec![super::super::props::fractional_triangle(
            [
                DVec3::new(-4096.0, -4096.0, 8.0),
                DVec3::new(4096.0, -4096.0, 8.0),
                DVec3::new(0.0, 4096.0, 8.0),
            ],
            64,
            None,
        )],
        &mut |_| Ok(()),
    )
    .unwrap();
    (surfaces, shadows, props)
}

#[test]
fn sky_transport_preserves_fractional_visibility_for_both_receiver_modes_and_real_blockers() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let hull = Occluder::new(
        [
            DVec3::X,
            -DVec3::X,
            DVec3::Y,
            -DVec3::Y,
            DVec3::Z,
            -DVec3::Z,
        ]
        .into_iter()
        .map(|normal| map_core::Plane {
            normal,
            dist: if normal == DVec3::Z {
                40.0
            } else if normal == -DVec3::Z {
                -32.0
            } else {
                2048.0
            },
        })
        .collect(),
        DVec3::new(-2048.0, -2048.0, 32.0),
        DVec3::new(2048.0, 2048.0, 40.0),
    )
    .unwrap();
    let shadows = ShadowIndex::new(
        vec![hull.clone().with_sky(vec![true; 6]).unwrap()],
        &mut |_| Ok(()),
    )
    .unwrap();
    let points = [
        DVec3::new(-8.0, -8.0, 8.0),
        DVec3::new(8.0, -8.0, 8.0),
        DVec3::new(0.0, 8.0, 8.0),
    ];
    let props = ShadowIndex::new(
        vec![super::super::props::fractional_triangle(
            points,
            64,
            Some(7),
        )],
        &mut |_| Ok(()),
    )
    .unwrap();
    let empty = ShadowIndex::new(Vec::<Triangle>::new(), &mut |_| Ok(())).unwrap();
    let environment = crate::map::MapEntity {
        id: 10,
        classname: "light_environment".into(),
        origin: None,
        brushes: Vec::new(),
        properties: [
            ("pitch", "-90"),
            ("_light", "255 255 255 255"),
            ("_ambient", "255 255 255 255"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
    };
    let light =
        super::super::sky::SkyLight::from_entity(&environment, &map_core::light::Targets::new())
            .unwrap();
    for (mode, normal, expected_sun) in [
        (super::super::ReceiverAngularMode::Normal, DVec3::Z, 191.0),
        (
            super::super::ReceiverAngularMode::IgnoreNormals,
            -DVec3::Z,
            66.85,
        ),
    ] {
        let mut visible = |direction| {
            super::super::sky_visibility_skipping_owner(
                direction,
                DVec3::ZERO,
                &shadows,
                &empty,
                &props,
                &None,
                None,
                &mut budget,
            )
        };
        let sun = light
            .sun_contribution_with_mode_visibility(normal, &[DVec3::Z], mode, &mut visible)
            .unwrap();
        assert!(sun.abs_diff_eq(DVec3::splat(expected_sun), 1e-12));
        let ambient = light
            .ambient_contribution_with_mode_visibility(normal, &[DVec3::Z], mode, &mut visible)
            .unwrap();
        assert!(ambient.abs_diff_eq(DVec3::splat(191.0), 1e-12));
    }
    assert_eq!(
        super::super::sky_visibility_skipping_owner(
            DVec3::Z,
            DVec3::ZERO,
            &shadows,
            &empty,
            &props,
            &None,
            Some(7),
            &mut budget
        )
        .unwrap(),
        1.0
    );
    let terrain = ShadowIndex::new(vec![Triangle::new(points).unwrap()], &mut |_| Ok(())).unwrap();
    assert_eq!(
        super::super::sky_visibility_skipping_owner(
            DVec3::Z,
            DVec3::ZERO,
            &shadows,
            &terrain,
            &props,
            &None,
            Some(7),
            &mut budget
        )
        .unwrap(),
        0.0
    );
    let model = Some(ShadowIndex::new(vec![hull], &mut |_| Ok(())).unwrap());
    assert_eq!(
        super::super::sky_visibility_skipping_owner(
            DVec3::Z,
            DVec3::ZERO,
            &shadows,
            &empty,
            &props,
            &model,
            Some(7),
            &mut budget
        )
        .unwrap(),
        0.0
    );
}

#[test]
fn world_and_model_directional_bounce_kernel_receive_transmitted_source_radiance() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let (surfaces, shadows, props) = transmitted_source(&mut budget);
    let directions = super::super::bounce::directions(32);
    for normal in [-DVec3::Z, DVec3::new(0.01, 0.02, -1.0)] {
        let color = super::super::bounce::gather(normal, &directions, &mut |direction| {
            Ok(surfaces
                .trace(
                    DVec3::Z * 16.0,
                    direction,
                    0.0,
                    &shadows,
                    &props,
                    &mut budget,
                )?
                .map_or(DVec3::ZERO, |hit| hit.color * 255.0))
        })
        .unwrap();
        assert!(color.abs_diff_eq(
            DVec3::new(191.0, 95.87450980392157, 47.937254901960784),
            1e-10
        ));
    }
}

#[test]
fn fractional_source_hits_scale_base_extra_styles_and_sky_once_at_actual_endpoint() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    for sky in [false, true] {
        let mut surfaces = floor(&mut budget);
        surfaces.radiance[0].average = DVec3::new(255.0, 128.0, 64.0) / 255.0;
        surfaces.radiance[0].sky = sky;
        surfaces.radiance[0].extras.push(Layer {
            style: 7,
            average: DVec3::new(64.0, 128.0, 192.0) / 255.0,
            samples: Vec::new(),
        });
        let hulls = if sky {
            vec![
                Occluder::new(
                    [
                        DVec3::X,
                        -DVec3::X,
                        DVec3::Y,
                        -DVec3::Y,
                        DVec3::Z,
                        -DVec3::Z,
                    ]
                    .into_iter()
                    .map(|normal| map_core::Plane {
                        normal,
                        dist: if normal == DVec3::Z {
                            0.0
                        } else if normal == -DVec3::Z {
                            16.0
                        } else {
                            2048.0
                        },
                    })
                    .collect(),
                    DVec3::new(-2048.0, -2048.0, -16.0),
                    DVec3::new(2048.0, 2048.0, 0.0),
                )
                .unwrap()
                .with_sky(vec![true; 6])
                .unwrap(),
            ]
        } else {
            Vec::new()
        };
        let shadows = ShadowIndex::new(hulls, &mut |_| Ok(())).unwrap();
        for (height, expected) in [
            (8.0, [191.0, 95.87450980392157, 47.937254901960784]),
            (0.0, [255.0, 128.0, 64.0]),
            (-8.0, [255.0, 128.0, 64.0]),
        ] {
            let points = [
                DVec3::new(-4096.0, -4096.0, height),
                DVec3::new(4096.0, -4096.0, height),
                DVec3::new(0.0, 4096.0, height),
            ];
            let props = ShadowIndex::new(
                vec![super::super::props::fractional_triangle(
                    points,
                    64,
                    Some(7),
                )],
                &mut |_| Ok(()),
            )
            .unwrap();
            let hit = surfaces
                .trace(
                    DVec3::Z * 16.0,
                    -DVec3::Z,
                    0.0,
                    &shadows,
                    &props,
                    &mut budget,
                )
                .unwrap()
                .unwrap();
            assert_eq!(hit.sky, sky);
            assert!((hit.color * 255.0).abs_diff_eq(DVec3::from_array(expected), 1e-12));
            let layers = surfaces.styled_colors(&hit, 0.0, &mut budget).unwrap();
            assert_eq!(layers[0], (0, hit.color));
            let expected_style = if height == 8.0 {
                DVec3::new(47.937254901960784, 95.87450980392157, 143.81176470588235)
            } else {
                DVec3::new(64.0, 128.0, 192.0)
            };
            assert_eq!(layers[1].0, 7);
            assert!((layers[1].1 * 255.0).abs_diff_eq(expected_style, 1e-12));
            let own = surfaces
                .trace_skipping_owner(
                    DVec3::Z * 16.0,
                    -DVec3::Z,
                    0.0,
                    &shadows,
                    &props,
                    Some(7),
                    &mut budget,
                )
                .unwrap()
                .unwrap();
            assert!((own.color * 255.0).abs_diff_eq(DVec3::new(255.0, 128.0, 64.0), 1e-12));
        }
    }
}

#[test]
fn source_search_keeps_nearby_positive_hits_and_shadow_endpoint_policy() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let surfaces = floor(&mut budget);
    let triangle = Triangle::new([
        DVec3::new(-2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, 2048.0, 0.0),
    ])
    .unwrap();
    let point = DVec3::new(0.0, 0.0, 0.00001);
    let end = point + DVec3::NEG_Z * 65536.0;
    assert!(triangle.intersection(point, end).unwrap().is_none());
    let (fraction, _) = triangle.source_intersection(point, end).unwrap().unwrap();
    assert!((fraction - 1.52587890625e-10).abs() < 1e-22);
    assert!(
        triangle
            .source_intersection(DVec3::ZERO, end)
            .unwrap()
            .is_none()
    );
    let shadows = ShadowIndex::new(Vec::<Occluder>::new(), &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(Vec::<PropTriangle>::new(), &mut |_| Ok(())).unwrap();
    let hit = surfaces
        .trace(point, DVec3::NEG_Z, 0.0, &shadows, &props, &mut budget)
        .unwrap()
        .unwrap();
    assert!(hit.front && !hit.sky);
    assert_eq!(hit.point, DVec3::ZERO);
    assert_eq!(hit.color * 255.0, DVec3::new(32.0, 16.0, 8.0));
    assert!(
        surfaces
            .trace(point, DVec3::Z, 0.0, &shadows, &props, &mut budget)
            .unwrap()
            .is_none()
    );
}

#[test]
fn reflected_owner_skip_preserves_other_unknown_and_world_blockers() {
    let control = Control::default();
    let mut budget = Budget::with_limit(&control, 100_000_000);
    let surfaces = floor(&mut budget);
    let shadows = ShadowIndex::new(Vec::<Occluder>::new(), &mut |_| Ok(())).unwrap();
    let point = DVec3::Z * 16.0;
    for (owners, skipped, visible) in [
        (vec![Some(7)], None, false),
        (vec![Some(7)], Some(7), true),
        (vec![Some(7)], Some(9), false),
        (vec![Some(7), Some(9)], Some(7), false),
        (vec![Some(9), Some(7)], Some(7), false),
        (vec![None], Some(7), false),
    ] {
        let props =
            ShadowIndex::new(owners.into_iter().map(plate).collect(), &mut |_| Ok(())).unwrap();
        let hit = surfaces
            .trace_skipping_owner(
                point,
                DVec3::NEG_Z,
                0.0,
                &shadows,
                &props,
                skipped,
                &mut budget,
            )
            .unwrap();
        assert_eq!(hit.is_some(), visible);
    }
    let hull = Occluder::new(
        vec![
            map_core::Plane {
                normal: DVec3::X,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_X,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::Y,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Y,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::Z,
                dist: 12.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Z,
                dist: -8.0,
            },
        ],
        DVec3::new(-16.0, -16.0, 8.0),
        DVec3::new(16.0, 16.0, 12.0),
    )
    .unwrap();
    let world = ShadowIndex::new(vec![hull], &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(vec![plate(Some(7))], &mut |_| Ok(())).unwrap();
    assert!(
        surfaces
            .trace_skipping_owner(
                point,
                DVec3::NEG_Z,
                0.0,
                &world,
                &props,
                Some(7),
                &mut budget
            )
            .unwrap()
            .is_none()
    );
    for cancelled in [false, true] {
        let control = Control::default();
        let mut budget = Budget::with_limit(&control, 100_000_000);
        if cancelled {
            control.cancel().unwrap();
        } else {
            budget.consume(100_000_000).unwrap();
        }
        assert!(
            surfaces
                .trace_skipping_owner(
                    point,
                    DVec3::NEG_Z,
                    0.0,
                    &shadows,
                    &props,
                    Some(7),
                    &mut budget
                )
                .is_err()
        );
    }
}
