use super::*;
use crate::physics::{Body, Model, Request, Shape, default_materials};
use source_formats::bsp::contents;

fn fluid(current_hu: Option<[f32; 3]>) -> Body {
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
        damping: None,
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
            bodies: vec![fluid(Some([64.0, -32.0, 16.0]))],
        }],
        materials: default_materials(),
        surface_data: Vec::new(),
    }
}

#[test]
fn public_schema_validates_finite_l2_bounded_canonical_source_velocities() {
    validate_water_current_velocity(None).unwrap();
    for (raw, expected) in [
        ("0 -0 +0e0", [0.0; 3]),
        ("128 0 0", [128.0, 0.0, 0.0]),
        ("0 -128 0", [0.0, -128.0, 0.0]),
        ("0 0 128", [0.0, 0.0, 128.0]),
        ("6.4e1 -32 16", [64.0, -32.0, 16.0]),
    ] {
        validate_water_current_velocity(Some(raw)).unwrap();
        assert_eq!(
            parse(raw).unwrap().map(f32::to_bits),
            expected.map(f32::to_bits)
        );
    }
    let exactly_max_bytes = format!("{}0 0 0", " ".repeat(MAX_WATER_CURRENT_VALUE_BYTES - 5));
    validate_water_current_velocity(Some(&exactly_max_bytes)).unwrap();
    for raw in [
        "",
        "0 0",
        "0 0 0 0",
        "NaN 0 0",
        "0 inf 0",
        "0 0 -inf",
        "1e999 0 0",
        "128.00001 0 0",
        "128 1 0",
        "-128 -1 0",
        "90.51 90.51 0",
        "128 -64 32",
        "127.999999 0.01 0",
        "1e-999 0 0",
        "1e-50 0 0",
        "1e-45 0 0",
        "0 0 0junk",
        "0 0 0x1",
        "0 0 1,0",
        "0 0 0\0",
        "0\u{a0}0 0",
    ] {
        assert!(validate_water_current_velocity(Some(raw)).is_err(), "{raw}");
    }
    assert!(validate_water_current_velocity(Some(&(exactly_max_bytes + " "))).is_err());
}

#[test]
fn native_boundary_has_one_scale_and_source_axis_order() {
    assert_eq!(native(None), [0.0; 3]);
    for (source, expected) in [
        ([128.0, 0.0, 0.0], [3.2512, 0.0, 0.0]),
        ([-128.0, 0.0, 0.0], [-3.2512, 0.0, 0.0]),
        ([0.0, 128.0, 0.0], [0.0, 3.2512, 0.0]),
        ([0.0, 0.0, -128.0], [0.0, 0.0, -3.2512]),
        ([64.0, -32.0, 16.0], [1.6256, -0.8128, 0.4064]),
    ] {
        validate(source).unwrap();
        assert_eq!(
            native(Some(source)).map(f32::to_bits),
            expected.map(f32::to_bits)
        );
    }
}

#[test]
fn world_property_duplicates_and_disabled_physics_are_not_ignored_even_for_zero() {
    let mut vmf = crate::test_common::room();
    assert_eq!(from_world(&vmf, PhysicsMode::None).unwrap(), None);
    vmf.world
        .properties
        .set(WATER_CURRENT_VELOCITY_KEY, "-0 0 0");
    assert_eq!(
        from_world(&vmf, PhysicsMode::Engine).unwrap(),
        Some([0.0; 3])
    );
    assert!(
        from_world(&vmf, PhysicsMode::None)
            .unwrap_err()
            .contains("engine physics")
    );
    vmf.world.properties.0.push((
        WATER_CURRENT_VELOCITY_KEY.to_ascii_uppercase(),
        "0 0 0".into(),
    ));
    assert!(
        from_world(&vmf, PhysicsMode::Engine)
            .unwrap_err()
            .contains("repeats")
    );
}

#[test]
fn parent_and_worker_request_validation_enforce_roles_uniformity_and_units() {
    let good = request();
    crate::physics::validate(&good).unwrap();
    let json = serde_json::to_vec(&good).unwrap();
    let roundtrip: Request = serde_json::from_slice(&json).unwrap();
    assert_eq!(
        roundtrip.models[0].bodies[0].current_hu,
        Some([64.0, -32.0, 16.0])
    );
    crate::physics::validate(&roundtrip).unwrap();
    for case in 0..10 {
        let mut bad = request();
        match case {
            0 => bad.models[0].bodies[0].current_hu = Some([128.0, 1.0, 0.0]),
            1 => bad.models[0].bodies[0].current_hu = Some([f32::NAN, 0.0, 0.0]),
            2 => bad.models[0].bodies[0].current_hu = Some([f32::from_bits(1), 0.0, 0.0]),
            3 => bad.models[0].bodies[0].contents = contents::SLIME,
            4 => bad.models[0].bodies[0].surface = None,
            5 => {
                bad.model_count = 2;
                bad.models[0].index = 1;
            }
            6 => bad.models[0].bodies.push(fluid(None)),
            7 => bad.models[0].bodies.push(fluid(Some([1.0, 0.0, 0.0]))),
            8 => {
                let mut slime = fluid(None);
                slime.contents = contents::SLIME;
                bad.models[0].bodies.push(slime);
            }
            9 => {
                bad.models[0].bodies[0].mesh = Some(crate::physics::Mesh {
                    vertices: Vec::new(),
                    triangles: Vec::new(),
                    materials: Vec::new(),
                })
            }
            _ => unreachable!(),
        }
        assert!(crate::physics::validate(&bad).is_err(), "case {case}");
    }
    let mut defaults = request();
    defaults.models[0].bodies[0].current_hu = None;
    let json = serde_json::to_value(&defaults).unwrap();
    assert!(json["models"][0]["bodies"][0].get("current_hu").is_none());
    let compatible: Request = serde_json::from_value(json).unwrap();
    crate::physics::validate(&compatible).unwrap();
    let mut uniform = request();
    uniform.models[0]
        .bodies
        .push(fluid(Some([64.0, -32.0, 16.0])));
    crate::physics::validate(&uniform).unwrap();
}
