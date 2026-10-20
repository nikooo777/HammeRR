use super::*;
use crate::physics::{Body, Model, Request, Shape, default_materials};
use source_formats::bsp::contents;

fn fluid(damping: Option<f32>, current_hu: Option<[f32; 3]>) -> Body {
    Body {
        contents: contents::WATER,
        shapes: vec![Shape {
            brush: 1,
            corners: (0..8)
                .map(|bits| {
                    [
                        (bits & 1) as f32 * 32.0,
                        ((bits >> 1) & 1) as f32 * 32.0,
                        ((bits >> 2) & 1) as f32 * 32.0,
                    ]
                })
                .collect(),
            planes: vec![
                [1.0, 0.0, 0.0, 32.0],
                [-1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 32.0],
                [0.0, -1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 32.0],
                [0.0, 0.0, -1.0, 0.0],
            ],
            materials: Vec::new(),
        }],
        surface: Some(32.0),
        current_hu,
        damping,
        surface_property: None,
        mass_area: None,
        mesh: None,
    }
}

fn request() -> Request {
    Request {
        model_count: 1,
        models: vec![Model {
            index: 0,
            bodies: vec![fluid(Some(0.005), None)],
        }],
        materials: default_materials(),
        surface_data: Vec::new(),
    }
}

#[test]
fn scalar_source_checks_endpoints_before_rounding_and_preserves_nonzero_representation() {
    validate_water_damping(None).unwrap();
    for (raw, expected) in [
        ("0", 0.0_f32),
        ("-0", 0.0),
        ("+0e-999", 0.0),
        ("0.005", 0.005),
        (" \t5e-3\r\n", 0.005),
        (".01", 0.01),
        ("1e-2", 0.01),
        ("0.009999999999999999", 0.01),
        ("1e-45", f32::from_bits(1)),
    ] {
        validate_water_damping(Some(raw)).unwrap();
        assert_eq!(parse(raw).unwrap().to_bits(), expected.to_bits(), "{raw}");
    }
    let exactly_max_bytes = format!("{}0", " ".repeat(MAX_WATER_DAMPING_VALUE_BYTES - 1));
    validate_water_damping(Some(&exactly_max_bytes)).unwrap();
    assert!(validate_water_damping(Some(&(exactly_max_bytes + " "))).is_err());
    for raw in [
        "",
        " ",
        "0 0",
        "0,0",
        "0x0",
        "0.01f32",
        "0\0",
        "NaN",
        "inf",
        "-inf",
        "1e999",
        "-0.001",
        "0.010000000000000002",
        "0.0100000001",
        "1e-999",
        "-1e-999",
        "1e-46",
        "\u{a0}0",
        "0\u{2003}",
        "0\u{b}0",
    ] {
        assert!(validate_water_damping(Some(raw)).is_err(), "{raw:?}");
    }
}

#[test]
fn world_duplicates_and_disabled_physics_are_rejected_even_for_zero_and_default() {
    let mut vmf = crate::test_common::room();
    assert_eq!(from_world(&vmf, PhysicsMode::None).unwrap(), None);
    for raw in ["-0", "0.01", "0.005"] {
        vmf.world.properties.set(WATER_DAMPING_KEY, raw);
        assert!(
            from_world(&vmf, PhysicsMode::None)
                .unwrap_err()
                .contains("engine physics")
        );
        assert_eq!(
            from_world(&vmf, PhysicsMode::Engine).unwrap(),
            Some(parse(raw).unwrap())
        );
    }
    vmf.world
        .properties
        .0
        .push((WATER_DAMPING_KEY.to_ascii_uppercase(), "0.005".into()));
    assert!(
        from_world(&vmf, PhysicsMode::Engine)
            .unwrap_err()
            .contains("repeats")
    );
}

#[test]
fn optional_scalar_requests_roundtrip_without_erasing_explicit_zero_or_default() {
    for value in [None, Some(0.0), Some(0.005), Some(DEFAULT_WATER_DAMPING)] {
        let mut original = request();
        original.models[0].bodies[0].damping = value;
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(
            json["models"][0]["bodies"][0].get("damping").is_none(),
            value.is_none()
        );
        assert!(json["models"][0]["bodies"][0].get("current_hu").is_none());
        let roundtrip: Request = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.models[0].bodies[0].damping, value);
        crate::physics::validate(&roundtrip).unwrap();
    }
}

#[test]
fn parent_and_worker_reject_invalid_native_scalars_and_wrong_body_roles() {
    for value in [
        f32::NAN,
        f32::INFINITY,
        -0.001,
        -f32::from_bits(1),
        0.010000001,
    ] {
        let mut invalid = request();
        invalid.models[0].bodies[0].damping = Some(value);
        assert!(crate::physics::validate(&invalid).is_err());
    }
    for case in 0..5 {
        let mut invalid = request();
        match case {
            0 => invalid.models[0].bodies[0].contents = contents::SLIME,
            1 => invalid.models[0].bodies[0].contents = contents::PLAYERCLIP,
            2 => invalid.models[0].bodies[0].surface = None,
            3 => {
                invalid.model_count = 2;
                invalid.models[0].index = 1;
                invalid.surface_data = vec![crate::physics::SurfaceData {
                    name: "surface.txt".into(),
                    text: "default { density 2000 }".into(),
                }];
            }
            4 => {
                invalid.models[0].bodies[0].mesh = Some(crate::physics::Mesh {
                    vertices: Vec::new(),
                    triangles: Vec::new(),
                    materials: Vec::new(),
                })
            }
            _ => unreachable!(),
        }
        assert!(
            crate::physics::validate(&invalid)
                .unwrap_err()
                .contains("Authored water damping")
        );
    }
}

#[test]
fn scalar_and_current_require_their_own_uniform_values_and_all_fluid_coverage() {
    for (damping, current) in [
        (Some(0.0), None),
        (Some(0.005), None),
        (Some(DEFAULT_WATER_DAMPING), None),
        (None, Some([64.0, -32.0, 16.0])),
        (Some(0.005), Some([64.0, -32.0, 16.0])),
    ] {
        let mut valid = request();
        valid.models[0].bodies = vec![fluid(damping, current), fluid(damping, current)];
        crate::physics::validate(&valid).unwrap();
        if damping.is_some() {
            let mut missing = valid.clone();
            missing.models[0].bodies[1].damping = None;
            assert!(
                crate::physics::validate(&missing)
                    .unwrap_err()
                    .contains("damping must cover")
            );
            let mut differing = valid.clone();
            differing.models[0].bodies[1].damping = Some(0.001);
            assert!(
                crate::physics::validate(&differing)
                    .unwrap_err()
                    .contains("damping must be uniform")
            );
            let mut slime = valid.clone();
            slime.models[0].bodies[1] = fluid(None, current);
            slime.models[0].bodies[1].contents = contents::SLIME;
            assert!(crate::physics::validate(&slime).is_err());
        }
        if current.is_some() {
            let mut missing = valid.clone();
            missing.models[0].bodies[1].current_hu = None;
            assert!(
                crate::physics::validate(&missing)
                    .unwrap_err()
                    .contains("current must cover")
            );
        }
    }
}
