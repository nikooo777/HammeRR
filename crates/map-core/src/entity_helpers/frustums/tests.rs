use super::*;
use source_formats::fgd::Catalog;

const FGD: &str = r#"@PointClass frustum(fov,near,far,tint,SCALE) = guide [
angles(angle) : "Angles" : "450 37 22"
fov(float) : "FOV" : 90
near(float) : "Near" : 4
far(float) : "Far" : 120
tint(color255) : "Color" : "12 200 80 200.5"
]"#;

fn fixture(fgd: &str, values: &[(&str, &str)]) -> Frustum {
    let catalog = Catalog::parse(fgd).unwrap();
    let class = catalog.class("guide").unwrap();
    let mut entity = Entity::new(1, "guide");
    entity.properties.set("origin", "10 20 30");
    entity
        .properties
        .0
        .extend(values.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    resolve(class, &entity, &class.helpers[0])
}

fn rotated(mut p: [f64; 3], pitch: f64, yaw: f64, roll: f64) -> [f64; 3] {
    let (s, c) = roll.to_radians().sin_cos();
    p = [p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]];
    let (s, c) = pitch.to_radians().sin_cos();
    p = [c * p[0] + s * p[2], p[1], -s * p[0] + c * p[2]];
    let (s, c) = yaw.to_radians().sin_cos();
    [
        c * p[0] - s * p[1] + 10.0,
        s * p[0] + c * p[1] + 20.0,
        p[2] + 30.0,
    ]
}

#[test]
fn frustum_pitch_scaling_precedes_angle_reduction_and_matches_independent_axis_products() {
    for scale in [-1.0, 1.0, 0.0, 0.5] {
        let result = fixture(&FGD.replace("SCALE", &scale.to_string()), &[]);
        assert_eq!(result.state, "ready", "{:?}", result.error);
        assert_eq!(result.pitch_scale, Some(scale));
        assert_eq!(result.effective_angles, Some([-scale * 450.0, 37.0, 22.0]));
        for (index, actual) in result.corners.unwrap().into_iter().enumerate() {
            let d = if index < 4 { 4.0 } else { 120.0 };
            let (h, v) = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)][index % 4];
            let expected = rotated([d, -h * d, v * d], -scale * 450.0, 37.0, 22.0);
            assert!(DVec3::from_array(actual).distance(DVec3::from_array(expected)) < 1e-9);
        }
        assert_eq!(result.lines().len(), 12);
        assert_eq!(result.intensity, Some(200.5));
        assert!(
            result.inputs[1..]
                .iter()
                .all(|v| v.value_source == "fgd_default")
        );
    }
}

#[test]
fn frustum_omitted_bindings_use_declared_defaults_without_constructor_substitution() {
    let fgd = FGD
        .replace("frustum(fov,near,far,tint,SCALE)", "frustum()")
        .replace("fov(float)", "_fov(float)")
        .replace("near(float)", "_NearPlane(float)")
        .replace("far(float)", "_FarPlane(float)")
        .replace("tint(color255)", "_light(color255)");
    let result = fixture(&fgd, &[]);
    assert_eq!(result.state, "ready");
    assert_eq!(result.pitch_scale, Some(1.0));
    assert_eq!(result.pitch_scale_source, "helper_default");
    assert_eq!(result.near_z, Some(4.0));
    let absent = fixture(
        &fgd.replace(
            "_NearPlane(float) : \"Near\" : 4",
            "_NearPlane(float) : \"Near\"",
        ),
        &[],
    );
    assert_eq!(absent.state, "invalid");
    assert!(
        absent
            .error
            .unwrap()
            .contains("authored value or FGD default")
    );
    let catalog = Catalog::parse(&fgd.replace(
        "angles(angle)",
        "origin(origin) : \"Origin\" : \"99 99 99\"\nangles(angle)",
    ))
    .unwrap();
    let class = catalog.class("guide").unwrap();
    let missing = resolve(class, &Entity::new(1, "guide"), &class.helpers[0]);
    assert_eq!(missing.state, "invalid");
    assert_eq!(missing.inputs[0].value_source, "missing");
    assert!(missing.corners.is_none());
}

