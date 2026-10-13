use compiler::lighting::{LuxelFrame, Occluder, PointLight, encode_color};
use compiler::map::MapEntity;
use glam::DVec3;
use map_core::Plane;
use source_formats::bsp::TexInfo;

fn entity(properties: &[(&str, &str)]) -> MapEntity {
    MapEntity {
        id: 10,
        classname: "light".into(),
        origin: Some(DVec3::new(0.0, 0.0, 100.0)),
        brushes: Vec::new(),
        properties: properties
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

#[test]
fn projected_luxels_recover_nonunit_mirrored_fractional_and_sloped_positions() {
    for (normal, distance) in [
        (DVec3::Z, 0.0),
        (DVec3::new(0.0, -1.0, 1.0).normalize(), 16.0),
    ] {
        let plane = Plane {
            normal,
            dist: distance,
        };
        let info = TexInfo {
            lightmap_vecs: [[0.25, 0.0625, 0.0, -0.375], [-0.03125, -0.125, 0.0, 1.75]],
            ..TexInfo::default()
        };
        let mins = [-5, -9];
        let frame = LuxelFrame::new(plane, &info, mins, [8, 4]).unwrap();
        assert_eq!(frame.dimensions(), [9, 5]);
        assert_eq!(frame.sample_count(), 45);
        for t in 0..5 {
            for s in 0..9 {
                let point = frame.position(s, t).unwrap();
                assert!(plane.distance_to(point).abs() < 1e-9);
                for (axis, sample) in [s, t].into_iter().enumerate() {
                    let row = info.lightmap_vecs[axis];
                    let projected =
                        point.dot(DVec3::new(row[0] as f64, row[1] as f64, row[2] as f64))
                            + row[3] as f64;
                    assert!((projected - (mins[axis] as f64 + sample as f64)).abs() < 1e-9);
                }
            }
        }
        assert!(frame.position(9, 0).is_err());
    }
}

#[test]
fn invalid_projection_and_sample_extents_fail_without_allocation() {
    let plane = Plane {
        normal: DVec3::Z,
        dist: 0.0,
    };
    let good = TexInfo {
        lightmap_vecs: [[0.0625, 0.0, 0.0, 0.0], [0.0, 0.0625, 0.0, 0.0]],
        ..TexInfo::default()
    };
    for rows in [
        [[0.0; 4]; 2],
        [[1.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
        [[1.0, 0.0, 0.0, f32::NAN], [0.0, 1.0, 0.0, 0.0]],
    ] {
        assert!(
            LuxelFrame::new(
                plane,
                &TexInfo {
                    lightmap_vecs: rows,
                    ..good
                },
                [0; 2],
                [1; 2]
            )
            .is_err()
        );
    }
    for size in [[-1, 2], [i32::MAX; 2], [2048; 2]] {
        assert!(LuxelFrame::new(plane, &good, [0; 2], size).is_err());
    }
}

#[test]
fn point_lights_match_reference_distance_color_gamma_and_lambert_response() {
    let light = PointLight::from_entity(
        &entity(&[("_light", "255 128 0 200"), ("_quadratic_attn", "1")]),
        4,
    )
    .unwrap();
    let center = light.contribution(DVec3::ZERO, DVec3::Z).unwrap();
    assert!((center.x - 200.0).abs() < 1e-9);
    assert!((center.y - (128.0_f64 / 255.0).powf(2.2) * 200.0).abs() < 1e-9);
    assert_eq!(center.z, 0.0);
    let far = light
        .contribution(DVec3::new(0.0, 0.0, -100.0), DVec3::Z)
        .unwrap();
    assert!((far.x - 50.0).abs() < 1e-9);
    assert_eq!(
        light.contribution(DVec3::ZERO, -DVec3::Z).unwrap(),
        DVec3::ZERO
    );
    let diagonal = light
        .contribution(DVec3::new(100.0, 0.0, 0.0), DVec3::Z)
        .unwrap();
    assert!((diagonal.x - 100.0 / 2.0_f64.sqrt()).abs() < 1e-9);
    let native = light.world_light();
    assert_eq!(native.cluster, 4);
    assert!((native.intensity[0] as f64 - 2_000_000.0 / 255.0).abs() < 0.001);
    let constant = PointLight::from_entity(&entity(&[("_light", "255 255 255 80")]), 0).unwrap();
    assert_eq!(
        constant.contribution(DVec3::ZERO, DVec3::Z).unwrap(),
        DVec3::splat(80.0)
    );
    let cutoff =
        PointLight::from_entity(&entity(&[("_light", "255"), ("_distance", "50")]), 0).unwrap();
    assert_eq!(
        cutoff.contribution(DVec3::ZERO, DVec3::Z).unwrap(),
        DVec3::ZERO
    );
}

#[test]
fn unsupported_and_nonfinite_light_settings_are_explicit() {
    for properties in [
        vec![("_light", "255 255 255 NaN")],
        vec![("_light", "999 0 0 200")],
        vec![("_light", "1 2")],
        vec![("_light", "255"), ("_quadratic_attn", "-1")],
        vec![("_light", "255"), ("style", "32")],
        vec![("_light", "255"), ("targetname", "switchable")],
        vec![("_light", "255"), ("_zero_percent_distance", "128")],
        vec![("_light", "255 255 255 1e308"), ("_quadratic_attn", "1")],
    ] {
        assert!(
            PointLight::from_entity(&entity(&properties), 0).is_err(),
            "{properties:?}"
        );
    }
}

#[test]
fn percentage_falloff_matches_sdk_curves_without_legacy_intensity_rescaling() {
    for (half, end, expected) in [
        ("128", "2048", [1.0, 1.0 / 30720.0, 239.0 / 3932160.0]),
        (
            "100",
            "200",
            [8.0 / 261.0, 0.01 * 8.0 / 261.0, 0.006325 * 8.0 / 261.0],
        ),
    ] {
        let source = entity(&[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", half),
            ("_zero_percent_distance", end),
            ("_quadratic_attn", "999"),
            ("_distance", "1"),
        ]);
        let light = PointLight::from_entity(&source, 7).unwrap();
        let record = light.world_light();
        assert_eq!(record.radius, 0.0);
        assert!((record.intensity[0] as f64 - 200.0 / 255.0).abs() < 1e-7);
        for (actual, expected) in [
            record.constant_attenuation,
            record.linear_attenuation,
            record.quadratic_attenuation,
        ]
        .into_iter()
        .zip(expected)
        {
            assert!((f64::from(actual) - expected).abs() < 1e-7 * expected.abs().max(1e-10));
        }
        let half: f64 = half.parse().unwrap();
        let color = light
            .contribution(light.origin - DVec3::Z * half, DVec3::Z)
            .unwrap();
        assert!(color.distance(DVec3::new(100.0, 0.0, 0.0)) < 1e-10);
    }
    let light = PointLight::from_entity(
        &entity(&[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "128"),
            ("_zero_percent_distance", "2048"),
        ]),
        0,
    )
    .unwrap();
    assert!(
        (light
            .contribution(light.origin - DVec3::Z * 2048.0, DVec3::Z)
            .unwrap()
            .x
            - 200.0 / 256.0)
            .abs()
            < 1e-10
    );
}

#[test]
fn percentage_hard_fade_and_soft_caps_keep_point_and_spot_response_bounded() {
    let source = entity(&[
        ("_light", "255 0 0 200"),
        ("_fifty_percent_distance", "100"),
        ("_zero_percent_distance", "200"),
        ("_hardfalloff", "1"),
        ("pitch", "-90"),
        ("_inner_cone", "20"),
        ("_cone", "45"),
    ]);
    let point = PointLight::from_entity(&source, 0).unwrap();
    let spot = point.with_spotlight(&source, &Default::default()).unwrap();
    let coefficients = [8.0 / 261.0, 0.01 * 8.0 / 261.0, 0.006325 * 8.0 / 261.0];
    let d = 187.5;
    let expected = 100.0 / (coefficients[0] + coefficients[1] * d + coefficients[2] * d * d);
    for light in [point, spot] {
        assert!(
            (light
                .contribution(light.origin - DVec3::Z * d, DVec3::Z)
                .unwrap()
                .x
                - expected)
                .abs()
                < 1e-10
        );
        for distance in [200.0, 256.0] {
            assert_eq!(
                light
                    .contribution(light.origin - DVec3::Z * distance, DVec3::Z)
                    .unwrap(),
                DVec3::ZERO
            );
        }
    }
    let source = entity(&[
        ("_light", "255 0 0 200"),
        ("_fifty_percent_distance", "100"),
        ("_zero_percent_distance", "50000"),
    ]);
    let light = PointLight::from_entity(&source, 0).unwrap();
    assert!(light.world_light().quadratic_attenuation < 0.0);
    let mut previous = f64::INFINITY;
    for distance in [100.0, 50000.0, 60000.0, 100000.0, 600000.0] {
        let current = light
            .contribution(light.origin - DVec3::Z * distance, DVec3::Z)
            .unwrap()
            .x;
        assert!(current.is_finite() && current >= 0.0 && current <= previous);
        previous = current;
    }
    assert_eq!(previous, 0.0);
}

#[test]
fn invalid_percentage_falloff_and_unrepresentable_coefficients_are_explicit() {
    for fields in [
        vec![
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "100"),
        ],
        vec![
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "50"),
        ],
        vec![("_fifty_percent_distance", "-1")],
        vec![("_fifty_percent_distance", "NaN")],
        vec![("_fifty_percent_distance", "1e308")],
        vec![("_fifty_percent_distance", "1e-100")],
        vec![("_fifty_percent_distance", "100"), ("_hardfalloff", "2")],
        vec![("_hardfalloff", "1")],
        vec![("_quadratic_attn", "1e-100")],
    ] {
        let mut fields = fields;
        fields.push(("_light", "255 0 0 200"));
        assert!(
            PointLight::from_entity(&entity(&fields), 0).is_err(),
            "{fields:?}"
        );
    }
    let light = PointLight::from_entity(
        &entity(&[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "100"),
        ]),
        0,
    )
    .unwrap();
    assert!((light.contribution(DVec3::ZERO, DVec3::Z).unwrap().x - 100.0).abs() < 1e-10);
}

