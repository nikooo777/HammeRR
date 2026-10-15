use super::*;

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
