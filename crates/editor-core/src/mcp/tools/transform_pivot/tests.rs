use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn args(value: Value) -> Parameters<TransformPivotPickArgs> {
    Parameters(serde_json::from_value(value).unwrap())
}

#[test]
fn mcp_surface_pivot_shares_human_settings_provenance_and_lazy_graphics() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-64.0, -64.0, -16.0),
            DVec3::new(64.0, 64.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let state = value(server.transform_pivot_pick(args(json!({"action":"inspect"}))));
    let begun = value(server.transform_pivot_pick(args(
        json!({"action":"begin","snapshot":state["snapshot"],"width":511,"height":511}),
    )));
    let token = begun["picking"]["token"].clone();
    let picked = value(server.transform_pivot_pick(args(json!({"action":"ray","token":token,"ray_origin":[3.125,-5.25,96],"ray_direction":[0,0,-7]}))));
    assert_eq!(picked["current_pivot"], json!([3.125, -5.25, 0.0]));
    assert_eq!(picked["last_pick"]["point"], picked["current_pivot"]);
    assert!(picked["picking"].is_null());
    assert_eq!(
        lock(&shared).transform_tool().pivot,
        Some(DVec3::new(3.125, -5.25, 0.0))
    );
    assert!(
        server
            .transform_pivot_pick(args(json!({"action":"cancel","token":token})))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let human = {
        let mut session = lock(&shared);
        let camera = session.camera();
        let snapshot = session.transform_pivot_pick_stamp();
        session
            .begin_transform_pivot_pick(Actor::User, camera, (511, 511), &snapshot)
            .unwrap()
    };
    let token = human.picking.unwrap().token;
    assert!(
        server
            .transform_pivot_pick(args(
                json!({"action":"ray","token":token,"ray_origin":[0,0,96],"ray_direction":[0,0,-1]})
            ))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    lock(&shared)
        .cancel_transform_pivot_pick(Actor::User, token)
        .unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    for invalid in [
        json!({"action":"begin","width":511,"height":511}),
        json!({"action":"pixel","token":1,"x":1,"y":2,"width":511}),
        json!({"action":"cancel","token":1,"pivot":[0,0,0]}),
        json!({"action":"unknown"}),
    ] {
        assert!(serde_json::from_value::<TransformPivotPickArgs>(invalid).is_err());
    }
}
