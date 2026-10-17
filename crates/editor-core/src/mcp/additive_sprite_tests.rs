use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn additive_sprite_controls_and_inspection_share_temporary_resource_view_policy() {
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
