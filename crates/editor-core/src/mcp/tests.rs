use super::*;

fn rope_presentation_result(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn rope_presentation_mcp_queries_area_volume_and_temporary_focus_share_bounds_without_graphics() {
    let mut session = Session::new(Err("no assets needed".into()));
    let target = session
        .create_entity(
            Actor::User,
            "keyframe_rope",
            Some(DVec3::new(256.0, 0.0, 128.0)),
            BTreeMap::from([("targetname".into(), "end".into())]),
        )
        .unwrap();
    let source = session
        .create_entity(
            Actor::User,
            "move_rope",
            Some(DVec3::new(0.0, 0.0, 128.0)),
            BTreeMap::from([
                ("NextKey".into(), "end".into()),
                ("Slack".into(), "0".into()),
                ("Width".into(), "32".into()),
            ]),
        )
        .unwrap();
    session.select(&[source], SelectMode::Replace).unwrap();
    session.set_camera(Camera::planar(
        Projection::Front,
        Vec3::new(128.0, 0.0, 128.0),
        512.0,
    ));
    let memory = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let authored = session.object_bounds_in(session.document(), &[source]);
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let raw = rope_presentation_result(server.get_map_info())["bounds"].clone();
    let disabled = rope_presentation_result(server.get_selection())["bounds"].clone();
    rope_presentation_result(server.set_view(Parameters(
        serde_json::from_value(json!({"rope_preview":true})).unwrap(),
    )));
    let visible = json!({"mins":[-16.0,-16.0,112.0],"maxs":[272.0,16.0,144.0]});
    assert_eq!(
        rope_presentation_result(server.get_selection())["bounds"],
        visible
    );
    let details = rope_presentation_result(server.get_objects(Parameters(IdsArgs {
        ids: Some(vec![id_string(source), id_string(target)]),
    })));
    assert_eq!(details["objects"][0]["bounds"], visible);
    assert_eq!(
        details["objects"][1]["bounds"],
        json!({"mins":[248.0,-8.0,120.0],"maxs":[264.0,8.0,136.0]})
    );
    assert_eq!(
        rope_presentation_result(server.get_map_info())["bounds"],
        raw
    );
    let volume = rope_presentation_result(
        server.select_volume(Parameters(
            serde_json::from_value(json!({"mins":[120,-2,120],"maxs":[136,2,124],"preview":true}))
                .unwrap(),
        )),
    );
    assert_eq!(volume["preview"]["objects"], json!([id_string(source)]));
    let token = volume["preview"]["token"].as_str().unwrap().to_owned();
    rope_presentation_result(server.set_view(Parameters(
        serde_json::from_value(json!({"rope_preview":false})).unwrap(),
    )));
    assert!(
        server
            .commit_volume_selection(Parameters(VolumeTokenArgs { token }))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(
        rope_presentation_result(server.get_selection())["bounds"],
        disabled
    );
    let spec = CameraSpec {
        focus: Some(vec![id_string(source)]),
        ..Default::default()
    };
    {
        let mut session = lock(&shared);
        let focused = spec
            .resolve_with_instance_contents(&session, 0.5, None, Some(true))
            .unwrap();
        let expected = camera.frame_bounds(
            Vec3::new(-16.0, -16.0, 112.0),
            Vec3::new(272.0, 16.0, 144.0),
            0.5,
        );
        assert_eq!(focused, expected);
        assert_ne!(focused, spec.resolve(&session, 0.5).unwrap());
        assert!(!session.view().rope_preview);
        let view = ViewOptions {
            rope_preview: true,
            ..session.view()
        };
        session.set_view(view);
        let excluded = spec
            .resolve_with_instance_contents(&session, 0.5, None, Some(false))
            .unwrap();
        let (mins, maxs) = authored.unwrap();
        assert_eq!(
            excluded,
            camera.frame_bounds(mins.as_vec3(), maxs.as_vec3(), 0.5)
        );
        let view = ViewOptions {
            rope_preview: false,
            ..session.view()
        };
        session.set_view(view);
    }
    let hit = rope_presentation_result(
        server.pick_at_pixel(Parameters(
            serde_json::from_value(json!({
                "width":100,"height":100,"x":49.5,"y":49.5,"rope_preview":true,
                "camera":{"focus":[id_string(source)]}
            }))
            .unwrap(),
        )),
    );
    assert_eq!(hit["object"], id_string(source));
    assert!(hit["point"][0].as_f64().unwrap() > 100.0);
    rope_presentation_result(server.set_view(Parameters(
        serde_json::from_value(json!({"rope_preview":true})).unwrap(),
    )));
    let area = rope_presentation_result(
        server.select_area(Parameters(
            serde_json::from_value(json!({
                "start":[40,40],"end":[60,60],"width":100,"height":100,"preview":true,
                "camera":{"projection":"front","center":[128,0,128],"ortho_height":64}
            }))
            .unwrap(),
        )),
    );
    assert_eq!(area["matches"]["objects"], json!([id_string(source)]));
    rope_presentation_result(server.set_view(Parameters(
        serde_json::from_value(json!({"rope_preview":false})).unwrap(),
    )));
    assert!(lock(&shared).commit_area_selection().is_err());
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), [source]);
    assert_eq!(
        session.object_bounds_in(session.document(), &[source]),
        authored
    );
    assert_eq!(server.rendering_status()["initialized"], false);
}

