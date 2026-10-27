use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn instance_collapse_mcp_shares_owned_all_root_preview_cancel_apply_and_undo() {
    let (_directory, _game, parent, child) = crate::test_assets::instance_map(false);
    let (_assets, game) = crate::test_assets::model_lighting::game();
    let mut content =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    let mut light = source_formats::vmf::Entity::new(1000, "light");
    light.properties.set("origin", "0 0 96");
    light.properties.set("_light", "255 160 64 240");
    content.entities.push(light);
    let mut cube = source_formats::vmf::Entity::new(1001, "env_cubemap");
    cube.properties.set("origin", "0 0 80");
    cube.properties.set("cubemapsize", "1");
    cube.properties
        .set("sides", content.world.solids[0].sides[0].id.to_string());
    content.entities.push(cube);
    let mut prop = source_formats::vmf::Entity::new(1002, "prop_static");
    prop.properties.set("origin", "16 8 32");
    prop.properties.set("model", "models/test/receiver.mdl");
    prop.properties.set("solid", "0");
    prop.properties.set("skin", "1");
    content.entities.push(prop);
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
    assert_eq!(ready["helper_id_map"].as_array().unwrap().len(), 1);
    assert_eq!(ready["stats"]["cubemaps"], 1);
    assert_eq!(ready["stats"]["static_props"], 1);
    assert_eq!(ready["static_prop_id_map"].as_array().unwrap().len(), 1);
    assert_eq!(ready["entity_provenance"][2]["classname"], "prop_static");
    assert_eq!(ready["entity_provenance"][2]["emitted"], false);
    assert_eq!(ready["entity_provenance"][2]["compiler_handled"], true);
    assert_eq!(ready["entity_provenance"][1]["classname"], "env_cubemap");
    assert_eq!(ready["entity_provenance"][1]["emitted"], false);
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

#[test]
fn instance_collapse_mcp_selected_leading_scope_is_owned_frozen_and_retains_other_roots() {
    let (_directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let first = session.document().vmf().entities.last().unwrap().id;
    let retained = session
        .edit(Actor::User, "retained reference", |tx| {
            let id = tx.new_object_id()?;
            let mut reference = source_formats::vmf::Entity::new(id, "func_instance");
            reference.properties.set("file", "missing-unselected.vmf");
            tx.add_entity(reference.clone())?;
            Ok(reference)
        })
        .unwrap();
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let source = session.document().vmf().clone();
    let bytes = std::fs::read(&child).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let call =
        |args| server.set_instance_collapse(Parameters(serde_json::from_value(args).unwrap()));
    let initial = value(server.get_instance_collapse());
    let ready = value(call(
        json!({"action":"prepare","snapshot":initial["snapshot"],"scope":"selected_leading_references"}),
    ));
    assert_eq!(ready["scope"], "selected_leading_references");
    assert_eq!(ready["removed"], json!([format!("entity:{first}")]));
    assert_eq!(ready["selected"], ready["removed"]);
    assert_eq!(ready["stats"]["instances"], 1);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(
        lock(&shared)
            .render_document()
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.id == retained.id),
        Some(&retained)
    );
    for args in [
        json!({"action":"prepare","snapshot":ready["snapshot"],"scope":"selected_references"}),
        json!({"action":"apply","token":ready["token"],"scope":"all_visible_references"}),
        json!({"action":"review","token":ready["token"],"scope":"selected_leading_references"}),
    ] {
        assert!(serde_json::from_value::<InstanceCollapseArgs>(args).is_err());
    }
    let reviewed = lock(&shared)
        .review_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(reviewed["scope"], ready["scope"]);
    assert!(
        call(json!({"action":"apply","token":reviewed["token"]}))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let reviewed = value(call(json!({"action":"review","token":reviewed["token"]})));
    let shown = lock(&shared).render_document().vmf().clone();
    let applied = value(call(json!({"action":"apply","token":reviewed["token"]})));
    assert_eq!(applied["captured_preview"]["scope"], ready["scope"]);
    assert_eq!(applied["captured_preview"]["selected"], ready["selected"]);
    assert_eq!(lock(&shared).document().vmf(), &shown);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(std::fs::read(child).unwrap(), bytes);
}
