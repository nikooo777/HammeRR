use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn action(server: &EditorServer, args: Value) -> Value {
    value(server.set_measurement(Parameters(serde_json::from_value(args).unwrap())))
}

fn preview(server: &EditorServer, id: Option<u32>) -> Value {
    let state = value(server.get_measurements());
    action(
        server,
        json!({"action":"preview","id":id,"name":"Doorway clearance",
        "start":[-16.25,-24.5,8.125],"end":[64.5,48.25,32.75],"snapshot":state["snapshot"]}),
    )
}

#[test]
fn mcp_and_human_measurements_share_preview_ownership_persistence_and_undo() {
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let original = lock(&shared).document().vmf().clone();
    let server = EditorServer::new(shared.clone(), None);
    let draft = preview(&server, None);
    let token = draft["draft"]["token"].as_u64().unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    let reviewed = lock(&shared)
        .review_measurement(Actor::User, token)
        .unwrap();
    let user_token = reviewed["draft"]["token"].as_u64().unwrap();
    for args in [
        json!({"action":"apply","token":user_token}),
        json!({"action":"cancel","token":user_token}),
        json!({"action":"apply","token":token}),
    ] {
        assert!(
            server
                .set_measurement(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    let reviewed = action(&server, json!({"action":"review","token":user_token}));
    let token = reviewed["draft"]["token"].as_u64().unwrap();
    action(&server, json!({"action":"apply","token":token}));
    let state = value(server.get_measurements());
    assert_eq!(
        state["measurements"][0]["start"],
        json!([-16.25, -24.5, 8.125])
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 1);
    let saved = lock(&shared).document().vmf().clone();
    let revised = preview(&server, Some(1));
    action(
        &server,
        json!({"action":"cancel","token":revised["draft"]["token"]}),
    );
    assert_eq!(lock(&shared).document().vmf(), &saved);
    let state = value(server.get_measurements());
    action(
        &server,
        json!({"action":"delete","id":1,"snapshot":state["snapshot"]}),
    );
    assert!(
        lock(&shared)
            .measurements()
            .unwrap()
            .measurements
            .is_empty()
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &saved);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

fn image(server: &EditorServer, projection: &str) -> Vec<u8> {
    let camera = if projection == "perspective" {
        json!({"projection":projection,"position":[-160,-192,144],"look_at":[24,12,20]})
    } else {
        json!({"projection":projection,"center":[24,12,20],"ortho_height":160})
    };
    let result = server
        .render_view(Parameters(
            serde_json::from_value(json!({"width":400,"height":300,"camera":camera})).unwrap(),
        ))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    let block = serde_json::to_value(&result.content[0]).unwrap();
    assert_eq!(block["type"], "image");
    base64::engine::general_purpose::STANDARD
        .decode(block["data"].as_str().unwrap())
        .unwrap()
}

#[test]
#[ignore = "requires a graphics adapter for offscreen MCP validation"]
fn measurement_offscreen_mcp_renders_preview_saved_hidden_and_undo_in_all_projections() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_visible: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let original = lock(&shared).document().vmf().clone();
    let server = EditorServer::new(shared.clone(), None);
    for projection in ["perspective", "top", "front", "side"] {
        let empty = image(&server, projection);
        let draft = preview(&server, None);
        let token = draft["draft"]["token"].as_u64().unwrap();
        let rendered = image(&server, projection);
        assert_ne!(rendered, empty, "{projection} measurement was invisible");
        assert_eq!(lock(&shared).document().vmf(), &original);
        if let Some(directory) = std::env::var_os("HAMMERR_MEASUREMENT_SNAPSHOTS") {
            std::fs::write(
                PathBuf::from(directory).join(format!("measurements-{projection}.png")),
                &rendered,
            )
            .unwrap();
        }
        action(&server, json!({"action":"apply","token":token}));
        assert_eq!(
            image(&server, projection),
            rendered,
            "{projection} Apply changed the preview"
        );
        let state = value(server.get_measurements());
        action(
            &server,
            json!({"action":"view","show_guides":false,"snapshot":state["snapshot"]}),
        );
        assert_eq!(
            image(&server, projection),
            empty,
            "{projection} hidden ruler remained visible"
        );
        let state = value(server.get_measurements());
        action(
            &server,
            json!({"action":"view","show_guides":true,"snapshot":state["snapshot"]}),
        );
        assert_eq!(image(&server, projection), rendered);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(image(&server, projection), empty);
    }
}
