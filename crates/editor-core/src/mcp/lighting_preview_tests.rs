use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn authored_lighting_mcp_is_lazy_strict_and_temporary_overrides_preserve_shared_reviews() {
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let block = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .create_entity(
            Actor::User,
            "light",
            Some(DVec3::Z * 64.0),
            BTreeMap::from([("_light".into(), "255 128 32 100".into())]),
        )
        .unwrap();
    session
        .preview_transform(
            Actor::User,
            &[block],
            edit::Transform {
                translation: DVec3::X * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(value(server.get_lighting_preview())["enabled"], false);
    for args in [
        json!({"lighting_preview":1}),
        json!({"lighting_preview":"true"}),
        json!({"lighting_quality":true}),
        json!({"lighting_exposure_ev":"1"}),
    ] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    let enabled = value(server.set_view(Parameters(serde_json::from_value(json!({"lighting_preview":true,"lighting_quality":"detail","lighting_exposure_ev":2})).unwrap())));
    assert_eq!(enabled["lighting_preview"], true);
    let report = value(server.get_lighting_preview());
    assert_eq!(report["report"]["admitted_lights"], 1);
    assert_eq!(report["report"]["admitted_receivers"], 6);
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let accepted = lock(&shared).view();
    for args in [
        json!({"lighting_quality":"invalid"}),
        json!({"lighting_exposure_ev":9}),
        json!({"lighting_quality":"fast","normal_maps":true}),
    ] {
        assert!(
            server
                .set_view(Parameters(serde_json::from_value(args.clone()).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(lock(&shared).view(), accepted);
        assert!(
            server
                .render_view(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(
            value(server.get_map_info())["rendering"]["initialized"],
            false
        );
    }
    let args: RenderArgs =
        serde_json::from_value(json!({"lighting_preview":false,"lighting_exposure_ev":-2}))
            .unwrap();
    let temporary = lock(&shared)
        .resolve_view_update(args.view_update())
        .unwrap();
    assert!(!temporary.lighting_preview);
    assert_ne!(temporary.scene_key(0), accepted.scene_key(0));
    assert_eq!(lock(&shared).view(), accepted);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn authored_lighting_mcp_render_override_reports_the_rendered_snapshot_and_restores_pixels() {
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let floor = session
        .create_block(
            Actor::User,
            DVec3::new(-32.0, -32.0, -8.0),
            DVec3::new(32.0, 32.0, 0.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::User,
            &[floor],
            edit::Transform {
                translation: DVec3::X * 8.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let mut args = json!({"width":65,"height":65,"camera":{"projection":"perspective","position":[0,0,96],"look_at":[0,0,0]},"selection_outlines":false});
    let off = rendered(&server, args.clone());
    args["lighting_preview"] = json!(true);
    args["lighting_exposure_ev"] = json!(2);
    let on = rendered(&server, args.clone());
    assert_ne!(off.0, on.0);
    assert_eq!(on.1["lighting_preview"]["report"]["admitted_lights"], 0);
    assert_eq!(on.1["lighting_preview"]["report"]["active"], true);
    assert_eq!(
        on.1["lighting_preview"]["report"]["source"]["render_revision"],
        lock(&shared).render_document().revision()
    );
    assert_eq!(on.0, rendered(&server, args.clone()).0);
    args["lighting_preview"] = json!(false);
    assert_eq!(off.0, rendered(&server, args.clone()).0);
    for surface in ["flat", "wireframe", "lightmap_density", "smoothing_groups"] {
        args["surface"] = json!(surface);
        let baseline = rendered(&server, args.clone());
        args["lighting_preview"] = json!(true);
        let lit = rendered(&server, args.clone());
        assert_eq!(baseline.0, lit.0);
        assert_eq!(lit.1["lighting_preview"]["report"]["active"], false);
        args["lighting_preview"] = json!(false);
    }
    let session = lock(&shared);
    assert!(!session.view().lighting_preview);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
}

fn rendered(server: &EditorServer, args: Value) -> (Vec<u8>, Value) {
    let result = server
        .render_view(Parameters(serde_json::from_value(args).unwrap()))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    let png = base64::engine::general_purpose::STANDARD
        .decode(&result.content[0].as_image().unwrap().data)
        .unwrap();
    let report = serde_json::from_str(&result.content[1].as_text().unwrap().text).unwrap();
    (png, report)
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn tool_terrain_temporary_visibility_rebuilds_retained_lighting_counts_and_pixels() {
    let (directory, game) = crate::test_assets::game();
    std::fs::write(
        directory.path().join("materials/test/terrain-tool.vmt"),
        "LightmappedGeneric { $basetexture test/checker %compilenodraw 1 }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::new(-32.0, -32.0, -8.0),
            DVec3::new(32.0, 32.0, 0.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    let face = map_core::surface::FaceId { solid, side };
    session.create_terrain(Actor::User, &[face], 2, 0).unwrap();
    session
        .edit(Actor::User, "mixed-parent tool terrain", |tx| {
            for side in &mut tx.solid_mut(solid)?.sides {
                assert!(side.material.eq_ignore_ascii_case("test/checker"));
                if side.id == face.side {
                    side.material = "test/terrain-tool".into();
                }
            }
            Ok(())
        })
        .unwrap();
    session.set_view(ViewOptions {
        grid_visible: false,
        ..session.view()
    });
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let view = session.view();
    let selection = session.selected_ids();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let mut args = json!({"width":65,"height":65,"camera":{"projection":"perspective","position":[0,0,96],"look_at":[0,0,0]},"selection_outlines":false,"lighting_preview":true,"tool_textures":true});
    let shown = rendered(&server, args.clone());
    args["tool_textures"] = json!(false);
    let hidden = rendered(&server, args.clone());
    args["tool_textures"] = json!(true);
    let restored = rendered(&server, args);
    assert_ne!(shown.0, hidden.0);
    assert_eq!(shown.0, restored.0);
    for (rendered, triangles) in [(&shown, 32), (&hidden, 0), (&restored, 32)] {
        let report = &rendered.1["lighting_preview"]["report"];
        assert_eq!(report["terrain"]["excluded_surfaces"], 1);
        assert_eq!(report["terrain"]["excluded_triangles"], triangles);
        assert_eq!(
            report["source"]["scene_serial"],
            shown.1["lighting_preview"]["report"]["source"]["scene_serial"]
        );
    }
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.view(), view);
    assert_eq!(session.selected_ids(), selection);
}
