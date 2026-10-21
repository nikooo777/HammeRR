use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn terrain(tool: bool) -> (tempfile::TempDir, Session) {
    let (directory, game) = crate::test_assets::game();
    let texture = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    for pixel in bytes[80..].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[128, 128, 128, 255]);
    }
    std::fs::write(texture, bytes).unwrap();
    if tool {
        std::fs::write(
            directory.path().join("materials/test/checker.vmt"),
            "LightmappedGeneric { $basetexture test/checker %compilenodraw 1 }",
        )
        .unwrap();
    }
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::new(-64.0, -64.0, -8.0),
            DVec3::new(64.0, 64.0, 0.0),
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
    session
        .create_terrain(
            Actor::User,
            &[map_core::surface::FaceId { solid, side }],
            3,
            0,
        )
        .unwrap();
    session
        .edit(Actor::User, "ridge", |tx| {
            let solid = tx.solid_mut(solid)?;
            let geometry = map_core::BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| solid.sides[face.side].id == side)
                .unwrap();
            let info = solid.sides[face.side].displacement.as_mut().unwrap();
            let mesh = map_core::DisplacementMesh::build(face, info).unwrap();
            info.distances = Some(
                mesh.base_positions
                    .chunks(mesh.size)
                    .map(|row| {
                        row.iter()
                            .map(|point| 32.0 * (1.0 - point.x.abs() / 64.0))
                            .collect()
                    })
                    .collect(),
            );
            Ok(())
        })
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    session.set_view(ViewOptions {
        grid_visible: false,
        ..session.view()
    });
    (directory, session)
}

#[test]
fn surface_shading_is_lazy_atomic_and_preserves_review_and_world_space_terrain_normals() {
    let (_directory, mut session) = terrain(false);
    let mesh = session.build_scene();
    assert_eq!(mesh.stats.displacements, 1);
    for direction in [-1.0, 1.0] {
        assert!(
            mesh.vertices
                .iter()
                .any(|vertex| { vertex.normal[0] * direction > 0.3 && vertex.normal[2] > 0.8 })
        );
    }
    let object = ObjectId::Solid(session.document().vmf().world.solids[0].id);
    session
        .preview_transform(
            Actor::User,
            &[object],
            edit::Transform {
                translation: DVec3::X * 8.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for args in [
        json!({"surface_shading":1}),
        json!({"surface_shading":"true"}),
    ] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    assert!(!lock(&shared).view().surface_shading);
    let enabled = value(server.set_view(Parameters(
        serde_json::from_value(json!({"surface_shading":true})).unwrap(),
    )));
    assert_eq!(enabled["surface_shading"], true);
    let accepted = lock(&shared).view();
    let invalid = server
        .set_view(Parameters(
            serde_json::from_value(json!({"surface_shading":false,"smoothing_group":0})).unwrap(),
        ))
        .unwrap();
    assert!(invalid.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).view(), accepted);
    let args: RenderArgs = serde_json::from_value(json!({"surface_shading":false})).unwrap();
    let temporary = lock(&shared)
        .resolve_view_update(args.view_update())
        .unwrap();
    assert!(!temporary.surface_shading);
    assert_eq!(temporary.scene_key(0), accepted.scene_key(0));
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let session = lock(&shared);
    assert_eq!(session.view(), accepted);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.history_snapshot(), history);
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
    let decoder = png::Decoder::new(Cursor::new(png));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (129, 129));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    (pixels, report)
}

fn render_args() -> Value {
    json!({"width":129,"height":129,"camera":{"projection":"top","center":[0,0,16],"ortho_height":160},"selection_outlines":false,"surface_shading":false})
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn surface_shading_terrain_oracle_increases_relief_restores_pixels_and_keeps_authored_overrides() {
    let (_directory, session) = terrain(false);
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let source = server.session().document().vmf().clone();
    let history = server.session().history_snapshot();
    let camera = server.session().camera();
    let view = server.session().view();
    let mut args = render_args();
    let ordinary = rendered(&server, args.clone());
    args["surface_shading"] = json!(true);
    let shaded = rendered(&server, args.clone());
    assert_eq!(ordinary.1["view"]["surface_shading"], false);
    assert_eq!(shaded.1["view"]["surface_shading"], true);
    let contrast = |pixels: &[u8]| {
        (i32::from(pixels[(64 * 129 + 38) * 4]) - i32::from(pixels[(64 * 129 + 90) * 4])).abs()
    };
    assert!(contrast(&shaded.0) > contrast(&ordinary.0) + 5);
    args["surface_shading"] = json!(false);
    assert_eq!(ordinary.0, rendered(&server, args.clone()).0);
    for surface in [
        "textured",
        "flat",
        "wireframe",
        "lightmap_density",
        "smoothing_groups",
    ] {
        args["surface"] = json!(surface);
        args["lighting_preview"] = json!(true);
        args["surface_shading"] = json!(false);
        let off = rendered(&server, args.clone());
        args["surface_shading"] = json!(true);
        let on = rendered(&server, args.clone());
        assert_eq!(off.0, on.0, "{surface}");
        if surface == "textured" {
            assert_eq!(
                on.1["lighting_preview"]["report"]["terrain"]["admitted_surfaces"],
                1
            );
        }
    }
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.view(), view);
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn surface_shading_preserves_tool_terrain_and_entity_helper_pixels() {
    let (_directory, session) = terrain(true);
    let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
    let mut args = render_args();
    let ordinary = rendered(&server, args.clone());
    args["surface_shading"] = json!(true);
    assert_eq!(ordinary.0, rendered(&server, args.clone()).0);
    let mut session = Session::new(Err("helper control needs no assets".into()));
    session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap();
    session.set_view(ViewOptions {
        grid_visible: false,
        ..session.view()
    });
    let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
    args["surface_shading"] = json!(false);
    let ordinary = rendered(&server, args.clone());
    args["surface_shading"] = json!(true);
    assert_eq!(ordinary.0, rendered(&server, args).0);
}
