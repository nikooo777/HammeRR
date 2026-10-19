use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn action(
    server: &EditorServer,
    action: &str,
    name: Option<&str>,
    color: Option<Value>,
) -> ToolResult {
    let state = value(server.get_color_palette());
    server.set_color_palette(Parameters(
        serde_json::from_value(json!({
            "action":action,"snapshot":state["snapshot"],"name":name,"color":color
        }))
        .unwrap(),
    ))
}

#[test]
fn typed_tools_share_user_authority_preserve_maps_and_require_explicit_reload() {
    let directory = tempfile::tempdir().unwrap();
    let authority = crate::color_palette::Authority::configured(Ok(directory.path().into()));
    let mut first = Session::new(Err("no assets needed".into()));
    first.use_application_color_palette(authority.clone());
    let mut second = Session::new(Err("no assets needed".into()));
    second.use_application_color_palette(authority);
    let unrelated = tempfile::tempdir().unwrap();
    second.configure_recovery(Ok(unrelated.path().into()));
    let source = second.document().vmf().clone();
    let source_history = second.history_snapshot();
    let first = EditorServer::new(Arc::new(Mutex::new(first)), None);
    let shared = Arc::new(Mutex::new(second));
    let second = EditorServer::new(shared.clone(), None);
    let rgb = [0.12345678901234566, 0.25000000000000006, 0.9876543210987654];
    value(action(
        &first,
        "save",
        Some("Precise"),
        Some(json!({"space":"rgb1","channels":rgb})),
    ));
    assert_eq!(
        value(second.get_color_palette())["entries"][0]["rgb1"],
        json!(rgb)
    );
    assert_eq!(
        value(action(&second, "use", Some("Precise"), None))["color"]["rgb1"],
        json!(rgb)
    );
    let snapshot = value(second.get_color_palette())["snapshot"].clone();
    let user_snapshot = lock(&shared).color_palette_state()["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    lock(&shared)
        .change_color_palette(
            "replace",
            &user_snapshot,
            Some("Precise"),
            Some(crate::color::Input {
                space: crate::color::Space::Hsl,
                channels: [120.0, 100.0, 50.0],
            }),
        )
        .unwrap();
    assert!(
        second
            .set_color_palette(Parameters(
                serde_json::from_value(
                    json!({"action":"delete","snapshot":snapshot,"name":"Precise"})
                )
                .unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap()
    );
    assert_eq!(
        value(action(&first, "use", Some("Precise"), None))["color"]["rgb255"],
        json!([0, 255, 0])
    );
    std::fs::write(
        directory.path().join(crate::color_palette::FILE_NAME),
        b"invalid",
    )
    .unwrap();
    assert!(
        action(&second, "use", Some("Precise"), None)
            .unwrap()
            .is_error
            .unwrap()
    );
    assert_eq!(value(second.get_color_palette())["ready"], false);
    assert!(
        action(&second, "reload", None, None)
            .unwrap()
            .is_error
            .unwrap()
    );
    assert_eq!(
        value(second.get_color_palette())["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), source_history);
    assert!(
        !unrelated
            .path()
            .join(crate::color_palette::FILE_NAME)
            .exists()
    );
    assert!(first.offscreen.lock().unwrap().renderer.is_none());
    for bad in [
        json!({"action":"save","snapshot":"","name":"A","color":{"space":"rgb1","channels":[0,0,0]},"extra":true}),
        json!({"action":"unknown_action","snapshot":""}),
        json!({"action":"delete","name":"A"}),
    ] {
        assert!(serde_json::from_value::<ColorPaletteArgs>(bad).is_err());
    }
    for (kind, name, color) in [
        (
            "save",
            Some("Invalid"),
            Some(json!({"space":"rgb1","channels":[0,0,1.1]})),
        ),
        ("reload", Some("Precise"), None),
        (
            "use",
            Some("Precise"),
            Some(json!({"space":"rgb255","channels":[0,0,0]})),
        ),
    ] {
        assert!(
            action(&second, kind, name, color)
                .unwrap()
                .is_error
                .unwrap()
        );
    }
}

#[test]
fn rename_tool_and_user_operation_share_one_palette_without_changing_maps() {
    let directory = tempfile::tempdir().unwrap();
    let authority = crate::color_palette::Authority::configured(Ok(directory.path().into()));
    let mut user = Session::new(Err("no assets needed".into()));
    let mut agent = Session::new(Err("no assets needed".into()));
    user.use_application_color_palette(authority.clone());
    agent.use_application_color_palette(authority);
    let source = user.document().vmf().clone();
    let history = user.history_snapshot();
    let shared = Arc::new(Mutex::new(agent));
    let server = EditorServer::new(shared.clone(), None);
    let precise = [0.12345678901234566, 0.25000000000000006, 0.9876543210987654];
    value(action(
        &server,
        "save",
        Some("Original"),
        Some(json!({"space":"rgb1","channels":precise})),
    ));
    let snapshot = user.color_palette_state()["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    user.rename_color_palette(&snapshot, "Original", "User renamed")
        .unwrap();
    fn rename(
        server: &EditorServer,
        name: &str,
        new_name: &str,
        color: Option<Value>,
    ) -> ToolResult {
        let state = value(server.get_color_palette());
        server.set_color_palette(Parameters(serde_json::from_value(json!({"action":"rename","snapshot":state["snapshot"],"name":name,"new_name":new_name,"color":color})).unwrap()))
    }
    let renamed = value(rename(&server, "User renamed", "Agent renamed", None));
    assert_eq!(renamed["entries"][0]["name"], "Agent renamed");
    assert_eq!(renamed["entries"][0]["rgb1"], json!(precise));
    assert_eq!(user.color_palette_state()["entries"], renamed["entries"]);
    let bytes = std::fs::read(directory.path().join(crate::color_palette::FILE_NAME)).unwrap();
    let path = directory.path().join(crate::color_palette::FILE_NAME);
    let inspected_source = crate::preference_file::read_source(
        &path,
        crate::color_palette::MAX_FILE_BYTES,
        "Color palette",
    )
    .unwrap();
    for (arguments, expected_error) in [
        (
            json!({"action":"rename","snapshot":renamed["snapshot"],"new_name":"Missing old name"}),
            "Choose an existing color to rename",
        ),
        (
            json!({"action":"rename","snapshot":renamed["snapshot"],"name":"Agent renamed"}),
            "Enter a new saved color name",
        ),
        (
            json!({"action":"rename","snapshot":renamed["snapshot"]}),
            "Choose an existing color to rename",
        ),
    ] {
        let result = server
            .set_color_palette(Parameters(
                serde_json::from_value::<ColorPaletteArgs>(arguments).unwrap(),
            ))
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains(expected_error)
        );
        assert_eq!(
            value(server.get_color_palette())["snapshot"],
            renamed["snapshot"]
        );
        assert_eq!(
            crate::preference_file::read_source(
                &path,
                crate::color_palette::MAX_FILE_BYTES,
                "Color palette"
            )
            .unwrap(),
            inspected_source
        );
    }
    assert!(
        rename(
            &server,
            "Agent renamed",
            "Rejected",
            Some(json!({"space":"rgb1","channels":[0,0,0]}))
        )
        .unwrap()
        .is_error
        .unwrap()
    );
    assert!(
        rename(&server, "Agent renamed", " invalid ", None)
            .unwrap()
            .is_error
            .unwrap()
    );
    let state = value(server.get_color_palette());
    assert!(server.set_color_palette(Parameters(serde_json::from_value(json!({"action":"use","snapshot":state["snapshot"],"name":"Agent renamed","new_name":"Rejected"})).unwrap())).unwrap().is_error.unwrap());
    assert_eq!(
        std::fs::read(directory.path().join(crate::color_palette::FILE_NAME)).unwrap(),
        bytes
    );
    assert_eq!(user.document().vmf(), &source);
    assert_eq!(user.history_snapshot(), history);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
