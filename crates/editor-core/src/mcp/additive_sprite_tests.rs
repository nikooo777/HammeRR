use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn fixture() -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    std::fs::write(directory.path().join("materials/test/sprite.vmt"),"Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" }").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(
            r#"@PointClass size(4 4 4) sprite() = env_sprite [
model(sprite) : "Sprite" : "materials/test/sprite.vmt"
scale(string) : "Scale" : "1"
rendermode(integer) : "Mode" : 0
rendercolor(color255) : "Color" : "255 255 255"
renderamt(integer) : "Amount" : 255
renderfx(integer) : "FX" : 0
]"#,
        )
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::Agent,
            "env_sprite",
            Some(DVec3::ZERO),
            std::collections::BTreeMap::from([("rendermode".into(), "5".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    (directory, session, id)
}

#[test]
fn additive_sprite_controls_and_inspection_share_temporary_resource_view_policy() {
    let (_directory, mut session, id) = fixture();
    let source = session.document().vmf().clone();
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(
        view_state(lock(&shared).view())["additive_sprite_guides"],
        false
    );
    for args in [
        json!({"additive_sprite_guides":1}),
        json!({"additive_sprite_guides":"true"}),
    ] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    let enabled = value(server.set_view(Parameters(
        serde_json::from_value(json!({"additive_sprite_guides":true})).unwrap(),
    )));
    assert_eq!(enabled["additive_sprite_guides"], true);
    {
        let session = lock(&shared);
        let guide = session
            .entity_helper_report(id)
            .unwrap()
            .sprite_resource
            .unwrap();
        assert!(guide.blend_preview.applied);
        let args: RenderArgs =
            serde_json::from_value(json!({"additive_sprite_guides":false})).unwrap();
        let view = session.resolve_view_update(args.view_update()).unwrap();
        assert!(!view.additive_sprite_guides);
        assert_ne!(view.scene_key(0), session.view().scene_key(0));
        let report = session.sprite_guide_preview_state(view);
        assert_eq!(report["requested"], false);
        assert_eq!(
            report["guides"][0]["guide"]["blend_preview"]["effective_policy"],
            "raw_rgba"
        );
        assert!(session.view().additive_sprite_guides);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.property_draft().unwrap().token, token);
    }
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn additive_sprite_render_reports_retain_uploaded_scene_across_later_edits() {
    let (directory, mut session, id) = fixture();
    let texture = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    for pixel in bytes[80..].as_chunks_mut::<4>().0.iter_mut() {
        pixel.copy_from_slice(&[0, 0, 0, 255]);
    }
    std::fs::write(texture, bytes).unwrap();
    session.reload_material_assets(Actor::Agent);
    session.select(&[], SelectMode::Replace).unwrap();
    session
        .set_spatial(
            Actor::Agent,
            crate::spatial::Update {
                grid_visible: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let shared_view = session.view();
    let view = session
        .resolve_view_update(crate::session::ViewUpdate {
            additive_sprite_guides: Some(true),
            selection_outlines: Some(false),
            ..Default::default()
        })
        .unwrap();
    let camera = Camera::looking_at(Vec3::Z * 96.0, Vec3::ZERO);
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let rendered = server.render_png(&camera, (65, 65), view).unwrap();
    let retained = rendered.sprite_guides.clone();
    assert_eq!(retained["requested"], true);
    assert_eq!(retained["entity_images"], true);
    assert_eq!(retained["total"], 1);
    assert_eq!(retained["guides"][0]["id"], id);
    assert_eq!(
        retained["guides"][0]["guide"]["blend_preview"]["effective_policy"],
        "authored_additive"
    );
    let cached = server.render_png(&camera, (65, 65), view).unwrap();
    assert_eq!(cached.sprite_guides, retained);
    assert_eq!(cached.png, rendered.png);

    lock(&shared)
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("renderamt".into(), Some("128".into()))],
        )
        .unwrap();
    let inspector = value(server.get_entity_inspector(Parameters(
        serde_json::from_value(json!({"id":format!("entity:{id}")})).unwrap(),
    )));
    assert_eq!(
        inspector["helpers"]["sprite_resource"]["blend_preview"]["renderamt"]["raw"],
        "128"
    );
    assert_eq!(
        rendered.sprite_guides["guides"][0]["guide"]["blend_preview"]["renderamt"]["raw"],
        "255"
    );
    assert_eq!(rendered.sprite_guides, retained);
    assert_eq!(
        server
            .offscreen
            .lock()
            .unwrap()
            .renderer
            .as_ref()
            .unwrap()
            .sprite_guides,
        retained
    );
    assert_ne!(
        rendered.sprite_guides,
        lock(&shared).sprite_guide_preview_state(view)
    );
    let changed = server.render_png(&camera, (65, 65), view).unwrap();
    assert_eq!(
        changed.sprite_guides["guides"][0]["guide"]["blend_preview"]["effective_policy"],
        "raw_rgba"
    );
    assert_eq!(
        changed.sprite_guides["guides"][0]["guide"]["blend_preview"]["applied"],
        false
    );
    assert_ne!(changed.png, rendered.png);

    for (entity_images, additive_sprite_guides) in [(true, true), (true, false), (false, true)] {
        let result = server
            .render_view(Parameters(
                serde_json::from_value(json!({"width":65,"height":65,"camera":{"position":[0,0,96],"look_at":[0,0,0]},"selection_outlines":false,"entity_images":entity_images,"additive_sprite_guides":additive_sprite_guides})).unwrap(),
            ))
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
        let report: Value =
            serde_json::from_str(&result.content[1].as_text().unwrap().text).unwrap();
        assert_eq!(report["sprite_guides"]["requested"], additive_sprite_guides);
        assert_eq!(report["sprite_guides"]["entity_images"], entity_images);
        let effective = {
            let session = lock(&shared);
            session
                .resolve_view_update(crate::session::ViewUpdate {
                    entity_images: Some(entity_images),
                    additive_sprite_guides: Some(additive_sprite_guides),
                    selection_outlines: Some(false),
                    ..Default::default()
                })
                .unwrap()
        };
        let expected = server.render_png(&camera, (65, 65), effective).unwrap();
        assert_eq!(report["sprite_guides"], expected.sprite_guides);
        assert_eq!(lock(&shared).view(), shared_view);
    }

    lock(&shared)
        .delete_objects(Actor::Agent, &[ObjectId::Entity(id)])
        .unwrap();
    assert!(lock(&shared).entity_inspector(id).is_err());
    assert_eq!(changed.sprite_guides["guides"][0]["id"], id);
    assert_eq!(changed.sprite_guides["total"], 1);
    assert_eq!(rendered.sprite_guides, retained);
    let deleted = server.render_png(&camera, (65, 65), view).unwrap();
    assert_eq!(deleted.sprite_guides["total"], 0);
    assert_eq!(deleted.sprite_guides["guides"], json!([]));
}
