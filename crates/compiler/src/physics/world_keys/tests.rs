use super::*;
use crate::physics::{Body, MASK_SOLID, Mesh};
use source_formats::keyvalues::Value;

const SOLID: &str = r#"staticsolid { "index" "0" "contents" "33570827" }"#;
const CLIP: &str = r#"staticsolid { "index" "1" "contents" "65536" }"#;
const TERRAIN: &str = r#"staticsolid { "index" "2" "contents" "33570827" }"#;
const WATER: &str = r#"fluid { "index" "3" "surfaceprop" "water" "damping" "0.01" "contents" "32" "surfaceplane" "0 0 1 64" "currentvelocity" "0 0 0" }"#;
const SLIME: &str = r#"fluid { "index" "4" "surfaceprop" "slime" "damping" "0.01" "contents" "16" "surfaceplane" "0 0 1 -8" "currentvelocity" "0 0 0" }"#;
const MATERIALS: &str = r#"materialtable { "default" "1" "metal" "2" }"#;

fn body(contents: i32, surface: Option<f32>) -> Body {
    Body {
        contents,
        shapes: Vec::new(),
        surface,
        current_hu: None,
        damping: None,
        surface_property: None,
        mass_area: None,
        mesh: None,
    }
}

fn model() -> Model {
    let mut terrain = body(MASK_SOLID, None);
    terrain.mesh = Some(Mesh {
        vertices: Vec::new(),
        triangles: Vec::new(),
        materials: Vec::new(),
    });
    Model {
        index: 0,
        bodies: vec![
            body(MASK_SOLID, None),
            body(source_formats::bsp::contents::PLAYERCLIP, None),
            terrain,
            body(source_formats::bsp::contents::WATER, Some(64.0)),
            body(source_formats::bsp::contents::SLIME, Some(-8.0)),
        ],
    }
}

fn materials() -> Vec<String> {
    vec!["default".into(), "metal".into()]
}

fn keys() -> String {
    format!("{SOLID}\n{CLIP}\n{TERRAIN}\n{WATER}\n{SLIME}\n{MATERIALS}\n\0")
}

#[test]
fn authored_damping_requires_the_exact_requested_scalar_independently_of_current() {
    for current in [None, Some([64.0, -32.0, 16.0])] {
        for (requested, text) in [(0.0, "-0"), (0.005, "5e-3"), (0.01, "1e-2")] {
            let mut expected = model();
            expected.bodies[4].contents = source_formats::bsp::contents::WATER;
            for body in &mut expected.bodies[3..] {
                body.damping = Some(requested);
                body.current_hu = current;
            }
            let original = keys()
                .replace("\"slime\"", "\"water\"")
                .replace("\"contents\" \"16\"", "\"contents\" \"32\"")
                .replace("\"damping\" \"0.01\"", &format!("\"damping\" \"{text}\""));
            let original = if current.is_some() {
                original.replace("\"0 0 0\"", "\"1.6256 -0.8128 0.4064\"")
            } else {
                original
            };
            validate(
                original.as_bytes(),
                &expected,
                &materials(),
                &Control::default(),
            )
            .unwrap();
            for substitute in ["0", "0.005", "0.01"] {
                if substitute.parse::<f32>().unwrap() == requested {
                    continue;
                }
                let changed = original.replacen(
                    &format!("\"damping\" \"{text}\""),
                    &format!("\"damping\" \"{substitute}\""),
                    1,
                );
                assert!(
                    validate(
                        changed.as_bytes(),
                        &expected,
                        &materials(),
                        &Control::default()
                    )
                    .is_err()
                );
            }
        }
    }
    let changed = keys().replace("\"damping\" \"0.01\"", "\"damping\" \"0.005\"");
    assert!(
        validate(
            changed.as_bytes(),
            &model(),
            &materials(),
            &Control::default()
        )
        .is_err()
    );
}

#[test]
fn exact_solid_clip_mesh_water_and_slime_keys_and_semantic_spelling_are_admitted() {
    let original = keys();
    validate(
        original.as_bytes(),
        &model(),
        &materials(),
        &Control::default(),
    )
    .unwrap();
    let equivalent = original
        .replace("fluid", "FlUiD")
        .replace("staticsolid", "StaticSolid")
        .replace("materialtable", "MaterialTable")
        .replace("contents", "CONTENTS")
        .replace("\"water\"", "\"WaTeR\"")
        .replace("\"0.01\"", "\"1e-2\"")
        .replace("\"0 0 1 64\"", "\"-0 0.0 1e0 6.4e1\"")
        .replace("\"0 0 0\"", "\"-0 0e0 +0.0\"")
        .replace("\"index\" \"3\"", "\"INDEX\" \"+003\"")
        .replace("\"32\"", "\"+032\"");
    validate(
        equivalent.as_bytes(),
        &model(),
        &materials(),
        &Control::default(),
    )
    .unwrap();
    let mut nodes = keyvalues::parse_bounded(original.trim_end_matches('\0'), 1, 64).unwrap();
    nodes.reverse();
    for node in &mut nodes {
        if let Value::Block(fields) = &mut node.value {
            fields.reverse();
        }
    }
    let reordered = keyvalues::write(&nodes, &Default::default()) + "\0";
    validate(
        reordered.as_bytes(),
        &model(),
        &materials(),
        &Control::default(),
    )
    .unwrap();
}

