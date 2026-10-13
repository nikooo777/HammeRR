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
        vec![("_light", "255"), ("_fifty_percent_distance", "128")],
        vec![("_light", "255 255 255 1e308"), ("_quadratic_attn", "1")],
    ] {
        assert!(
            PointLight::from_entity(&entity(&properties), 0).is_err(),
            "{properties:?}"
        );
    }
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
