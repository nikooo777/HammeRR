use super::*;
use crate::session::baked_prop_vertices::{Query, test_support};

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn prop_leaf_bounds_tools_share_complete_unbaked_membership_and_atomic_refusals() {
    for invalid in [
        json!({"prop":-1}),
        json!({"prop":1,"build_token":1}),
        json!({"prop":1,"path":"foreign.bsp"}),
        json!({"prop":1,"leaf_limit":1}),
    ] {
        assert!(serde_json::from_value::<SetCompiledPropLeafBoundsArgs>(invalid).is_err());
    }
    let schema =
        serde_json::to_value(schemars::schema_for!(SetCompiledPropLeafBoundsArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["path"].is_null());
    let (directory, game, vmf) = test_support::leaf_membership_fixture();
    let source = directory.path().join("maps/leaf-membership.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::Agent,
            crate::session::compilation::BuildSettingsUpdate {
                lighting: Some("fullbright"),
                physics: Some("none"),
                bake_static_prop_vertices: Some(false),
                pack_static_prop_assets: Some(false),
                pack_sky_assets: Some(false),
                generate_reflections: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session.begin_compile(Actor::Agent, None).unwrap().run();
    session.finish_compile(Actor::Agent, &result);
    assert!(result.as_ref().unwrap().published());
    let inspection = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(inspection["bake"]["status"], "not_baked");
    let token = inspection["build"]["token"].as_str().unwrap().to_owned();
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.history_snapshot(),
        session.selected_ids(),
    );
    let expected = session
        .set_compiled_prop_leaf_bounds(Actor::User, Some(token.clone()), Some(1))
        .unwrap();
    assert!(expected["leaf_count"].as_u64().unwrap() > 1);
    session
        .set_compiled_prop_leaf_bounds(Actor::User, None, None)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for name in [
        "get_compiled_prop_leaf_bounds",
        "set_compiled_prop_leaf_bounds",
    ] {
        assert!(server.get_tool(name).is_some());
    }
    assert_eq!(
        value(server.get_compiled_prop_leaf_bounds()),
        json!({"requested":false,"displayed":false})
    );
    let actual = value(server.set_compiled_prop_leaf_bounds(Parameters(
        serde_json::from_value(json!({"build_token":token,"prop":1})).unwrap(),
    )));
    assert_eq!(actual, expected);
    assert_eq!(value(server.get_compiled_prop_leaf_bounds()), expected);
    let key = {
        let session = lock(&shared);
        session.render_scene_key(session.view())
    };
    for arguments in [
        json!({"prop":0}),
        json!({"build_token":"wrong","prop":1}),
        json!({"build_token":token,"prop":u32::MAX}),
        json!({"build_token":token}),
    ] {
        let result = server
            .set_compiled_prop_leaf_bounds(Parameters(serde_json::from_value(arguments).unwrap()))
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(value(server.get_compiled_prop_leaf_bounds()), expected);
        let session = lock(&shared);
        assert_eq!(session.render_scene_key(session.view()), key);
    }
    std::fs::write(source.with_extension("bsp"), b"replaced output").unwrap();
    let stale = value(server.get_compiled_prop_leaf_bounds());
    assert_eq!(stale["requested"], true);
    assert_eq!(stale["displayed"], false);
    assert!(stale["reason"].as_str().unwrap().contains("suppressed"));
    let result = server
        .set_compiled_prop_leaf_bounds(Parameters(
            serde_json::from_value(json!({"build_token":token,"prop":0})).unwrap(),
        ))
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert_eq!(value(server.get_compiled_prop_leaf_bounds()), stale);
    let cleared = value(server.set_compiled_prop_leaf_bounds(Parameters(
        serde_json::from_value(json!({"prop":null})).unwrap(),
    )));
    assert_eq!(cleared, json!({"requested":false,"displayed":false}));
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.camera(),
            session.history_snapshot(),
            session.selected_ids()
        ),
        before
    );
}
