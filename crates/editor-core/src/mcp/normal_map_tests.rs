use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn normal_map_controls_and_reports_share_lazy_scene_admission_without_editing_source() {
    let (directory, game) = crate::test_assets::game();
    std::fs::write(
        directory.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $bumpmap test/checker }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let object = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session
        .document()
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let camera = session.camera();
    session
        .preview_transform(
            Actor::User,
            &[object],
            map_core::edit::Transform {
                translation: DVec3::X * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let preview = session.render_document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let initial = value(server.get_normal_map_preview());
    assert_eq!(initial["enabled"], false);
    assert_eq!(initial["report"]["materials"], json!([]));
    for args in [json!({"normal_maps":1}), json!({"normal_maps":"true"})] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    assert_eq!(
        value(server.set_view(Parameters(
            serde_json::from_value(json!({"normal_maps":true})).unwrap()
        )))["normal_maps"],
        true
    );
    let admitted = value(server.get_normal_map_preview());
    assert_eq!(admitted["report"]["applied_faces"], 6);
    assert_eq!(admitted["report"]["materials"][0]["state"], "ready");
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    value(server.set_view(Parameters(
        serde_json::from_value(json!({"normal_maps":false})).unwrap(),
    )));
    let args: RenderArgs = serde_json::from_value(json!({"normal_maps":true})).unwrap();
    let override_view = lock(&shared)
        .resolve_view_update(args.view_update())
        .unwrap();
    assert!(override_view.normal_maps);
    assert!(!lock(&shared).view().normal_maps);
    assert_ne!(
        override_view.scene_key(0),
        lock(&shared).view().scene_key(0)
    );
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.camera(), camera);
    assert_eq!(
        session
            .document()
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        history
    );
}
