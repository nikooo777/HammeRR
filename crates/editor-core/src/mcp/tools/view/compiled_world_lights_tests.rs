use super::*;
use crate::session::compiled_world_lights::{Channel, Query, test_support};

#[test]
fn compiled_world_lights_mcp_matches_the_owned_read_only_session_query() {
    for invalid in [
        json!({"path":"foreign.bsp"}),
        json!({"offset":-1}),
        json!({"build_token":1}),
        json!({"exposure":2}),
        json!({"limit":"2"}),
    ] {
        assert!(serde_json::from_value::<InspectCompiledWorldLightsArgs>(invalid).is_err());
    }
    let schema =
        serde_json::to_value(schemars::schema_for!(InspectCompiledWorldLightsArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["path"].is_null());
    let (_directory, session, source) = test_support::compiled(true);
    let output = source.with_extension("bsp");
    let expected = session
        .inspect_compiled_world_lights(&Query::default())
        .unwrap();
    let expected = serde_json::to_value(expected).unwrap();
    assert_eq!(expected["build"]["status"], "current");
    assert_eq!(expected["page"]["total"], 4);
    let token = expected["build"]["token"].as_str().unwrap().to_owned();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.camera(),
        session.selected_ids(),
        session.compile_settings(),
        std::fs::read(&source).unwrap(),
        std::fs::read(&output).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let actual = server
        .inspect_compiled_world_lights(Parameters(serde_json::from_value(json!({})).unwrap()))
        .unwrap();
    assert!(!actual.is_error.unwrap_or(false));
    assert_eq!(actual.content.len(), 1);
    let actual: Value = serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, expected);
    let query = Query {
        build_token: Some(token.clone()),
        channel: Channel::Hdr,
        offset: 2,
        limit: 2,
    };
    let expected = lock(&shared).inspect_compiled_world_lights(&query).unwrap();
    let actual = server
        .inspect_compiled_world_lights(Parameters(
            serde_json::from_value(
                json!({"build_token":token,"channel":"hdr","offset":2,"limit":2}),
            )
            .unwrap(),
        ))
        .unwrap();
    assert!(!actual.is_error.unwrap_or(false));
    let actual: Value = serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, serde_json::to_value(expected).unwrap());
    assert_eq!(actual["page"]["rows"][0]["ordinal"], 2);
    for invalid in [
        json!({"offset":1}),
        json!({"build_token":"foreign"}),
        json!({"limit":65}),
        json!({"channel":"automatic"}),
    ] {
        assert!(
            server
                .inspect_compiled_world_lights(Parameters(serde_json::from_value(invalid).unwrap()))
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
            std::fs::read(&source).unwrap(),
            std::fs::read(&output).unwrap(),
        ),
        before
    );
}
