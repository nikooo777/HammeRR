use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn typed_sky3d_tools_require_explicit_camera_and_preserve_source_without_graphics() {
    let mut session = Session::new(Err("fixture content unavailable".into()));
    let camera = session
        .create_entity(
            Actor::User,
            "sky_camera",
            Some(DVec3::new(64.0, 32.0, 16.0)),
            [
                ("scale".into(), "16".into()),
                ("targetname".into(), "miniature".into()),
            ]
            .into(),
        )
        .unwrap();
    let invalid = session
        .create_entity(
            Actor::User,
            "sky_camera",
            Some(DVec3::ZERO),
            [("scale".into(), "0".into())].into(),
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let selection = session.selected_ids();
    let view_camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let initial = value(server.get_3d_sky_preview());
    assert_eq!(initial["enabled"], false);
    assert!(initial["camera_entity"].is_null());
    assert_eq!(initial["ready"], false);
    assert_eq!(initial["cameras"].as_array().unwrap().len(), 2);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    for (arguments, success) in [
        (json!({"action":"settings","enabled":true}), false),
        (
            json!({"action":"settings","enabled":true,"camera":id_string(invalid)}),
            false,
        ),
        (
            json!({"action":"settings","enabled":false,"camera":id_string(camera)}),
            true,
        ),
        (
            json!({"action":"settings","enabled":true,"camera":id_string(camera)}),
            true,
        ),
        (json!({"action":"prepare","enabled":true}), false),
        (json!({"action":"cancel","background":true}), false),
    ] {
        let before = value(server.get_3d_sky_preview());
        let request = CallToolRequestParams::new("set_3d_sky_preview")
            .with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
        let response =
            tokio::time::timeout(Duration::from_secs(5), server.call_tool(request, context))
                .await
                .unwrap()
                .unwrap();
        let CallToolResponse::Complete(result) = response else {
            panic!("unexpected response")
        };
        assert_eq!(!result.is_error.unwrap_or(false), success, "{result:?}");
        let current = value(server.get_3d_sky_preview());
        if !success {
            assert_eq!(current["enabled"], before["enabled"]);
            assert_eq!(current["camera_entity"], before["camera_entity"]);
        }
        assert_eq!(current["ready"], false);
    }
    let prepared = server
        .set_3d_sky_preview(Parameters(
            serde_json::from_value(json!({"action":"prepare"})).unwrap(),
        ))
        .unwrap();
    let failed = value(server.get_3d_sky_preview());
    assert_eq!(failed["ready"], false);
    assert_ne!(failed["status"], "ready");
    assert!(prepared.is_error.unwrap_or(false) || failed["status"] == "failed");
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    {
        let state = lock(&shared);
        assert_eq!(state.document().vmf(), &source);
        assert_eq!(state.history_snapshot(), history);
        assert_eq!(state.selected_ids(), selection);
        assert_eq!(state.camera(), view_camera);
    }
    tokio::time::timeout(Duration::from_secs(5), service.cancel())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn sky3d_schema_rejects_unknown_actions_fields_and_wrong_value_types() {
    for arguments in [
        json!({"action":"nearest"}),
        json!({"action":"settings","enabled":"true"}),
        json!({"action":"settings","enabled":false,"camera":42}),
        json!({"action":"prepare","background":"true"}),
        json!({"action":"cancel","typo":true}),
    ] {
        assert!(serde_json::from_value::<Sky3dPreviewArgs>(arguments).is_err());
    }
    for arguments in [json!({"sky3d_preview":"true"}), json!({"sky3d_preview":42})] {
        assert!(serde_json::from_value::<SetViewArgs>(arguments.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(arguments).is_err());
    }
}
