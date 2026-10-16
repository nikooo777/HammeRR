use super::*;
use source_formats::fgd::Catalog;

const FGD: &str = r#"
@BaseClass = projector_values [
    angles(angle) : "Pitch Yaw Roll" : "0 0 0"
    lightfov(float) : "FOV" : 90
    nearz(float) : "NearZ" : 4
    farz(float) : "FarZ" : 750
    lightcolor(color255) : "Light Color" : "255 255 255 200"
    target(target_destination) : "target"
    parentname(target_destination) : "Parent"
    cameraspace(integer) : "Camera Space" : 0
]
@PointClass base(projector_values) size(-2 -2 -2, 2 2 2)
    frustum(lightfov,nearz,farz,lightcolor,-1) = env_projectedtexture []
"#;

fn fixture(values: &[(&str, &str)]) -> Projector {
    let catalog = Catalog::parse(FGD).unwrap();
    let mut entity = Entity::new(1, "env_projectedtexture");
    entity.properties.set("origin", "10 20 30");
    entity.properties.0.extend(
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string())),
    );
    resolve(catalog.class("env_projectedtexture").unwrap(), &entity)
}

fn close(actual: [f64; 3], expected: [f64; 3]) {
    assert!(
        DVec3::from_array(actual).distance(DVec3::from_array(expected)) < 1e-9,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn source_projector_defaults_are_fgd_values_with_twelve_axial_edges() {
    let result = fixture(&[]);
    assert_eq!(result.state, "ready");
    assert_eq!(result.fov_degrees, Some(90.0));
    assert_eq!((result.near_z, result.far_z), (Some(4.0), Some(750.0)));
    assert_eq!(result.color, Some([255; 3]));
    assert_eq!(result.intensity, Some(200.0));
    assert_eq!(result.inputs[0].value_source, "authored");
    assert!(
        result.inputs[1..6]
            .iter()
            .all(|input| input.value_source == "fgd_default")
    );
    assert_eq!(result.helper_pitch_scales[0].raw.as_deref(), Some("-1"));
    let corners = result.corners.unwrap();
    close(corners[0], [14.0, 24.0, 26.0]);
    close(corners[4], [760.0, 770.0, -720.0]);
    close(corners[6], [760.0, -730.0, 780.0]);
    assert_eq!(result.lines().len(), 12);
    assert_eq!(result.lines()[8], [corners[0], corners[4]]);
}

#[test]
fn source_projector_pitch_yaw_and_roll_follow_sdk_axes_independently() {
    for (angles, first, opposite) in [
        ("90 0 0", [6.0, 24.0, 26.0], [14.0, 16.0, 26.0]),
        ("0 90 0", [6.0, 24.0, 26.0], [14.0, 24.0, 34.0]),
        ("0 0 90", [14.0, 24.0, 34.0], [14.0, 16.0, 26.0]),
    ] {
        let result = fixture(&[("angles", angles)]);
        assert_eq!(result.state, "ready");
        let corners = result.corners.unwrap();
        close(corners[0], first);
        close(corners[2], opposite);
    }
    let result = fixture(&[("angles", "30 0 0"), ("lightfov", "60")]);
    let corners = result.corners.unwrap();
    let center = (DVec3::from_array(corners[0]) + DVec3::from_array(corners[2])) * 0.5;
    close(center.to_array(), [10.0 + 2.0 * 3_f64.sqrt(), 20.0, 28.0]);
    assert!(
        (DVec3::from_array(corners[0]).distance(DVec3::from_array(corners[1]))
            - 8.0 / 3_f64.sqrt())
        .abs()
            < 1e-9
    );
}

#[test]
fn source_projector_invalid_authored_inputs_never_use_valid_defaults() {
    for (key, raw) in [
        ("lightfov", "0"),
        ("lightfov", "180"),
        ("lightfov", "NaN"),
        ("nearz", "0"),
        ("nearz", "750"),
        ("farz", "65537"),
        ("farz", "1e100"),
        ("angles", "1 2"),
        ("lightcolor", "-1 -1 -1 1"),
        ("lightcolor", "12 34 56 -1"),
        ("lightfov", "179.999"),
    ] {
        let result = fixture(&[(key, raw)]);
        assert_eq!(result.state, "invalid", "{key}: {raw}");
        assert!(result.error.is_some());
        assert!(result.corners.is_none() && result.guide_bounds.is_none());
        assert!(result.lines().is_empty());
        assert_eq!(
            result
                .inputs
                .iter()
                .find(|input| input.property == key)
                .unwrap()
                .value_source,
            "authored"
        );
    }
    let duplicate = fixture(&[("lightfov", "60"), ("LIGHTFOV", "75")]);
    assert!(duplicate.error.unwrap().contains("more than once"));
    let oversized = fixture(&[("farz", &"0".repeat(MAX_VALUE_BYTES + 1))]);
    assert!(oversized.inputs[4].raw_truncated);
    assert_eq!(
        oversized.inputs[4].raw.as_ref().unwrap().len(),
        MAX_VALUE_BYTES
    );
    assert!(oversized.error.unwrap().contains("was not parsed"));
    let collapsed = fixture(&[("nearz", "0.000000001")]);
    assert_eq!(collapsed.state, "invalid");
    assert!(collapsed.error.unwrap().contains("native precision"));
}

#[test]
fn source_projector_runtime_branches_remain_explicitly_unsupported() {
    for (key, raw) in [
        ("target", "aim"),
        ("parentname", "parent"),
        ("cameraspace", "1"),
        ("angle", "-1"),
    ] {
        let result = fixture(&[(key, raw)]);
        assert_eq!(result.state, "unsupported", "{key}");
        assert!(result.error.unwrap().contains(key));
        assert!(result.lines().is_empty());
    }
}

#[test]
fn source_projector_pitch_metadata_is_bounded_and_never_changes_geometry() {
    let catalog = Catalog::parse(&FGD.replace(
        "frustum(lightfov,nearz,farz,lightcolor,-1)",
        &format!(
            "{} frustum(a,b,c,d,{})",
            "frustum(a,b,c,d,100) ".repeat(MAX_PITCH_SCALES),
            "x".repeat(400)
        ),
    ))
    .unwrap();
    let mut entity = Entity::new(1, "env_projectedtexture");
    entity.properties.set("origin", "10 20 30");
    let result = resolve(catalog.class("env_projectedtexture").unwrap(), &entity);
    assert_eq!(result.corners, fixture(&[]).corners);
    assert_eq!(result.helper_pitch_scales.len(), MAX_PITCH_SCALES);
    assert_eq!(result.omitted_helper_declarations, 1);
    let catalog = Catalog::parse(&FGD.replace(
        "frustum(lightfov,nearz,farz,lightcolor,-1)",
        &format!("frustum(a,b,c,d,{})", "é".repeat(200)),
    ))
    .unwrap();
    let result = resolve(catalog.class("env_projectedtexture").unwrap(), &entity);
    assert!(result.helper_pitch_scales[0].raw_truncated);
    assert_eq!(
        result.helper_pitch_scales[0].raw.as_ref().unwrap().len(),
        MAX_VALUE_BYTES
    );
}

#[test]
fn source_projector_requires_authored_origin_compatible_definitions_and_exact_class_scope() {
    let mut entity = Entity::new(1, "env_projectedtexture");
    let catalog = Catalog::parse(&FGD.replace(
        "angles(angle)",
        "origin(origin) : \"Origin\" : \"10 20 30\" angles(angle)",
    ))
    .unwrap();
    let result = resolve(catalog.class("env_projectedtexture").unwrap(), &entity);
    assert_eq!(result.state, "invalid");
    assert_eq!(result.inputs[0].value_source, "missing");
    assert!(result.error.unwrap().contains("origin"));
    entity.properties.set("origin", "10 20 30");
    let catalog = Catalog::parse(&FGD.replace("lightfov(float)", "lightfov(string)")).unwrap();
    let result = resolve(catalog.class("env_projectedtexture").unwrap(), &entity);
    assert_eq!(result.state, "invalid");
    assert!(result.error.unwrap().contains("declared float"));
    let catalog = Catalog::parse(&FGD.replace(
        "frustum(lightfov,nearz,farz,lightcolor,-1)",
        "frustum(a,b,c,1,2,3,-1)",
    ))
    .unwrap();
    let class = catalog.class("env_projectedtexture").unwrap();
    let result = resolve(class, &entity);
    assert_eq!(result.state, "ready");
    assert_eq!(
        result.helper_pitch_scales[0].declaration_state,
        "unsupported_helper_form"
    );
    assert!(result.helper_pitch_scales[0].raw.is_none());
    entity.properties.set("classname", "light_spot");
    assert_eq!(resolve(class, &entity).state, "unsupported");
}