#[test]
fn missing_extra_duplicate_and_wrong_body_roles_cannot_publish_as_world_keys() {
    let original = keys();
    for (label, from, to) in [
        ("missing fluid", WATER, String::new()),
        ("missing solid", SOLID, String::new()),
        ("missing material table", MATERIALS, String::new()),
        ("duplicate fluid index", SLIME, WATER.into()),
        ("duplicate solid index", CLIP, SOLID.into()),
        ("duplicate material table", WATER, MATERIALS.into()),
        (
            "fluid treated as solid",
            WATER,
            r#"staticsolid { index 3 contents 32 }"#.into(),
        ),
        (
            "world solid treated as fluid",
            SOLID,
            WATER.replace("\"3\"", "\"0\""),
        ),
        ("brush-model mass role", "staticsolid", "solid".into()),
        (
            "unsupported world role",
            "staticsolid",
            "virtualterrain".into(),
        ),
        ("scalar body", WATER, r#"fluid "not a block""#.into()),
        ("extra block", MATERIALS, format!("{MATERIALS}\nextra {{}}")),
        (
            "duplicate whole body",
            MATERIALS,
            format!("{MATERIALS}\n{WATER}"),
        ),
    ] {
        let changed = original.replace(from, &to);
        assert_ne!(changed, original, "{label}");
        assert!(
            validate(
                changed.as_bytes(),
                &model(),
                &materials(),
                &Control::default()
            )
            .is_err(),
            "{label}"
        );
    }
}

#[test]
fn malformed_or_changed_body_fields_and_native_float_values_are_rejected() {
    let original = keys();
    for (from, to) in [
        (r#""index" "3""#, r#""index" "99""#),
        (r#""index" "3""#, r#""index" "-1""#),
        (r#""index" "3""#, r#""index" "3.0""#),
        (r#""index" "3""#, r#""index" "0x3""#),
        (r#""contents" "33570827""#, r#""contents" "1""#),
        (r#""contents" "32""#, r#""contents" "16""#),
        (r#""contents" "32""#, r#""contents" "48""#),
        (r#""contents" "32""#, r#""contents" "3.2e1""#),
        (r#""contents" "32""#, r#""contents" "32junk""#),
        (r#""surfaceprop" "water""#, r#""surfaceprop" "slime""#),
        (r#""surfaceprop" "slime""#, r#""surfaceprop" "water""#),
        (r#""surfaceprop" "water""#, r#""surfaceprop" "metal""#),
        (r#""damping" "0.01""#, r#""damping" "0""#),
        (r#""damping" "0.01""#, r#""damping" "-0.01""#),
        (r#""damping" "0.01""#, r#""damping" "0.02""#),
        (r#""damping" "0.01""#, r#""damping" "NaN""#),
        (r#""damping" "0.01""#, r#""damping" "inf""#),
        (r#""damping" "0.01""#, r#""damping" "1e50""#),
        (r#""damping" "0.01""#, r#""damping" "0.01junk""#),
        (
            r#""currentvelocity" "0 0 0""#,
            r#""currentvelocity" "1 0 0""#,
        ),
        (
            r#""currentvelocity" "0 0 0""#,
            r#""currentvelocity" "0 NaN 0""#,
        ),
        (r#""currentvelocity" "0 0 0""#, r#""currentvelocity" "0 0""#),
        (
            r#""currentvelocity" "0 0 0""#,
            r#""currentvelocity" "0 0 0 0""#,
        ),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 0 1 65""#,
        ),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 0 -1 64""#,
        ),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 0 2 128""#,
        ),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 1 1 64""#,
        ),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 0 1 NaN""#,
        ),
        (r#""surfaceplane" "0 0 1 64""#, r#""surfaceplane" "0 0 1""#),
        (
            r#""surfaceplane" "0 0 1 64""#,
            r#""surfaceplane" "0 0 1 64 0""#,
        ),
        (
            r#""surfaceplane" "0 0 1 -8""#,
            r#""surfaceplane" "0 0 1 -7""#,
        ),
        (r#""index" "3""#, r#""INDEX" "3" "index" "3""#),
        (r#""contents" "32""#, r#""index" "3""#),
        (r#""contents" "32""#, r#""contents" {}"#),
        (r#""contents" "32""#, r#""contents" "32" "extra" "1""#),
    ] {
        let changed = original.replace(from, to);
        assert_ne!(changed, original, "{from}");
        assert!(
            validate(
                changed.as_bytes(),
                &model(),
                &materials(),
                &Control::default()
            )
            .is_err(),
            "{from} -> {to}"
        );
    }
    for field in FLUID_FIELDS {
        let mut nodes = keyvalues::parse_bounded(original.trim_end_matches('\0'), 1, 64).unwrap();
        let Value::Block(fields) = &mut nodes[3].value else {
            unreachable!()
        };
        fields.retain(|candidate| candidate.key != field);
        let changed = keyvalues::write(&nodes, &Default::default()) + "\0";
        assert!(
            validate(
                changed.as_bytes(),
                &model(),
                &materials(),
                &Control::default()
            )
            .is_err(),
            "missing {field}"
        );
    }
}

#[test]
fn material_tables_require_the_exact_requested_index_to_surface_mapping() {
    let original = keys();
    for replacement in [
        r#"materialtable {}"#,
        r#"materialtable { default 1 }"#,
        r#"materialtable { default 1 metal 2 extra 3 }"#,
        r#"materialtable { default 1 metal 1 }"#,
        r#"materialtable { default 2 metal 1 }"#,
        r#"materialtable { default 0 metal 2 }"#,
        r#"materialtable { default 1 metal 128 }"#,
        r#"materialtable { default -1 metal 2 }"#,
        r#"materialtable { default 1.0 metal 2 }"#,
        r#"materialtable { default 1 wrong 2 }"#,
        r#"materialtable { default 1 default 2 }"#,
        r#"materialtable { default 1 metal {} }"#,
        r#"materialtable "not a block""#,
    ] {
        let changed = original.replace(MATERIALS, replacement);
        assert!(
            validate(
                changed.as_bytes(),
                &model(),
                &materials(),
                &Control::default()
            )
            .is_err(),
            "{replacement}"
        );
    }
    let changed = original.replace(MATERIALS, r#"materialtable { METAL +002 DEFAULT 01 }"#);
    validate(
        changed.as_bytes(),
        &model(),
        &materials(),
        &Control::default(),
    )
    .unwrap();
    let changed = original.replace(MATERIALS, r#"materialtable { DEFAULT 1 METAL 2 metal 3 }"#);
    validate(
        changed.as_bytes(),
        &model(),
        &["default".into(), "Metal".into(), "metal".into()],
        &Control::default(),
    )
    .unwrap();
}

#[test]
fn authored_current_requires_exactly_one_native_scale_without_axis_remapping() {
    let mut expected = model();
    expected.bodies.pop();
    expected.bodies[3].current_hu = Some([64.0, -32.0, 16.0]);
    let original = keys().replace(SLIME, "");
    let encoded = original.replace(
        "\"currentvelocity\" \"0 0 0\"",
        "\"currentvelocity\" \"1.6256 -0.8128 0.4064\"",
    );
    validate(
        encoded.as_bytes(),
        &expected,
        &materials(),
        &Control::default(),
    )
    .unwrap();
    for wrong in [
        "0 0 0",
        "64 -32 16",
        "0.04129024 -0.02064512 0.01032256",
        "1.6256 -0.4064 -0.8128",
        "1.6256 0.8128 0.4064",
        "NaN 0 0",
    ] {
        let changed = encoded.replace("1.6256 -0.8128 0.4064", wrong);
        assert!(
            validate(
                changed.as_bytes(),
                &expected,
                &materials(),
                &Control::default()
            )
            .is_err(),
            "{wrong}"
        );
    }
    expected.bodies[3].current_hu = Some([0.0; 3]);
    validate(
        original.as_bytes(),
        &expected,
        &materials(),
        &Control::default(),
    )
    .unwrap();
}

#[test]
fn world_key_framing_syntax_size_and_cancellation_stay_bounded() {
    let original = keys();
    for malformed in [
        original.trim_end_matches('\0').as_bytes().to_vec(),
        [original.as_bytes(), b"\0"].concat(),
        [b"\xff".as_slice(), original.as_bytes()].concat(),
        vec![b' '; MAX_PHYSICS_KEY_BYTES + 1],
        b"\0".to_vec(),
        original.replace(MATERIALS, "materialtable {").into_bytes(),
        original
            .replace(MATERIALS, "materialtable { default { nested {} } }")
            .into_bytes(),
        original
            .replace(MATERIALS, "materialtable { default 1 metal 2 } garbage")
            .into_bytes(),
    ] {
        assert!(validate(&malformed, &model(), &materials(), &Control::default()).is_err());
    }
    let control = Control::default();
    control.cancel().unwrap();
    assert!(validate(original.as_bytes(), &model(), &materials(), &control).is_err());
}
