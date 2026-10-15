use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn stopping_component_picking_succeeds_for_large_selected_owners() {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for index in 0..17 {
        let origin = DVec3::X * (index as f64 * 128.0);
        ids.push(
            session
                .create_block(
                    Actor::Agent,
                    origin,
                    origin + DVec3::splat(64.0),
                    "brick/test",
                    false,
                )
                .unwrap(),
        );
    }
    let owner = session
        .tie_to_entity(Actor::Agent, &ids, "func_detail", Default::default())
        .unwrap();
    session.select(&[owner], SelectMode::Replace).unwrap();
    let ObjectId::Solid(id) = ids[0] else {
        unreachable!()
    };
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for enabled in [true, false, false] {
        let snapshot = lock(&shared).component_stamp();
        let report = value(
            server.set_component_picking(Parameters(
                serde_json::from_value(
                    json!({"enabled":enabled,"kind":"face","solids":[id],"snapshot":snapshot}),
                )
                .unwrap(),
            )),
        );
        assert_eq!(report["picking"].is_null(), !enabled);
        assert_eq!(
            report["brushes"].as_array().unwrap().len(),
            usize::from(enabled)
        );
        assert_eq!(lock(&shared).document().vmf(), &original);
    }
    let sources = value(
        server.get_component_sources(Parameters(ComponentSourcesArgs {
            offset: None,
            limit: None,
        })),
    );
    assert_eq!(sources["total"], 17);
    assert_eq!(sources["active_solids"].as_array().unwrap().len(), 17);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn component_weld_tools_report_only_edited_children_in_a_large_entity() {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for index in 0..17 {
        let origin = DVec3::X * (index as f64 * 128.0);
        ids.push(
            session
                .create_block(
                    Actor::Agent,
                    origin,
                    origin + DVec3::splat(64.0),
                    "brick/test",
                    false,
                )
                .unwrap(),
        );
    }
    let owner = session
        .tie_to_entity(Actor::Agent, &ids, "func_detail", Default::default())
        .unwrap();
    let ObjectId::Solid(id) = ids[0] else {
        unreachable!()
    };
    let shape = session.component_topology(&[id]).unwrap().remove(0);
    let selected = shape
        .vertices
        .iter()
        .filter(|vertex| vertex.position.z == 64.0)
        .map(|vertex| map_core::subobject::Component {
            solid: id,
            sides: vertex.sides.clone(),
        })
        .collect::<Vec<_>>();
    session
        .select_components(&selected, SelectMode::Replace, &session.component_stamp())
        .unwrap();
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for commit in [false, true] {
        let snapshot = lock(&shared).component_stamp();
        let preview = value(server.preview_component_weld(Parameters(ComponentWeldArgs {
            snapshot,
            texture_lock: None,
            target: None,
            target_corner: None,
        })));
        assert_eq!(preview["brushes"].as_array().unwrap().len(), 1);
        let token = preview["preview"]["token"].as_str().unwrap().to_string();
        let result = if commit {
            server.commit_component_edit(Parameters(VolumeTokenArgs { token }))
        } else {
            server.cancel_component_edit(Parameters(VolumeTokenArgs { token }))
        };
        assert_eq!(value(result)["brushes"].as_array().unwrap().len(), 1);
        assert_eq!(lock(&shared).document().owner_of_solid(id), Some(owner));
        if commit {
            lock(&shared).undo(Actor::Agent).unwrap();
        }
        assert_eq!(lock(&shared).document().vmf(), &original);
    }
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn headless_map_tools_and_status_do_not_initialize_graphics() {
    let session = Arc::new(Mutex::new(Session::new(Err("no game needed".into()))));
    let server = EditorServer::new(session.clone(), None);
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let result = value(
        server.create_block(Parameters(
            serde_json::from_value(json!({
                "mins": [0,0,0], "maxs": [64,64,64], "material": "brick/test"
            }))
            .unwrap(),
        )),
    );
    let id = result["id"].as_str().unwrap();
    let objects = value(server.get_objects(Parameters(
        serde_json::from_value(json!({"ids": [id]})).unwrap(),
    )));
    assert_eq!(objects["objects"][0]["faces"].as_array().unwrap().len(), 6);
    assert_eq!(value(server.get_map_info())["world_brushes"], 1);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    let output = value(server.undo(Parameters(serde_json::from_value(json!({})).unwrap())));
    assert!(output.is_object());
    assert_eq!(lock(&session).document().objects().count(), 0);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