#[test]
fn resource_visibility_parameters_default_on_and_key_each_temporary_scene_variant() {
    let default = ViewOptions::default();
    assert_eq!(view_state(default)["entity_images"], true);
    assert_eq!(view_state(default)["entity_model_guides"], true);
    let args: SetViewArgs =
        serde_json::from_value(json!({"entity_images":false,"entity_model_guides":false})).unwrap();
    assert_eq!(args.entity_images, Some(false));
    assert_eq!(args.entity_model_guides, Some(false));
    let args: RenderArgs =
        serde_json::from_value(json!({"entity_images":true,"entity_model_guides":false})).unwrap();
    assert_eq!(args.entity_images, Some(true));
    assert_eq!(args.entity_model_guides, Some(false));
    for args in [
        json!({"entity_images":"false"}),
        json!({"entity_model_guides":1}),
    ] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    let mut keys = Vec::new();
    for entity_images in [false, true] {
        for entity_model_guides in [false, true] {
            keys.push(
                ViewOptions {
                    entity_images,
                    entity_model_guides,
                    ..default
                }
                .scene_key(7),
            );
        }
    }
    for (index, key) in keys.iter().enumerate() {
        assert!(!keys[..index].contains(key));
    }
}

#[test]
fn sky_inspection_tool_has_strict_empty_arguments_and_matches_read_only_session_report() {
    let _: InspectSkyAssetsArgs = serde_json::from_value(json!({})).unwrap();
    for value in [json!({"skyname":"sky_dust_hdr"}), json!({"channel":"hdr"})] {
        assert!(serde_json::from_value::<InspectSkyAssetsArgs>(value).is_err());
    }
    let schema = serde_json::to_value(schemars::schema_for!(InspectSkyAssetsArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let mut session = Session::new(Err("fixture has no mounted content".into()));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_family".into(),
            },
        )
        .unwrap();
    let document = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let draft = json!(session.property_draft());
    let serial = session.management_serial();
    let expected = json!(session.inspect_sky_assets());
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let result = server
        .inspect_sky_assets(Parameters(InspectSkyAssetsArgs {}))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let actual: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(actual["authored_name"], "pending_family");
    assert_eq!(actual["ldr_ready"], false);
    assert_eq!(actual["hdr_ready"], false);
    assert_eq!(actual["limits"]["asset_lookups"], 128);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(json!(session.property_draft()), draft);
    assert_eq!(session.management_serial(), serial);
}

