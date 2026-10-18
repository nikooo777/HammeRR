use super::*;

fn close(actual: [f64; 3], expected: [f64; 3]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
    }
}

#[test]
fn literal_primary_secondary_and_nontrivial_colors_match_both_models() {
    for (rgb, hue) in [
        ([255.0, 0.0, 0.0], 0.0),
        ([255.0, 255.0, 0.0], 60.0),
        ([0.0, 255.0, 0.0], 120.0),
        ([0.0, 255.0, 255.0], 180.0),
        ([0.0, 0.0, 255.0], 240.0),
        ([255.0, 0.0, 255.0], 300.0),
    ] {
        let converted = convert(Input {
            space: Space::Rgb255,
            channels: rgb,
        })
        .unwrap();
        close(converted.hsv, [hue, 100.0, 100.0]);
        close(converted.hsl, [hue, 100.0, 50.0]);
        for (space, channels) in [
            (Space::Hsv, [hue, 100.0, 100.0]),
            (Space::Hsl, [hue, 100.0, 50.0]),
        ] {
            assert_eq!(
                convert(Input { space, channels }).unwrap().rgb255,
                rgb.map(|value| value as u8)
            );
        }
    }
    for (space, channels) in [
        (Space::Rgb1, [0.25, 0.5, 0.75]),
        (Space::Hsv, [210.0, 200.0 / 3.0, 75.0]),
        (Space::Hsl, [210.0, 50.0, 50.0]),
    ] {
        let converted = convert(Input { space, channels }).unwrap();
        close(converted.rgb1, [0.25, 0.5, 0.75]);
        assert_eq!(converted.rgb255, [64, 128, 191]);
        close(converted.hsv, [210.0, 200.0 / 3.0, 75.0]);
        close(converted.hsl, [210.0, 50.0, 50.0]);
    }
}

#[test]
fn achromatic_hue_and_wrapped_red_are_explicit_and_near_white_stays_finite() {
    for rgb in [[0.0; 3], [1.0; 3], [0.125; 3]] {
        let converted = convert(Input {
            space: Space::Rgb1,
            channels: rgb,
        })
        .unwrap();
        close(converted.hsv, [0.0, 0.0, rgb[0] * 100.0]);
        close(converted.hsl, [0.0, 0.0, rgb[0] * 100.0]);
    }
    for space in [Space::Hsv, Space::Hsl] {
        let level = if matches!(space, Space::Hsv) {
            100.0
        } else {
            50.0
        };
        assert_eq!(
            convert(Input {
                space,
                channels: [360.0, 100.0, level]
            })
            .unwrap()
            .rgb255,
            [255, 0, 0]
        );
    }
    let near_white = convert(Input {
        space: Space::Rgb1,
        channels: [1.0, 1.0, 1.0 - f64::EPSILON / 2.0],
    })
    .unwrap();
    assert!(near_white.hsl.into_iter().all(f64::is_finite));
    assert!((0.0..=100.0).contains(&near_white.hsl[1]));
    for level in [0.1, 0.3, 99.9] {
        let saturated = convert(Input {
            space: Space::Hsl,
            channels: [240.0, 100.0, level],
        })
        .unwrap();
        assert!(
            saturated
                .rgb1
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );
        assert!(
            convert(Input {
                space: Space::Rgb1,
                channels: saturated.rgb1
            })
            .is_ok()
        );
    }
}

#[test]
fn invalid_channels_reject_without_clamping_or_wrapping() {
    for (space, channels) in [
        (Space::Rgb255, [-1.0, 0.0, 0.0]),
        (Space::Rgb255, [256.0, 0.0, 0.0]),
        (Space::Rgb255, [12.5, 0.0, 0.0]),
        (Space::Rgb1, [1.00001, 0.0, 0.0]),
        (Space::Hsv, [-1.0, 50.0, 50.0]),
        (Space::Hsl, [361.0, 50.0, 50.0]),
        (Space::Hsv, [240.0, 101.0, 50.0]),
        (Space::Hsl, [120.0, 50.0, -0.01]),
    ] {
        assert!(convert(Input { space, channels }).is_err());
    }
    for space in [Space::Rgb255, Space::Rgb1, Space::Hsv, Space::Hsl] {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for index in 0..3 {
                let mut channels = [0.0; 3];
                channels[index] = invalid;
                assert!(convert(Input { space, channels }).is_err());
            }
        }
    }
    assert!(Input::property("color1", "0.2 0.3 0.4 10").is_err());
    assert!(Input::property("color255", "255 255 255 -1").is_err());
}
