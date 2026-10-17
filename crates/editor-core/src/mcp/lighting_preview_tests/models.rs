use super::*;

fn fixture() -> (tempfile::TempDir, EditorServer, ObjectId) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    let mut session = Session::new(Ok(game));
    let model = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::ZERO),
            BTreeMap::from([("model".into(), "models/test/receiver.mdl".into())]),
        )
        .unwrap();
    session.set_view(ViewOptions {
        grid_visible: false,
        ..session.view()
    });
    let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
    (directory, server, model)
}

#[test]
fn model_receiver_mcp_inspection_remains_lazy_and_preserves_owned_geometry_reviews() {
    let (_directory, server, model) = fixture();
    server
        .session()
        .preview_transform(
            Actor::User,
            &[model],
            edit::Transform {
                translation: DVec3::new(8., 4., 2.),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = server.session().document().vmf().clone();
    let preview = server.session().render_document().vmf().clone();
    let history = server.session().history_snapshot();
    let camera = server.session().camera();
    assert_eq!(
        value(server.get_lighting_preview())["report"]["models"]["admitted_instances"],
        0
    );
    value(
        server.set_view(Parameters(
            serde_json::from_value(json!({"lighting_preview":true,"lighting_quality":"detail"}))
                .unwrap(),
        )),
    );
    let report = value(server.get_lighting_preview());
    assert_eq!(report["report"]["models"]["admitted_instances"], 1);
    assert_eq!(report["report"]["models"]["admitted_parts"], 1);
    assert_eq!(report["report"]["models"]["admitted_triangles"], 2);
    assert!(
        report["summary"]
            .as_str()
            .unwrap()
            .contains("model instances")
    );
    assert_eq!(
        report["source"]["render_revision"],
        server.session().render_document().revision()
    );
    assert_eq!(server.session().document().vmf(), &source);
    assert_eq!(server.session().render_document().vmf(), &preview);
    assert_eq!(server.session().history_snapshot(), history);
    assert_eq!(server.session().camera(), camera);
    assert!(
        !value(server.get_map_info())["rendering"]["initialized"]
            .as_bool()
            .unwrap()
    );
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn model_receiver_mcp_retains_actual_uploaded_counts_across_temporary_visibility_cache_and_reload()
{
    let (directory, server, _model) = fixture();
    let source = server.session().document().vmf().clone();
    let history = server.session().history_snapshot();
    let shared_view = server.session().view();
    let camera = server.session().camera();
    let args = json!({"width":65,"height":65,"camera":{"projection":"top","center":[0,0,0],"ortho_height":65},"selection_outlines":false,"lighting_preview":false});
    let off = rendered(&server, args.clone());
    let mut on_args = args.clone();
    on_args["lighting_preview"] = json!(true);
    let on = rendered(&server, on_args.clone());
    assert_ne!(off.0, on.0);
    assert_eq!(
        on.1["lighting_preview"]["report"]["models"]["admitted_triangles"],
        2
    );
    assert_eq!(on.0, rendered(&server, on_args.clone()).0);
    assert_eq!(off.0, rendered(&server, args.clone()).0);
    assert_eq!(
        value(server.get_lighting_preview())["report"]["models"]["admitted_triangles"],
        0
    );
    assert_eq!(
        on.1["lighting_preview"]["report"]["models"]["admitted_triangles"],
        2
    );
    let material = directory.path().join("materials/models/test/one.vmt");
    std::fs::write(
        &material,
        "VertexLitGeneric { $basetexture test/checker %compilenodraw 1 }",
    )
    .unwrap();
    assert_eq!(rendered(&server, on_args.clone()).0, on.0);
    value(server.reload_material_assets());
    let mut sequence = Vec::new();
    for tools in [true, false, true] {
        on_args["tool_textures"] = json!(tools);
        let frame = rendered(&server, on_args.clone());
        let report = &frame.1["lighting_preview"]["report"];
        assert_eq!(report["models"]["admitted_instances"], 0);
        assert_eq!(report["models"]["excluded_instances"], 1);
        assert_eq!(
            report["models"]["excluded_triangles"],
            if tools { 2 } else { 0 }
        );
        assert_eq!(frame.0, rendered(&server, on_args.clone()).0);
        assert_eq!(
            report,
            &rendered(&server, on_args.clone()).1["lighting_preview"]["report"]
        );
        sequence.push(frame);
    }
    assert_ne!(sequence[0].0, sequence[1].0);
    assert_eq!(sequence[0].0, sequence[2].0);
    assert_eq!(server.session().document().vmf(), &source);
    assert_eq!(server.session().history_snapshot(), history);
    assert_eq!(server.session().view(), shared_view);
    assert_eq!(server.session().camera(), camera);
}
