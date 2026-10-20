use super::*;

fn fluid(current_hu: Option<[f32; 3]>) -> Body {
    Body {
        contents: source_formats::bsp::contents::WATER,
        shapes: Vec::new(),
        surface: Some(64.0),
        current_hu,
        damping: None,
        surface_property: None,
        mass_area: None,
        mesh: None,
    }
}

#[test]
fn default_fluid_text_is_unchanged_and_current_only_changes_its_native_vector() {
    let baseline = "fluid {\n\"index\" \"2\"\n\"surfaceprop\" \"water\"\n\"damping\" \"0.01\"\n\"contents\" \"32\"\n\"surfaceplane\" \"0 0 1 64\"\n\"currentvelocity\" \"0 0 0\"\n}\n";
    for value in [None, Some([0.0; 3]), Some([-0.0; 3])] {
        assert_eq!(fluid_keys(2, &fluid(value), 64.0).unwrap(), baseline);
    }
    assert_eq!(
        fluid_keys(2, &fluid(Some([64.0, -32.0, 16.0])), 64.0).unwrap(),
        baseline.replace(
            "\"currentvelocity\" \"0 0 0\"",
            "\"currentvelocity\" \"1.6256 -0.8128 0.4064\""
        )
    );
}

#[test]
fn authored_damping_changes_only_the_scalar_and_explicit_default_preserves_text() {
    let mut body = fluid(Some([64.0, -32.0, 16.0]));
    let baseline = fluid_keys(2, &body, 64.0).unwrap();
    for (value, text) in [(0.0, "0"), (-0.0, "0"), (0.005, "0.005"), (0.01, "0.01")] {
        body.damping = Some(value);
        assert_eq!(
            fluid_keys(2, &body, 64.0).unwrap(),
            baseline.replace("\"damping\" \"0.01\"", &format!("\"damping\" \"{text}\""))
        );
    }
}

#[test]
fn native_key_growth_reserves_one_terminator_before_append() {
    let mut keys = "x".repeat(MAX_PHYSICS_KEY_BYTES - 2);
    append_keys(&mut keys, "y").unwrap();
    let before = keys.clone();
    assert!(append_keys(&mut keys, "z").is_err());
    assert_eq!(keys, before);
    keys.push('\0');
    assert_eq!(keys.len(), MAX_PHYSICS_KEY_BYTES);
}