#[test]
fn spotlights_match_source_cones_focus_direction_and_native_records() {
    let source = entity(&[
        ("_light", "255 255 255 200"),
        ("_constant_attn", "1"),
        ("pitch", "-90"),
        ("_inner_cone", "20"),
        ("_cone", "45"),
        ("_exponent", "2"),
    ]);
    let light = PointLight::from_entity(&source, 7)
        .unwrap()
        .with_spotlight(&source, &Default::default())
        .unwrap();
    let record = light.world_light();
    assert_eq!(record.kind, source_formats::bsp::world_light_kind::SPOT);
    assert_eq!(record.cluster, 7);
    assert!((DVec3::from_array(record.normal.map(f64::from)) - DVec3::NEG_Z).length() < 1e-6);
    assert!((f64::from(record.stop_dot) - 20_f64.to_radians().cos()).abs() < 1e-6);
    assert!((f64::from(record.stop_dot2) - 45_f64.to_radians().cos()).abs() < 1e-6);
    assert_eq!(record.exponent, 2.0);
    for degrees in [0_f64, 10.0, 35.0, 60.0] {
        let angle = degrees.to_radians();
        let point = light.origin + DVec3::new(angle.sin(), 0.0, -angle.cos()) * 100.0;
        let dot = angle.cos();
        let fringe = if degrees > 45.0 {
            0.0
        } else if degrees <= 20.0 {
            1.0
        } else {
            ((dot - 45_f64.to_radians().cos())
                / (20_f64.to_radians().cos() - 45_f64.to_radians().cos()))
            .powi(2)
        };
        let expected = DVec3::splat(200.0 * dot * dot * fringe);
        assert!(
            light
                .contribution(point, DVec3::Z)
                .unwrap()
                .distance(expected)
                < 1e-8
        );
    }
}

