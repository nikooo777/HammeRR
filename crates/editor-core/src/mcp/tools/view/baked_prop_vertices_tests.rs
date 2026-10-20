use super::*;
use crate::session::baked_prop_vertices::{Query, test_support};

#[test]
fn baked_prop_vertices_mcp_schema_and_real_publication_match_read_only_session() {
    for invalid in [
        json!({"path":"foreign.bsp"}),
        json!({"prop":-1}),
        json!({"build_token":1}),
        json!({"vertex_limit":"2"}),
    ] {
        assert!(serde_json::from_value::<InspectBakedPropVerticesArgs>(invalid).is_err());
    }
    let schema = serde_json::to_value(schemars::schema_for!(InspectBakedPropVerticesArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["path"].is_null());
    let (directory, game, vmf) = test_support::fixture();
    let source = directory.path().join("maps/inspection.vmf");
    let output = directory.path().join("maps/inspection.bsp");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::Agent,
            crate::session::compilation::BuildSettingsUpdate {
                lighting: Some("direct_ldr_hdr"),
                bake_static_prop_vertices: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let completed = session
        .begin_compile(Actor::Agent, Some(&output))
        .unwrap()
        .run();
    session.finish_compile(Actor::Agent, &completed);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let expected = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(expected["build"]["status"], "current");
    assert_eq!(expected["selected"]["ordinal"], 1);
    let token = expected["build"]["token"].as_str().unwrap().to_owned();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.camera(),
        session.selected_ids(),
        session.compile_settings(),
        std::fs::read(&output).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let actual = server
        .inspect_baked_prop_vertices(Parameters(serde_json::from_value(json!({})).unwrap()))
        .unwrap();
    assert!(!actual.is_error.unwrap_or(false));
    assert_eq!(actual.content.len(), 1);
    let actual: Value = serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, expected);
    let args = json!({"build_token":token,"channel":"hdr","prop":1,"section":0,"vertex_offset":256,"vertex_limit":2});
    let actual = server
        .inspect_baked_prop_vertices(Parameters(serde_json::from_value(args).unwrap()))
        .unwrap();
    assert!(!actual.is_error.unwrap_or(false));
    let actual: Value = serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual["vertices"]["rows"][0]["index"], 256);
    assert_eq!(actual["vertices"]["returned"], 2);
    let expected = lock(&shared)
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token.clone()),
            channel: crate::session::baked_prop_vertices::Channel::Hdr,
            prop: Some(1),
            section: Some(0),
            vertex_offset: 256,
            vertex_limit: 2,
            ..Default::default()
        })
        .unwrap()
        .report;
    assert_eq!(actual, expected);
    for invalid in [
        json!({"prop":1}),
        json!({"build_token":"foreign","prop":1}),
        json!({"vertex_limit":257}),
        json!({"build_token":token,"prop":1,"section":99}),
    ] {
        assert!(
            server
                .inspect_baked_prop_vertices(Parameters(serde_json::from_value(invalid).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    assert!(!server.rendering_status()["initialized"].as_bool().unwrap());
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.selected_ids(),
            session.compile_settings(),
            std::fs::read(&output).unwrap()
        ),
        before
    );
}
