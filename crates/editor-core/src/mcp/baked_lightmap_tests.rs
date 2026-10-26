use super::*;
use crate::session::baked_lightmaps::Query;

#[test]
fn baked_lightmap_mcp_matches_session_and_encodes_cpu_image_without_initializing_graphics() {
    for value in [
        json!({"path":"arbitrary.bsp"}),
        json!({"face":-1}),
        json!({"channel":1}),
        json!({"source":{"kind":"root","solid":1,"side":2,"path":"foreign.vmf"}}),
        json!({"source":{"kind":"instance","instance":1,"file":"child.vmf","solid":2}}),
        json!({"source":{"kind":"generated_region","solid":1,"side":2}}),
    ] {
        assert!(serde_json::from_value::<InspectBakedLightmapsArgs>(value).is_err());
    }
    let schema = serde_json::to_value(schemars::schema_for!(InspectBakedLightmapsArgs)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let (_directory, game, source, child_path) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    session
        .update_compile_settings(Actor::Agent, Some("direct_ldr"), Some("none"))
        .unwrap();
    let result = session.begin_compile(Actor::Agent, None).unwrap().run();
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let expected = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert!(expected.image.is_some());
    let history = session.history_snapshot();
    let camera = session.camera();
    let document = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let result = server
        .inspect_baked_lightmaps(Parameters(serde_json::from_value(json!({})).unwrap()))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let actual: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, expected.report);
    let image = result.content[1].as_image().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(server.rendering_status()["initialized"], false);
    let origin = &expected.report["mapping"]["contributors"][0];
    let source = if origin["kind"] == "instance" {
        json!({"kind":"instance","instance":origin["instance"],"file":origin["file"],"solid":origin["solid"],"side":origin["side"],"occurrence":origin["occurrence"]})
    } else {
        json!({"kind":"root","solid":origin["solid"],"side":origin["side"]})
    };
    let args = serde_json::from_value(json!({"source":source,"limit":1})).unwrap();
    let result = server.inspect_baked_lightmaps(Parameters(args)).unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let report: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    let expected_source = lock(&shared)
        .inspect_baked_lightmaps(&Query {
            source: Some(serde_json::from_value(source).unwrap()),
            limit: 1,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(report, expected_source.report);
    assert!(report["mapping"]["match_page"]["total"].as_u64().unwrap() > 0);
    let child =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(child_path).unwrap()).unwrap();
    let original = &child.world.solids[0];
    let owner = lock(&shared).document().vmf().entities.last().unwrap().id;
    let external = json!({"kind":"instance","instance":owner,"file":"child.vmf","solid":original.id,"side":original.sides[0].id,"occurrence":[{"helper":owner,"file":"child.vmf"}]});
    let expected = lock(&shared)
        .inspect_baked_lightmaps(&Query {
            source: Some(serde_json::from_value(external.clone()).unwrap()),
            ..Query::default()
        })
        .unwrap();
    let result = server
        .inspect_baked_lightmaps(Parameters(
            serde_json::from_value(json!({"source":external})).unwrap(),
        ))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let report: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(report, expected.report);
    assert!(report["mapping"]["match_page"]["total"].as_u64().unwrap() > 0);
    assert!(
        report["mapping"]["source_occurrence_label"]
            .as_str()
            .unwrap()
            .contains("child.vmf")
    );
    assert!(
        server
            .inspect_baked_lightmaps(Parameters(
                serde_json::from_value(
                    json!({"face":0,"source":{"kind":"root","solid":1,"side":2}})
                )
                .unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}