#[test]
fn spotlight_targets_override_angles_and_invalid_cones_or_references_reject() {
    let source = entity(&[
        ("_light", "255"),
        ("_inner_cone", "20"),
        ("_cone", "45"),
        ("target", "aim"),
        ("angles", "broken"),
    ]);
    let targets = [("aim", vec![source.origin.unwrap() + DVec3::X])].into();
    let light = PointLight::from_entity(&source, 0)
        .unwrap()
        .with_spotlight(&source, &targets)
        .unwrap();
    assert_eq!(light.world_light().normal, [1.0, 0.0, 0.0]);
    let duplicate = [("aim", vec![DVec3::X, DVec3::Y])].into();
    assert!(
        PointLight::from_entity(&source, 0)
            .unwrap()
            .with_spotlight(&source, &duplicate)
            .is_err()
    );
    for fields in [
        vec![("_inner_cone", "50"), ("_cone", "20")],
        vec![("_cone", "91")],
        vec![("pitch", "NaN")],
        vec![("target", "missing")],
    ] {
        let mut source = entity(&[("_light", "255")]);
        source.properties.extend(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        assert!(
            PointLight::from_entity(&source, 0)
                .unwrap()
                .with_spotlight(&source, &Default::default())
                .is_err()
        );
    }
    let point = entity(&[("_light", "255"), ("_inner_cone", "180"), ("_cone", "180")]);
    assert_eq!(
        PointLight::from_entity(&point, 0)
            .unwrap()
            .with_spotlight(&point, &Default::default())
            .unwrap()
            .world_light()
            .kind,
        source_formats::bsp::world_light_kind::POINT
    );
}

#[test]
fn rgbexp_encoding_preserves_energy_and_rejects_invalid_colors() {
    for color in [
        DVec3::ZERO,
        DVec3::new(255.0, 128.0, 0.0),
        DVec3::new(0.25, 0.125, 0.0625),
        DVec3::splat(65_536.0),
    ] {
        let encoded = encode_color(color).unwrap();
        let decoded = DVec3::from_array(encoded.linear());
        let tolerance = 2.0_f64.powi(encoded.exponent as i32);
        assert!((decoded - color).abs().max_element() <= tolerance);
        assert!(decoded.cmple(color).all());
    }
    for bad in [
        DVec3::splat(-1.0),
        DVec3::splat(f64::NAN),
        DVec3::splat(f64::MAX),
    ] {
        assert!(encode_color(bad).is_err());
    }
}

#[test]
fn convex_shadow_clipping_respects_slopes_gaps_inside_and_endpoints() {
    let mut planes = vec![
        Plane {
            normal: -DVec3::X,
            dist: 0.0,
        },
        Plane {
            normal: -DVec3::Y,
            dist: 0.0,
        },
        Plane {
            normal: -DVec3::Z,
            dist: 0.0,
        },
    ];
    planes.push(Plane {
        normal: DVec3::ONE.normalize(),
        dist: 16.0 / 3.0_f64.sqrt(),
    });
    let mut invalid = planes.clone();
    invalid[0].normal = DVec3::splat(f64::MAX);
    assert!(Occluder::new(invalid, DVec3::ZERO, DVec3::splat(16.0)).is_err());
    let wedge = Occluder::new(planes, DVec3::ZERO, DVec3::splat(16.0)).unwrap();
    assert!(
        wedge
            .intersects(DVec3::new(-1.0, 2.0, 2.0), DVec3::new(20.0, 2.0, 2.0))
            .unwrap()
    );
    assert!(
        !wedge
            .intersects(DVec3::new(-1.0, 12.0, 12.0), DVec3::new(20.0, 12.0, 12.0))
            .unwrap()
    );
    assert!(
        wedge
            .intersects(DVec3::splat(1.0), DVec3::splat(2.0))
            .unwrap()
    );
    assert!(
        !wedge
            .intersects(DVec3::new(-4.0, 2.0, 2.0), DVec3::new(0.0, 2.0, 2.0))
            .unwrap()
    );
    assert!(
        wedge
            .intersects(DVec3::splat(f64::NAN), DVec3::ZERO)
            .is_err()
    );
}
