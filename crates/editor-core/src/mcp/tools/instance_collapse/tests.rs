use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn instance_collapse_mcp_shares_owned_all_root_preview_cancel_apply_and_undo() {
    let (_directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut content =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    let mut light = source_formats::vmf::Entity::new(1000, "light");
    light.properties.set("origin", "0 0 96");
    light.properties.set("_light", "255 160 64 240");
    content.entities.push(light);
    std::fs::write(&child, content.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let bytes = std::fs::read(&child).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let call =
        |args| server.set_instance_collapse(Parameters(serde_json::from_value(args).unwrap()));
    let inspected = value(server.get_instance_collapse());
    let ready = value(call(
        json!({"action":"prepare","snapshot":inspected["snapshot"]}),
    ));
    assert_eq!(ready["scope"], "all_visible_references");
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["stats"]["point_lights"], 1);
    assert_eq!(ready["entity_id_map"].as_array().unwrap().len(), 1);
    assert_eq!(ready["entity_provenance"][0]["original_entity"], 1000);
    let token = ready["token"].as_u64().unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(
        lock(&shared)
            .apply_instance_collapse(Actor::User, token)
            .is_err()
    );
    for args in [
        json!({"action":"prepare","snapshot":ready["snapshot"],"ids":["entity:1"]}),
        json!({"action":"apply","token":token,"snapshot":"old"}),
        json!({"action":"cancel","token":token,"background":true}),
    ] {
        assert!(serde_json::from_value::<InstanceCollapseArgs>(args).is_err());
    }
    let reviewed = lock(&shared)
        .review_instance_collapse(Actor::User, token)
        .unwrap();
    assert!(
        call(json!({"action":"cancel","token":token}))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let human = reviewed["token"].as_u64().unwrap();
    let reviewed = value(call(json!({"action":"review","token":human})));
    value(call(json!({"action":"cancel","token":reviewed["token"]})));
    assert_eq!(lock(&shared).render_document().vmf(), &source);
    let snapshot = value(server.get_instance_collapse())["snapshot"].clone();
    let ready = value(call(json!({"action":"prepare","snapshot":snapshot})));
    let shown = lock(&shared).render_document().vmf().clone();
    let history = lock(&shared).document().undo_labels().count();
    let applied = value(call(json!({"action":"apply","token":ready["token"]})));
    assert_eq!(applied["applied"], true);
    assert_eq!(lock(&shared).document().vmf(), &shown);
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(std::fs::read(child).unwrap(), bytes);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
