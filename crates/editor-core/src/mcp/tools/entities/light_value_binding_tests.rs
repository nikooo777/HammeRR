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
            r#"@PointClass = light [
                _light(color255) : "Brightness" : "255 255 255 200"
                _lightHDR(color255) : "HDR brightness" : "-1 -1 -1 1"
            ]"#,
        )
        .unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for tool in [
        "create_entity",
        "set_entity_properties",
        "update_property_edit",
    ] {
        let description = server.get_tool(tool).unwrap().description.unwrap();
        assert!(description.contains("replace01–replace10"));
        assert!(description.contains("_lightHDR"));
    }
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
                "classname":"light","origin":[0,0,64],
                "properties":{"_light":"$Lamp","_lightHDR":"$HDR"}
            }))
            .unwrap(),
        )),
    );
    let id = created["id"].as_str().unwrap();
    report(server.set_entity_properties(Parameters(
        serde_json::from_value(json!({"id":id,"set":{"_light":"$Other"}})).unwrap(),
    )));
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let opened = report(server.begin_property_edit(Parameters(
        serde_json::from_value(json!({"ids":[id]})).unwrap(),
    )));
    let token = opened["draft"]["token"].as_u64().unwrap();
    let staged = report(
        server.update_property_edit(Parameters(
            serde_json::from_value(json!({"token":token,"changes":[
                {"action":"set","key":"_light","value":"$Draft"}
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
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