#[test]
fn frustum_invalid_data_and_native_alias_branches_are_explicit_and_bounded() {
    let fgd = FGD.replace("SCALE", "-1");
    for fov in ["0.02", "178"] {
        assert_eq!(fixture(&fgd, &[("fov", fov)]).state, "ready");
    }
    for (key, raw) in [
        ("fov", "0.01"),
        ("fov", "179"),
        ("near", "0"),
        ("far", "65537"),
        ("angles", "1e30 0 0"),
        ("tint", "1 2 3 -1"),
    ] {
        let current = if key == "angles" {
            FGD.replace("SCALE", "1e30")
        } else {
            fgd.clone()
        };
        let result = fixture(&current, &[(key, raw)]);
        assert_eq!(result.state, "invalid", "{key}: {raw}");
        assert!(result.corners.is_none() && result.error.is_some());
    }
    for scale in ["NaN", "inf", "1e100", "property_name"] {
        assert_eq!(fixture(&FGD.replace("SCALE", scale), &[]).state, "invalid");
    }
    for declaration in [
        "frustum(fov,fov,far,tint,-1)",
        "frustum(angles,near,far,tint,-1)",
        "frustum(fov,near,far,tint,-1,1)",
    ] {
        let result = fixture(
            &FGD.replace("frustum(fov,near,far,tint,SCALE)", declaration),
            &[],
        );
        assert_eq!(result.state, "unsupported");
    }
    let long = fixture(&fgd, &[("far", &"0".repeat(257))]);
    assert!(long.inputs[4].raw_truncated);
    assert_eq!(long.inputs[4].raw.as_ref().unwrap().len(), 256);
    assert_eq!(long.inputs[4].value_source, "authored");
    let duplicate = fixture(&fgd, &[("fov", "60"), ("FOV", "75")]);
    assert!(
        duplicate.inputs[2]
            .error
            .as_ref()
            .unwrap()
            .contains("more than once")
    );
    let long_binding = FGD.replace("fov,near", &format!("{},near", "x".repeat(80)));
    assert!(fixture(&long_binding, &[]).error.unwrap().contains("79"));
}

#[test]
fn frustum_shares_source_geometry_and_rejects_native_corner_collapse() {
    let fgd = FGD.replace("SCALE", "-1");
    let result = fixture(&fgd, &[]);
    let reference = projectors::square_geometry(
        DVec3::new(10.0, 20.0, 30.0),
        DVec3::new(450.0, 37.0, 22.0),
        90.0,
        4.0,
        120.0,
    )
    .unwrap();
    assert_eq!(result.corners, Some(reference.corners));
    let catalog = Catalog::parse(&fgd).unwrap();
    let class = catalog.class("guide").unwrap();
    let mut entity = Entity::new(1, "guide");
    for (k, v) in [
        ("origin", "65536 1 0"),
        ("angles", "90 0 45"),
        ("near", "0.001"),
        ("far", "1"),
    ] {
        entity.properties.set(k, v);
    }
    let collapsed = resolve(class, &entity, &class.helpers[0]);
    assert!(collapsed.error.unwrap().contains("native precision"));
}

#[test]
fn padded_numeric_bindings_preserve_authored_text_and_exact_geometry() {
    let fgd = FGD.replace("SCALE", "1");
    let plain = fixture(&fgd, &[("fov", "90"), ("near", "4"), ("far", "120")]);
    let padded = fixture(&fgd, &[("fov", " 90 "), ("near", "\t4 "), ("far", "120\n")]);
    assert_eq!(padded.state, "ready", "{:?}", padded.error);
    assert_eq!(padded.corners, plain.corners);
    assert_eq!(
        padded
            .inputs
            .iter()
            .find(|value| value.property == "fov")
            .unwrap()
            .raw
            .as_deref(),
        Some(" 90 ")
    );
    let oversized = format!("{}90", " ".repeat(MAX_VALUE_BYTES));
    let rejected = fixture(&fgd, &[("fov", &oversized)]);
    assert_eq!(rejected.state, "invalid");
    assert!(
        rejected
            .inputs
            .iter()
            .find(|value| value.property == "fov")
            .unwrap()
            .raw_truncated
    );
}
