use super::*;
use crate::session::compiled_world_lights::{Channel, Query, test_support};

#[test]
fn compiled_light_framing_mcp_uses_explicit_owned_records_and_shared_camera_operation() {
    for invalid in [
        json!({}),
        json!({"build_token":"1","ordinal":0}),
        json!({"build_token":"1","channel":"ldr"}),
        json!({"build_token":"1","channel":"ldr","ordinal":-1}),
        json!({"build_token":"1","channel":"ldr","ordinal":0,"path":"foreign.bsp"}),
    ] {
        assert!(
            serde_json::from_value::<SelectCompiledWorldLightSourceArgs>(invalid.clone()).is_err()
        );
        assert!(serde_json::from_value::<FrameCompiledWorldLightArgs>(invalid).is_err());
    }
    let schema = serde_json::to_value(schemars::schema_for!(FrameCompiledWorldLightArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["path"].is_null());
    let (_directory, mut session, source) = test_support::compiled(true);
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let draft = session.property_draft().unwrap().token;
    session
        .append_property_change(
            draft,
            crate::session::PropertyChange::Set {
                key: "skyname".into(),
                value: "unapplied_frame".into(),
            },
        )
        .unwrap();
    let token = session
        .inspect_compiled_world_lights(&Query::default())
        .unwrap()
        .build["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        session.compile_settings(),
        serde_json::json!(session.property_draft()),
        std::fs::read(&source).unwrap(),
        std::fs::read(source.with_extension("bsp")).unwrap(),
    );
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert!(server.get_tool("frame_compiled_world_light").is_some());
    assert!(
        server
            .get_tool("select_compiled_world_light_source")
            .is_some()
    );
    let source_camera = lock(&shared).camera();
    for (channel, ordinal, entity) in [("ldr", 0, 9001), ("hdr", 1, 9002)] {
        let selected = server
            .select_compiled_world_light_source(Parameters(
                serde_json::from_value(
                    json!({"build_token":token,"channel":channel,"ordinal":ordinal}),
                )
                .unwrap(),
            ))
            .unwrap();
        assert!(!selected.is_error.unwrap_or(false));
        let selected: Value =
            serde_json::from_str(&selected.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(
            selected,
            json!({"target":format!("entity:{entity}"),"source_kind":"root_entity","mode":"objects","selected_count":1})
        );
        assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(entity)]);
        assert_eq!(lock(&shared).camera(), source_camera);
    }
    let source_selection = lock(&shared).selected_ids();
    for args in [
        json!({"build_token":token,"channel":"ldr","ordinal":2}),
        json!({"build_token":"wrong","channel":"ldr","ordinal":0}),
        json!({"build_token":token,"channel":"automatic","ordinal":0}),
    ] {
        let refused = server
            .select_compiled_world_light_source(Parameters(serde_json::from_value(args).unwrap()))
            .unwrap();
        assert!(refused.is_error.unwrap_or(false));
        assert_eq!(lock(&shared).selected_ids(), source_selection);
    }
    lock(&shared)
        .select(&before.2, crate::session::SelectMode::Replace)
        .unwrap();

    for (channel, name, ordinal) in [(Channel::Ldr, "ldr", 0), (Channel::Hdr, "hdr", 1)] {
        let expected = {
            let mut session = lock(&shared);
            let camera = session.camera();
            let frame = session
                .frame_compiled_world_light(Actor::Agent, token.clone(), channel, ordinal, 1.7)
                .unwrap();
            let expected = json!({"build":frame.inspection.build,"channel":frame.inspection.channel,"record":frame.inspection.page.rows[0],"framed_bounds":frame.bounds,"aspect":1.7f32,"camera":camera_json(&frame.camera),"notes":frame.inspection.notes});
            session.set_camera(camera);
            expected
        };
        let actual = server
            .frame_compiled_world_light(Parameters(
                serde_json::from_value(
                    json!({"build_token":token,"channel":name,"ordinal":ordinal,"aspect":1.7}),
                )
                .unwrap(),
            ))
            .unwrap();
        assert!(!actual.is_error.unwrap_or(false), "{actual:?}");
        let actual: Value =
            serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(actual, expected);
    }
    for invalid in [
        json!({"build_token":token,"channel":"hdr","ordinal":2}),
        json!({"build_token":"0","channel":"ldr","ordinal":0}),
        json!({"build_token":token,"channel":"automatic","ordinal":0}),
        json!({"build_token":token,"channel":"ldr","ordinal":4}),
        json!({"build_token":token,"channel":"ldr","ordinal":0,"aspect":0}),
    ] {
        let camera = lock(&shared).camera();
        let result = server
            .frame_compiled_world_light(Parameters(serde_json::from_value(invalid).unwrap()))
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(lock(&shared).camera(), camera);
    }
    {
        let mut session = lock(&shared);
        session
            .preview_block(DVec3::ZERO, DVec3::splat(32.0))
            .unwrap();
    }
    let camera = lock(&shared).camera();
    let result = server
        .frame_compiled_world_light(Parameters(
            serde_json::from_value(json!({"build_token":token,"channel":"ldr","ordinal":0}))
                .unwrap(),
        ))
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert_eq!(
        lock(&shared).block_preview(),
        Some((DVec3::ZERO, DVec3::splat(32.0)))
    );
    assert_eq!(lock(&shared).camera(), camera);
    lock(&shared).cancel_block_preview();
    std::fs::write(source.with_extension("bsp"), b"changed owned output").unwrap();
    let result = server
        .frame_compiled_world_light(Parameters(
            serde_json::from_value(json!({"build_token":token,"channel":"ldr","ordinal":0}))
                .unwrap(),
        ))
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).camera(), camera);
    std::fs::write(source.with_extension("bsp"), &before.6).unwrap();
    assert!(!server.rendering_status()["initialized"].as_bool().unwrap());
    let session = lock(&shared);
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.selected_ids(),
            session.compile_settings(),
            serde_json::json!(session.property_draft()),
            std::fs::read(&source).unwrap(),
            std::fs::read(source.with_extension("bsp")).unwrap()
        ),
        before
    );
}

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
    assert_eq!(
        expected["page"]["rows"][0]["source_association"],
        json!({"status":"available","origin":{"kind":"root","entity":9001}})
    );
    assert_eq!(
        expected["page"]["rows"][1]["source_association"],
        json!({"status":"available","origin":{"kind":"root","entity":9002}})
    );
    assert_eq!(
        expected["page"]["rows"][2]["source_association"]["status"],
        "unavailable"
    );
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
