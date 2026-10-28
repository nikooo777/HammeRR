use super::*;

#[test]
fn model_scale_control_uses_native_default_and_preserves_unsupported_authored_values() {
    let catalog = source_formats::fgd::Catalog::parse(
        r#"
@PointClass = prop_dynamic [ model(studio) : "Model" modelscale(float) : "Custom scale" : 2 ]
@PointClass sprite() = env_sprite [ model(sprite) : "Image" ]
"#,
    )
    .unwrap();
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("model", "models/test.mdl");
    for class in [None, catalog.class("prop_dynamic")] {
        let original = entity.clone();
        let report = inspect(&entity, class);
        let scale = report
            .properties
            .iter()
            .find(|field| field.key == "modelscale")
            .unwrap();
        assert_eq!(scale.label, "Model scale");
        assert_eq!(scale.value, "1");
        assert_eq!(scale.control, Control::Number);
        assert_eq!(scale.range, Some([0.015625, 64.0]));
        assert!(!scale.advanced);
        assert_eq!(entity, original);
    }
    entity.properties.set("modelscale", "0.5");
    assert_eq!(
        field(&entity, None, "modelscale".into(), None).control,
        Control::Number
    );
    for raw in ["0", "NaN", "2 2 2", "65"] {
        entity.properties.set("modelscale", raw);
        let field = field(&entity, None, "modelscale".into(), None);
        assert_eq!(field.control, Control::Text);
        assert_eq!(field.value, raw);
        assert!(!field.advanced);
    }
    entity.properties.remove("modelscale");
    for class in [
        "prop_static",
        "prop_detail",
        "detail_prop",
        "prop_detail_sprite",
    ] {
        entity.properties.set("classname", class);
        assert!(
            !inspect(&entity, None)
                .properties
                .iter()
                .any(|field| field.key == "modelscale")
        );
    }
    entity.properties.set("classname", "env_sprite");
    assert!(
        !inspect(&entity, catalog.class("env_sprite"))
            .properties
            .iter()
            .any(|field| field.key == "modelscale")
    );
}
