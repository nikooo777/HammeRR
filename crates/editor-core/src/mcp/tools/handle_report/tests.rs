use super::*;

type Query = fn(&EditorServer, Parameters<HandlesArgs>) -> ToolResult;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn error(result: ToolResult) -> String {
    let result = result.unwrap();
    assert!(result.is_error.unwrap_or(false), "{result:?}");
    result.content[0].as_text().unwrap().text.clone()
}

#[test]
fn handle_queries_clamp_size_and_resolve_camera_without_a_guide() {
    let session = Session::new(Err("no assets needed".into()));
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let queries: [Query; 4] = [
        EditorServer::get_component_handles,
        EditorServer::get_clip_handles,
        EditorServer::get_transform_handles,
        EditorServer::get_terrain_move_handles,
    ];
    for query in queries {
        let report = value(query(
            &server,
            Parameters(
                serde_json::from_value(json!({
                    "width": 0,
                    "height": u32::MAX,
                    "camera": {"projection": "top", "center": [1, 2, 3]},
                    "point": [f64::MAX, 0],
                }))
                .unwrap(),
            ),
        ));
        assert_eq!(report["width"], 16);
        assert_eq!(report["height"], 2048);
        assert_eq!(report["camera"]["projection"], "top");
        assert!(report["origin"].is_null());
        assert!(report["picked"].is_null());
        assert_eq!(report["handles"], json!([]));
        assert_eq!(
            error(query(
                &server,
                Parameters(
                    serde_json::from_value(json!({
                        "camera": {"projection": "invalid"},
                        "point": [f64::MAX, 0],
                    }))
                    .unwrap()
                ),
            )),
            "projection must be perspective, top, front or side",
        );
    }
    assert_eq!(lock(&shared).camera(), camera);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn transform_handle_query_checks_point_after_camera_when_a_guide_exists() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .set_transform_tool(
            Actor::User,
            crate::session::TransformTool {
                mode: Some(render::handles::Mode::Move),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.transform_guide().is_some());
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared, None);
    assert_eq!(
        error(server.get_transform_handles(Parameters(
            serde_json::from_value(json!({"point": [f64::MAX, 0]})).unwrap(),
        ))),
        "Handle point must be finite",
    );
    assert_eq!(
        error(
            server.get_transform_handles(Parameters(
                serde_json::from_value(json!({
                    "camera": {"projection": "invalid"},
                    "point": [f64::MAX, 0],
                }))
                .unwrap(),
            ))
        ),
        "projection must be perspective, top, front or side",
    );
}