#[test]
fn fog_preview_parameters_require_review_and_entity_controller_ids() {
    let args: FogPreviewArgs =
        serde_json::from_value(json!({"enabled":true,"controller":"entity:12","snapshot":"1:2:3"}))
            .unwrap();
    assert_eq!(
        fog_controller_id(args.controller.as_deref()).unwrap(),
        Some(12)
    );
    assert!(fog_controller_id(Some("solid:12")).is_err());
    assert!(fog_controller_id(Some("12")).is_err());
    assert!(
        serde_json::from_value::<FogPreviewArgs>(json!({"enabled":true,"controller":"entity:12"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<FogPreviewArgs>(
            json!({"enabled":true,"snapshot":"1:2:3","color":[1,2,3]})
        )
        .is_err()
    );
    let args: RenderArgs =
        serde_json::from_value(json!({"fog_preview":true,"fog_controller":"entity:12"})).unwrap();
    assert_eq!(args.fog_preview, Some(true));
    assert_eq!(args.fog_controller.as_deref(), Some("entity:12"));
}

#[test]
fn coordinates_and_camera_round_trip_without_display_rounding() {
    let point_value = DVec3::new(0.000_123_456_789, -31_234.123_456_789, 1.0 / 3.0);
    let decoded: [f64; 3] = serde_json::from_value(point(point_value)).unwrap();
    assert_eq!(decoded, point_value.to_array());
    let mut session = Session::new(Err("no assets needed".into()));
    let mut camera = session.camera();
    camera.position = Vec3::new(0.000_123_456, -1_234.567_9, 321.123_44);
    camera.yaw_degrees = 27.123_456;
    camera.pitch_degrees = -12.987_654;
    camera.fov_y_degrees = 73.123_45;
    session.set_camera(camera);
    let spec: CameraSpec = serde_json::from_value(camera_json(&camera)).unwrap();
    assert_eq!(spec.resolve(&session, 1.0).unwrap(), camera);
    for projection in [Projection::Top, Projection::Front, Projection::Side] {
        let planar = Camera::planar(projection, camera.position, 231.123_45);
        let mut value = camera_json(&planar);
        for key in ["center", "yaw", "pitch"] {
            value.as_object_mut().unwrap().remove(key);
        }
        let spec: CameraSpec = serde_json::from_value(value).unwrap();
        assert_eq!(spec.resolve(&session, 1.0).unwrap(), planar);
    }
}

#[test]
fn planar_camera_overrides_do_not_change_the_shared_view() {
    let session = Session::new(Err("no assets needed".into()));
    let original = session.camera();
    let spec = CameraSpec {
        projection: Some("top".into()),
        center: Some([10.0, -20.0, 0.0]),
        ortho_height: Some(256.0),
        ..CameraSpec::default()
    };
    let camera = spec.resolve(&session, 1.0).unwrap();
    assert_eq!(camera.projection, Projection::Top);
    assert_eq!(camera.ortho_height, 256.0);
    assert_eq!(camera.position.x, 10.0);
    assert_eq!(camera.position.y, -20.0);
    assert_eq!(session.camera(), original);
    assert!(
        CameraSpec {
            pitch: Some(20.0),
            ..spec
        }
        .resolve(&session, 1.0)
        .is_err()
    );
    assert!(
        CameraSpec {
            position: Some([f32::NAN, 0.0, 0.0]),
            ..CameraSpec::default()
        }
        .resolve(&session, 1.0)
        .is_err()
    );
}

#[test]
fn clipping_plane_inputs_agree_and_reject_ambiguous_definitions() {
    let session = Session::new(Err("no assets needed".into()));
    let spec: ClipPlane =
        serde_json::from_value(json!({"normal": [3, 4, 5], "distance": 7})).unwrap();
    let plane = spec.resolve(&session).unwrap();
    let points = map_core::clip::plane_points(plane).map(|point| point.to_array());
    let spec: ClipPlane = serde_json::from_value(json!({"points": points})).unwrap();
    let actual = spec.resolve(&session).unwrap();
    assert!(actual.normal.abs_diff_eq(plane.normal, 1e-12));
    assert!((actual.dist - plane.dist).abs() < 1e-12);
    for projection in ["top", "front", "side"] {
        let spec: ClipPlane = serde_json::from_value(
            json!({"line": {"projection": projection, "start": [0, -32], "end": [0, 32]}}),
        )
        .unwrap();
        let actual = spec.resolve(&session).unwrap();
        let expected = session
            .clip_line_plane(
                Projection::parse(projection).unwrap(),
                glam::DVec2::new(0.0, -32.0),
                glam::DVec2::new(0.0, 32.0),
            )
            .unwrap();
        assert_eq!(actual, expected);
    }
    for value in [
        json!({}),
        json!({"normal": [1, 0, 0]}),
        json!({"normal": [1, 0, 0], "distance": 0, "points": points}),
        json!({"points": [[0, 0, 0], [1, 1, 1], [2, 2, 2]]}),
        json!({"line": {"projection": "perspective", "start": [0, 0], "end": [32, 32]}}),
    ] {
        let spec: ClipPlane = serde_json::from_value(value).unwrap();
        assert!(spec.resolve(&session).is_err());
    }
}
