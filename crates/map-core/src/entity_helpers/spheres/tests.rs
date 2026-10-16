use super::*;
use source_formats::fgd::Catalog;

fn resolve_fixture(definition: &str, values: &[(&str, &str)]) -> Vec<Sphere> {
    let catalog = Catalog::parse(definition).unwrap();
    let mut entity = Entity::new(1, "sample");
    entity.properties.set("origin", "10 20 30");
    entity.properties.0.extend(
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string())),
    );
    resolve(catalog.class("sample").unwrap(), &entity)
}

#[test]
fn inherited_distinct_named_radii_keep_defaults_authored_precedence_and_world_axes() {
    let definition = "@BaseClass sphere(inner) sphere(outer) sphere(INNER) sphere(text) = shared [ inner(float) : \"Inner\" : 12 outer(integer) : \"Outer\" : 24 ] @PointClass base(shared) = sample [ text(string) : \"Text radius\" : \"48.5\" ]";
    let rows = resolve_fixture(
        definition,
        &[("OuTeR", "36"), ("angles", "30 45 60"), ("modelscale", "9")],
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|row| row.property.as_str())
            .collect::<Vec<_>>(),
        ["inner", "outer", "text"]
    );
    assert_eq!(
        rows.iter().map(|row| row.radius).collect::<Vec<_>>(),
        [Some(12.0), Some(36.0), Some(48.5)]
    );
    assert_eq!(rows[0].value_source, "fgd_default");
    assert_eq!(rows[1].value_source, "authored");
    assert_eq!(
        rows[0].guide_bounds,
        Some([[-2.0, 8.0, 18.0], [22.0, 32.0, 42.0]])
    );
    let lines = rows[0].lines();
    assert_eq!(lines.len(), 3 * SPHERE_SEGMENTS);
    assert_eq!(lines[0][0], [22.0, 20.0, 30.0]);
    for (ring, axes) in [2, 1, 0].into_iter().enumerate() {
        let segments = &lines[ring * SPHERE_SEGMENTS..(ring + 1) * SPHERE_SEGMENTS];
        assert_eq!(segments.last().unwrap()[1], segments[0][0]);
        for segment in segments {
            for point in segment {
                assert_eq!(point[axes], [10.0, 20.0, 30.0][axes]);
                assert!(
                    (DVec3::from_array(*point).distance(DVec3::new(10.0, 20.0, 30.0)) - 12.0).abs()
                        < 1e-10
                );
            }
        }
    }
}

#[test]
fn malformed_authored_values_never_fall_back_and_nonpositive_values_remain_explicit() {
    let definition = "@PointClass sphere(radius) = sample [ radius(float) : \"Radius\" : 64 ]";
    for raw in ["bad", "NaN", "inf", "1 2", "65537", "1e100"] {
        let row = resolve_fixture(definition, &[("radius", raw)]).remove(0);
        assert_eq!(row.state, "invalid", "{raw}");
        assert_eq!(row.value_source, "authored");
        assert!(row.radius.is_none());
        assert!(row.lines().is_empty());
    }
    for raw in ["0", "-1", "-24"] {
        let row = resolve_fixture(definition, &[("radius", raw)]).remove(0);
        assert_eq!(row.state, "nonpositive");
        assert_eq!(row.radius, Some(raw.parse().unwrap()));
        assert!(row.guide_bounds.is_none());
        assert!(row.lines().is_empty());
    }
    let row = resolve_fixture(definition, &[("radius", "8"), ("RADIUS", "16")]).remove(0);
    assert_eq!(row.state, "invalid");
    assert!(row.error.unwrap().contains("more than once"));
    let integer = "@PointClass sphere(radius) = sample [ radius(integer) : \"Radius\" : 64 ]";
    assert_eq!(
        resolve_fixture(integer, &[("radius", "1.5")])[0].state,
        "invalid"
    );
}

#[test]
fn unsupported_forms_missing_values_and_native_precision_are_diagnostic() {
    for declaration in ["sphere()", "sphere(missing)", "sphere(radius, other)"] {
        let rows = resolve_fixture(
            &format!("@PointClass {declaration} = sample [ radius(float) : \"Radius\" : 64 ]"),
            &[],
        );
        assert_eq!(rows[0].state, "unsupported");
        assert!(rows[0].lines().is_empty());
    }
    let rows = resolve_fixture(
        "@PointClass sphere(radius) = sample [ radius(float) : \"Radius\" ]",
        &[],
    );
    assert_eq!(rows[0].state, "invalid");
    assert_eq!(rows[0].value_source, "missing");
    let definition = "@PointClass sphere(radius) = sample [ radius(float) : \"Radius\" : 1 ]";
    let catalog = Catalog::parse(definition).unwrap();
    let mut entity = Entity::new(1, "sample");
    for origin in ["NaN 0 0", "1e100 0 0", "1000000000000 0 0"] {
        entity.properties.set("origin", origin);
        assert_eq!(
            resolve(catalog.class("sample").unwrap(), &entity)[0].state,
            "invalid",
            "{origin}"
        );
    }
}

#[test]
fn report_caps_bound_oversized_input_without_parsing_its_clipped_prefix() {
    let definition = "@PointClass sphere(radius) = sample [ radius(string) : \"Radius\" : 64 ]";
    let raw = format!("64{}", " ".repeat(1024 * 1024));
    let row = resolve_fixture(definition, &[("radius", &raw)]).remove(0);
    assert_eq!(row.state, "invalid");
    assert!(row.raw_truncated);
    assert!(row.raw.unwrap().len() <= MAX_SPHERE_VALUE_BYTES);
    assert!(row.error.unwrap().len() <= MAX_SPHERE_ERROR_BYTES);
    let key = "r".repeat(1024 * 1024);
    let row = resolve_fixture(&format!("@PointClass sphere({key}) = sample []"), &[]).remove(0);
    assert_eq!(row.state, "unsupported");
    assert!(row.property_truncated);
    assert_eq!(row.property.len(), MAX_SPHERE_KEY_BYTES);
    let helpers = (0..20)
        .map(|i| format!("sphere(r{i})"))
        .collect::<Vec<_>>()
        .join(" ");
    let properties = (0..20)
        .map(|i| format!("r{i}(float) : \"Radius\" : {}", i + 1))
        .collect::<Vec<_>>()
        .join(" ");
    let rows = resolve_fixture(
        &format!("@PointClass {helpers} = sample [{properties}]"),
        &[],
    );
    assert_eq!(rows.len(), MAX_SPHERES_PER_ENTITY + 1);
    assert_eq!(rows[MAX_SPHERES_PER_ENTITY].state, "unsupported");
    assert!(
        rows[MAX_SPHERES_PER_ENTITY]
            .error
            .as_ref()
            .unwrap()
            .contains("omitted")
    );
}
