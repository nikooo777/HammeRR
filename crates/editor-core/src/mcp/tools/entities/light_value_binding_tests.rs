use super::*;

fn report(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn light_value_binding_mcp_uses_existing_string_schemas_and_shared_drafts() {
    let mut session = Session::new(Err("template authoring needs no assets".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"@PointClass = light_spot [
                _light(color255) : "Brightness" : "255 255 255 200"
                _lightHDR(color255) : "HDR brightness" : "-1 -1 -1 1"
                _fifty_percent_distance(string) : "50 percent distance" : "0"
                _zero_percent_distance(string) : "Near-zero distance" : "0"
                _constant_attn(string) : "Constant" : "0"
                _linear_attn(string) : "Linear" : "0"
                _quadratic_attn(string) : "Quadratic" : "1"
                _distance(integer) : "Maximum distance" : 0
                _hardfalloff(integer) : "hard falloff" : 0
                _inner_cone(integer) : "Inner (bright) angle" : 30
                _cone(integer) : "Outer (fading) angle" : 45
                _exponent(integer) : "Focus" : 1
                pitch(integer) : "Pitch" : -90
            ]
            @PointClass = func_instance [ file(instance_file) : "File" replace01(string) : "Binding" ]"#,
        )
        .unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let schema = serde_json::to_value(schemars::schema_for!(PropertyChange)).unwrap();
    let set = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["action"]["const"] == "set")
        .unwrap();
    assert_eq!(set["properties"]["value"]["type"], "string");
    let created = report(
        server.create_entity(Parameters(
            serde_json::from_value(json!({
                "classname":"light_spot","origin":[0,0,64],
                "properties":{"_light":"$Lamp","_lightHDR":"$HDR","_fifty_percent_distance":"$Half","_zero_percent_distance":"$End","_inner_cone":"$Inner","_cone":"$Outer","_exponent":"$Focus","pitch":"$Tilt","_constant_attn":"$Constant","_linear_attn":"$Linear","_quadratic_attn":"$Quadratic","_distance":"$Radius","_hardfalloff":"$Hard"}
            }))
            .unwrap(),
        )),
    );
    let id = created["id"].as_str().unwrap();
    report(
        server.set_entity_properties(Parameters(
            serde_json::from_value(
                json!({"id":id,"set":{"_light":"$Other","_fifty_percent_distance":"$OtherHalf","_inner_cone":"$OtherInner","_exponent":"$OtherFocus","pitch":"$OtherTilt","_constant_attn":"$OtherConstant","_distance":"$OtherRadius","_hardfalloff":"$OtherHard"}}),
            )
            .unwrap(),
        )),
    );
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"ids":[id]})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":"_light","value":"$Draft"},
                {"action":"set","key":"_zero_percent_distance","value":"$EndDraft"},
                {"action":"set","key":"_inner_cone","value":"$InnerDraft"},
                {"action":"set","key":"_cone","value":"$OuterDraft"},
                {"action":"set","key":"_exponent","value":"$FocusDraft"},
                {"action":"set","key":"pitch","value":"$TiltDraft"},
                {"action":"set","key":"_constant_attn","value":"$ConstantDraft"},
                {"action":"set","key":"_linear_attn","value":"$LinearDraft"},
                {"action":"set","key":"_quadratic_attn","value":"$QuadraticDraft"},
                {"action":"set","key":"_distance","value":"$RadiusDraft"},
                {"action":"set","key":"_hardfalloff","value":"$HardDraft"}
            ]}))
            .unwrap(),
        )),
    );
    assert_eq!(staged, report(server.get_property_edit()));
    assert_eq!(
        staged["preview"],
        serde_json::to_value(lock(&shared).preview_property_edit().unwrap()).unwrap()
    );
    let field = &staged["preview"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["field"]["key"] == "_light")
        .unwrap()["field"];
    assert_eq!(field["control"], "text");
    assert_eq!(field["value"], "$Draft");
    let distance = staged["preview"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["field"]["key"] == "_zero_percent_distance")
        .unwrap();
    assert_eq!(distance["field"]["kind"], "string");
    assert_eq!(distance["field"]["control"], "text");
    assert_eq!(distance["field"]["value"], "$EndDraft");
    for (key, value) in [
        ("_constant_attn", "$ConstantDraft"),
        ("_linear_attn", "$LinearDraft"),
        ("_quadratic_attn", "$QuadraticDraft"),
        ("_distance", "$RadiusDraft"),
        ("_hardfalloff", "$HardDraft"),
        ("_inner_cone", "$InnerDraft"),
        ("_cone", "$OuterDraft"),
        ("_exponent", "$FocusDraft"),
        ("pitch", "$TiltDraft"),
    ] {
        let field = &staged["preview"]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|property| property["field"]["key"] == key)
            .unwrap()["field"];
        assert_eq!(
            field["kind"],
            if key.ends_with("_attn") {
                "string"
            } else {
                "integer"
            }
        );
        assert_eq!(field["control"], "text");
        assert_eq!(field["value"], value);
        assert!(
            field["range"].is_null() && field["color"].is_null() && field["intensity"].is_null()
        );
    }
    assert!(field["color"].is_null() && field["intensity"].is_null());
    assert!(
        field["description"]
            .as_str()
            .unwrap()
            .contains("Unresolved template light value")
    );
    let rejected = server
        .update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":"_light","value":"$lamp embedded"}
            ]}))
            .unwrap(),
        ))
        .unwrap();
    assert!(rejected.is_error.unwrap());
    assert_eq!(report(server.get_property_edit()), staged);
    let rejected = server.update_property_edit(Parameters(serde_json::from_value(json!({"token":token,"changes":[{"action":"set","key":"_cone","value":"$Outer embedded"}]})).unwrap())).unwrap();
    assert!(rejected.is_error.unwrap());
    assert_eq!(report(server.get_property_edit()), staged);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    let ObjectId::Entity(id) = parse_id(id).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        lock(&shared)
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("_light"),
        Some("$Draft")
    );
    let scope = report(server.get_compile_settings())["light_value_bindings"].clone();
    assert_eq!(
        report(server.get_instance_reference())["light_value_bindings"],
        scope
    );
    assert_eq!(
        report(
            server
                .inspect_instance_contents(Parameters(serde_json::from_value(json!({})).unwrap()))
        )["light_value_bindings"],
        scope
    );
    let created = report(server.create_entity(Parameters(serde_json::from_value(json!({"classname":"func_instance","origin":[0,0,0],"properties":{"file":"child.vmf","replace01":"$Lamp $Outer"}})).unwrap())));
    let helper = created["id"].as_str().unwrap();
    report(server.set_entity_properties(Parameters(
        serde_json::from_value(json!({"id":helper,"set":{"replace01":"$Lamp $Other"}})).unwrap(),
    )));
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"ids":[helper]})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(server.update_property_edit(Parameters(serde_json::from_value(json!({"token":token,"changes":[{"action":"set","key":"replace01","value":"$Lamp $Lamp"}]})).unwrap())));
    let field = &staged["preview"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|property| property["field"]["key"] == "replace01")
        .unwrap()["field"];
    assert_eq!(field["kind"], "string");
    assert_eq!(field["control"], "text");
    assert_eq!(field["value"], "$Lamp $Lamp");
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn environment_color_binding_mcp_uses_shared_create_set_and_ambient_drafts() {
    let mut session = Session::new(Err("template authoring needs no assets".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"@PointClass = light_environment [
                _light(color255) : "Brightness" : "255 255 255 200"
                _lightHDR(color255) : "HDR brightness" : "-1 -1 -1 1"
                _ambient(color255) : "Ambient" : "255 255 255 20"
                _ambientHDR(color255) : "HDR ambient" : "-1 -1 -1 1"
                _lightscaleHDR(float) : "HDR gain" : "1"
                _AmbientScaleHDR(float) : "HDR ambient gain" : "1"
                SunSpreadAngle(float) : "Sun spread" : "0"
            ]"#,
        )
        .unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let created = report(server.create_entity(Parameters(
        serde_json::from_value(json!({
            "classname":"light_environment","origin":[0,0,64],
            "properties":{"_light":"$Sun","_lightHDR":"$SunHDR","_ambient":"$Sky","_ambientHDR":"$SkyHDR","_lightscaleHDR":"$Gain","_AmbientScaleHDR":"$SkyGain","SunSpreadAngle":"$Spread"}
        }))
        .unwrap(),
    )));
    let id = created["id"].as_str().unwrap();
    report(
        server.set_entity_properties(Parameters(
            serde_json::from_value(
                json!({"id":id,"set":{"_ambientHDR":"$OtherSkyHDR","_lightscaleHDR":"$OtherGain","SunSpreadAngle":"$OtherSpread"}}),
            )
            .unwrap(),
        )),
    );
    let before = lock(&shared).document().vmf().clone();
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"ids":[id]})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":"_ambient","value":"$SkyDraft"},
                {"action":"set","key":"_ambientHDR","value":"$SkyHDRDraft"},
                {"action":"set","key":"_lightscaleHDR","value":"$GainDraft"},
                {"action":"set","key":"_AmbientScaleHDR","value":"$SkyGainDraft"},
                {"action":"set","key":"SunSpreadAngle","value":"$SpreadDraft"}
            ]}))
            .unwrap(),
        )),
    );
    for key in [
        "_ambient",
        "_ambientHDR",
        "_lightscaleHDR",
        "_AmbientScaleHDR",
        "SunSpreadAngle",
    ] {
        let field = &staged["preview"]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|property| property["field"]["key"] == key)
            .unwrap()["field"];
        assert_eq!(field["control"], "text");
        assert!(field["color"].is_null() && field["intensity"].is_null());
        if key.to_ascii_lowercase().contains("scalehdr") || key == "SunSpreadAngle" {
            assert_eq!(field["kind"], "float");
            assert!(field["range"].is_null());
        }
        assert!(
            field["description"]
                .as_str()
                .unwrap()
                .contains("Unresolved template light value")
        );
    }
    assert_eq!(lock(&shared).document().vmf(), &before);
    report(server.commit_property_edit(Parameters(PropertyTokenArgs { token })));
    let scope = report(server.get_compile_settings())["light_value_bindings"].clone();
    assert_eq!(
        scope["classes"],
        json!(["light", "light_spot", "light_environment"])
    );
    assert_eq!(
        scope["properties"],
        json!(["_light", "_lightHDR", "_lightscaleHDR"])
    );
    assert_eq!(
        scope["class_properties"],
        json!({
            "light":["_light","_lightHDR","_lightscaleHDR","_fifty_percent_distance","_zero_percent_distance","_constant_attn","_linear_attn","_quadratic_attn","_distance","_hardfalloff"],
            "light_spot":["_light","_lightHDR","_lightscaleHDR","_fifty_percent_distance","_zero_percent_distance","_inner_cone","_cone","_exponent","pitch","_constant_attn","_linear_attn","_quadratic_attn","_distance","_hardfalloff"],
            "light_environment":["_light","_lightHDR","_ambient","_ambientHDR","_lightscaleHDR","_AmbientScaleHDR","SunSpreadAngle"]
        })
    );
    assert_eq!(
        scope["scalar_properties"],
        json!([
            "_lightscaleHDR",
            "_AmbientScaleHDR",
            "_fifty_percent_distance",
            "_zero_percent_distance",
            "SunSpreadAngle",
            "_inner_cone",
            "_cone",
            "_exponent",
            "pitch",
            "_constant_attn",
            "_linear_attn",
            "_quadratic_attn",
            "_distance",
            "_hardfalloff"
        ])
    );
    assert_eq!(scope["sun_spread_properties"], json!(["SunSpreadAngle"]));
    assert_eq!(scope["sun_spread_degrees"], json!([0, 90]));
    assert_eq!(scope["sun_spread_classes"], json!(["light_environment"]));
    assert_eq!(scope["scalar_parameters"], true);
    assert_eq!(
        scope["spotlight_angle_properties"],
        json!(["_inner_cone", "_cone"])
    );
    assert_eq!(scope["spotlight_angle_classes"], json!(["light_spot"]));
    assert_eq!(scope["spotlight_angle_degrees"], json!([0, 90]));
    assert_eq!(scope["spotlight_point_angles"], json!([180, 180]));
    assert_eq!(scope["spotlight_focus_properties"], json!(["_exponent"]));
    assert_eq!(scope["spotlight_focus_classes"], json!(["light_spot"]));
    assert_eq!(scope["spotlight_pitch_properties"], json!(["pitch"]));
    assert_eq!(scope["spotlight_pitch_classes"], json!(["light_spot"]));
    assert_eq!(scope["spotlight_pitch_degrees"], json!([-90, 90]));
    assert_eq!(
        scope["scalar_literal"],
        "one finite number; nonnegative except spotlight pitch; hard falloff zero or one"
    );
    assert_eq!(
        scope["falloff_distance_classes"],
        json!(["light", "light_spot"])
    );
    assert_eq!(
        scope["falloff_distance_properties"],
        json!(["_fifty_percent_distance", "_zero_percent_distance"])
    );
    assert!(
        !scope["class_properties"]["light_environment"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "_fifty_percent_distance" || key == "_zero_percent_distance")
    );
    assert_eq!(
        scope["attenuation_coefficient_properties"],
        json!(["_constant_attn", "_linear_attn", "_quadratic_attn"])
    );
    assert_eq!(
        scope["attenuation_coefficient_classes"],
        json!(["light", "light_spot"])
    );
    assert_eq!(
        scope["attenuation_coefficient_precedence"],
        "positive 50% distance selects percentage falloff instead"
    );
    assert_eq!(
        scope["attenuation_coefficient_all_zero"],
        "ordinary constant falloff"
    );
    assert!(
        !scope["class_properties"]["light_environment"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key.as_str().unwrap().ends_with("_attn"))
    );
    assert_eq!(scope["falloff_radius_properties"], json!(["_distance"]));
    assert_eq!(
        scope["falloff_radius_classes"],
        json!(["light", "light_spot"])
    );
    assert_eq!(
        scope["falloff_radius_precedence"],
        "positive 50% distance selects percentage falloff instead"
    );
    assert_eq!(scope["hard_falloff_properties"], json!(["_hardfalloff"]));
    assert_eq!(
        scope["hard_falloff_classes"],
        json!(["light", "light_spot"])
    );
    assert_eq!(scope["hard_falloff_values"], json!([0, 1]));
    assert_eq!(scope["hard_falloff_requires_positive_half_when_one"], true);
    assert_eq!(scope["literal_only"], false);
    assert_eq!(scope["root_literal_only"], true);
    assert_eq!(scope["nested_forwarding"], true);
    assert_eq!(scope["forwarding_operand"], "whole_value_$identifier");
    assert_eq!(
        scope["forwarding_scope"],
        "immediately_containing_func_instance"
    );
    assert_eq!(scope["forwarding_same_name"], true);
    assert_eq!(scope["forwarding_sibling_lookup"], false);
    assert_eq!(scope["forwarding_recursive"], false);
    assert_eq!(scope["forwarding_max_helper_levels"], 4);
    assert!(
        !scope["rejected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "ancestor_forwarding")
    );
    assert_eq!(scope["other_scalar_parameters"], false);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
