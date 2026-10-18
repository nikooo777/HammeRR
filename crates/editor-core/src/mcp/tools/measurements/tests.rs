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
fn mcp_origin_actions_share_human_drafts_and_preserve_missing_references_until_explicit_repair() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = session
        .edit(Actor::User, "source entity", |tx| {
            map_core::edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::new(16.125, -32.25, 64.5)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let object_text = id_string(object);
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let draft = preview(&server, None);
    let attached = action(
        &server,
        json!({"action":"attach_origin", "token":draft["draft"]["token"], "endpoint":"start", "object":object_text}),
    );
    assert_eq!(
        attached["draft"]["measurement"]["start"],
        json!([16.125, -32.25, 64.5])
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    let reviewed = lock(&shared)
        .review_measurement(Actor::User, attached["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let user_token = reviewed["draft"]["token"].as_u64().unwrap();
    assert!(server.set_measurement(Parameters(serde_json::from_value(json!({"action":"world_endpoint", "token":user_token, "endpoint":"start", "point":[1,2,3]})).unwrap())).unwrap().is_error.unwrap_or(false));
    lock(&shared)
        .apply_measurement(Actor::User, user_token)
        .unwrap();
    assert_eq!(lock(&shared).document().undo_labels().count(), 2);
    lock(&shared)
        .edit(Actor::User, "delete entity", |tx| {
            map_core::edit::delete(tx, &[object])
        })
        .unwrap();
    let missing = value(server.get_measurements());
    assert_eq!(missing["measurements"][0]["available"], false);
    assert!(missing["measurements"][0]["start"].is_null());
    assert_eq!(
        missing["measurements"][0]["attachments"]["start"]["object"],
        object_text
    );
    let state = action(
        &server,
        json!({"action":"preview", "id":1, "name":"Doorway clearance", "start":missing["measurements"][0]["stored_start"], "end":missing["measurements"][0]["end"], "snapshot":missing["snapshot"]}),
    );
    let token = state["draft"]["token"].clone();
    for args in [
        json!({"action":"detach_origin", "token":token, "endpoint":"start"}),
        json!({"action":"apply", "token":token}),
        json!({"action":"attach_origin", "token":token, "endpoint":"start", "object":"solid:999"}),
    ] {
        assert!(
            server
                .set_measurement(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(value(server.get_measurements()), state);
    }
    let repaired = action(
        &server,
        json!({"action":"world_endpoint", "token":token, "endpoint":"start", "point":[-1.125,2.25,3.5]}),
    );
    action(
        &server,
        json!({"action":"apply", "token":repaired["draft"]["token"]}),
    );
    let saved = value(server.get_measurements());
    assert_eq!(
        saved["measurements"][0]["start"],
        json!([-1.125, 2.25, 3.5])
    );
    assert!(saved["measurements"][0]["attachments"]["start"].is_null());
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(
        value(server.get_measurements())["measurements"],
        missing["measurements"]
    );
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
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

#[test]
fn mcp_measurement_surface_actions_share_tokens_exact_coordinates_and_pixel_view_guards() {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let user_camera = session.camera();
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let draft = preview(&server, None);
    let picked = action(
        &server,
        json!({"action":"begin_pick","token":draft["draft"]["token"],"endpoint":"start",
        "width":511,"height":511,"camera":{"projection":"top","center":[3.125,-5.25,0],"ortho_height":256},"snapshot":draft["pick_snapshot"]}),
    );
    assert_eq!(lock(&shared).camera(), user_camera);
    let token = picked["draft"]["token"].as_u64().unwrap();
    for args in [
        json!({"action":"pick_pixel","token":token,"x":255,"y":255,"width":512,"height":511}),
        json!({"action":"pick_pixel","token":token,"x":511,"y":255,"width":511,"height":511}),
        json!({"action":"pick_ray","token":token,"ray_origin":[0,0,96],"ray_direction":[0,0,0]}),
        json!({"action":"pick_ray","token":token,"ray_origin":[1000,1000,1000],"ray_direction":[0,0,1]}),
        json!({"action":"apply","token":token}),
    ] {
        assert!(
            server
                .set_measurement(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(value(server.get_measurements()), picked);
    }
    let state = action(
        &server,
        json!({"action":"pick_pixel","token":token,"x":255,"y":255,"width":511,"height":511}),
    );
    assert_eq!(
        state["draft"]["measurement"]["start"],
        json!([3.125, -5.25, 0.0])
    );
    assert_eq!(
        state["draft"]["measurement"]["end"],
        draft["draft"]["measurement"]["end"]
    );
    assert_eq!(state["draft"]["last_pick"]["endpoint"], "start");
    assert_eq!(lock(&shared).document().vmf(), &source);
    let state = action(
        &server,
        json!({"action":"begin_pick","token":state["draft"]["token"],"endpoint":"end","width":511,"height":511,"snapshot":state["pick_snapshot"]}),
    );
    let state = action(
        &server,
        json!({"action":"pick_ray","token":state["draft"]["token"],"ray_origin":[30.123456789012,12.987654321012,96],"ray_direction":[0,0,-7]}),
    );
    assert_eq!(
        state["draft"]["measurement"]["end"],
        json!([30.123456789012, 12.987654321012, 0.0])
    );
    let human = lock(&shared)
        .review_measurement(Actor::User, state["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let token = human["draft"]["token"].as_u64().unwrap();
    let state = lock(&shared)
        .begin_measurement_pick(
            Actor::User,
            token,
            crate::session::measurements::Endpoint::End,
            user_camera,
            (511, 511),
            human["pick_snapshot"].as_str().unwrap(),
        )
        .unwrap();
    assert!(
        server
            .set_measurement(Parameters(
                serde_json::from_value(
                    json!({"action":"cancel_pick","token":state["draft"]["token"]})
                )
                .unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let reviewed = action(
        &server,
        json!({"action":"review","token":state["draft"]["token"]}),
    );
    let cancelled = action(
        &server,
        json!({"action":"cancel_pick","token":reviewed["draft"]["token"]}),
    );
    assert_eq!(
        cancelled["draft"]["measurement"],
        reviewed["draft"]["measurement"]
    );
    assert_eq!(
        lock(&shared).measurement_state()["draft"]["measurement"],
        state["draft"]["measurement"]
    );
    action(
        &server,
        json!({"action":"apply","token":cancelled["draft"]["token"]}),
    );
    let saved = lock(&shared).measurements().unwrap().measurements[0].clone();
    assert_eq!(saved.start, DVec3::new(3.125, -5.25, 0.0));
    assert_eq!(saved.end, DVec3::new(30.123456789012, 12.987654321012, 0.0));
    assert_eq!(lock(&shared).document().undo_labels().count(), 2);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    for invalid in [
        json!({"action":"begin_pick","token":1,"endpoint":"middle","width":511,"height":511,"snapshot":"invalid"}),
        json!({"action":"pick_pixel","token":1,"x":-1,"y":255,"width":511,"height":511}),
        json!({"action":"pick_ray","token":1,"ray_origin":[0,0,1],"ray_direction":[0,0,-1],"snap":true}),
    ] {
        assert!(serde_json::from_value::<MeasurementArgs>(invalid).is_err());
    }
}

#[test]
fn mcp_local_capture_shares_reviewed_ownership_and_rigid_attachment_reports() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = session
        .edit(Actor::User, "local target", |tx| {
            map_core::edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::new(16.0, 32.0, 64.0)),
                &[("angles".into(), "0 90 0".into())],
                Vec::new(),
            )
        })
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let source = lock(&shared).document().vmf().clone();
    let draft = preview(&server, None);
    let attached = action(
        &server,
        json!({"action":"attach_local", "token":draft["draft"]["token"], "endpoint":"start", "object":id_string(object)}),
    );
    let report = &attached["draft"]["measurement"];
    assert_eq!(report["attachments"]["start"]["kind"], "entity_local");
    assert_eq!(report["attachments"]["start"]["object"], id_string(object));
    let offset: [f64; 3] =
        serde_json::from_value(report["attachments"]["start"]["local_offset"].clone()).unwrap();
    assert!(DVec3::from_array(offset).abs_diff_eq(DVec3::new(-56.5, 32.25, -55.875), 1e-10));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(serde_json::from_value::<MeasurementArgs>(json!({"action":"attach_local", "token":1, "endpoint":"start", "object":id_string(object), "offset":[0,0,0]})).is_err());
    let reviewed = lock(&shared)
        .review_measurement(Actor::User, attached["draft"]["token"].as_u64().unwrap())
        .unwrap();
    let current = reviewed["draft"]["token"].as_u64().unwrap();
    let wire_before = value(server.get_measurements());
    assert!(server.set_measurement(Parameters(serde_json::from_value(json!({"action":"attach_local", "token":current, "endpoint":"end", "object":id_string(object)})).unwrap())).unwrap().is_error.unwrap_or(false));
    assert_eq!(lock(&shared).measurement_state(), reviewed);
    assert_eq!(value(server.get_measurements()), wire_before);
    lock(&shared)
        .apply_measurement(Actor::User, current)
        .unwrap();
    assert_eq!(
        value(server.get_measurements())["measurements"][0]["attachments"]["start"],
        report["attachments"]["start"]
    );
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
