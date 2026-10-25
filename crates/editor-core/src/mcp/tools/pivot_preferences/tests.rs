use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn dispatched_pivot_defaults_share_human_settings_and_refuse_terminal_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().into()));
    let point = DVec3::new(1.125, -2.375, 3.0625);
    let custom = json!({"mode":"custom","point":[1.125,-2.375,3.0625]});
    session
        .set_transform_tool(
            Actor::User,
            crate::session::TransformTool {
                pivot: Some(point),
                ..Default::default()
            },
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    for (name, mut arguments, expected, report) in [
        (
            "get_transform_pivot_defaults",
            json!({}),
            Some(point),
            custom.clone(),
        ),
        (
            "set_transform_pivot_defaults",
            json!({"action":"save"}),
            Some(point),
            custom.clone(),
        ),
        (
            "set_transform_tool",
            json!({"pivot_mode":"world"}),
            Some(DVec3::ZERO),
            json!({"mode":"world_origin"}),
        ),
        (
            "set_transform_pivot_defaults",
            json!({"action":"reload"}),
            Some(DVec3::ZERO),
            json!({"mode":"world_origin"}),
        ),
        (
            "set_transform_pivot_defaults",
            json!({"action":"apply"}),
            Some(point),
            custom.clone(),
        ),
        (
            "set_transform_pivot_defaults",
            json!({"action":"reset"}),
            Some(point),
            custom.clone(),
        ),
        (
            "set_transform_pivot_defaults",
            json!({"action":"apply"}),
            None,
            json!({"mode":"selection_center"}),
        ),
    ] {
        if name == "set_transform_pivot_defaults" {
            arguments["snapshot"] = json!(lock(&shared).pivot_preferences_stamp());
        }
        let request =
            CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
        let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
            .await
            .unwrap()
            .unwrap()
        {
            CallToolResponse::Complete(result) => result,
            result => panic!("unexpected response: {result:?}"),
        };
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let value: Value =
            serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
        if name != "set_transform_tool" {
            assert_eq!(value["current"], report);
            assert_eq!(value["ready"], true);
            assert_eq!(
                value["write_committed"],
                if matches!(arguments["action"].as_str(), Some("save" | "reset")) {
                    json!(true)
                } else if name == "get_transform_pivot_defaults" {
                    Value::Null
                } else {
                    json!(false)
                }
            );
        }
        assert_eq!(lock(&shared).transform_tool().pivot, expected);
        assert_eq!(document.active_operations(), 0);
    }
    for invalid in [
        json!({}),
        json!({"action":"save"}),
        json!({"snapshot":""}),
        json!({"action":"unknown","snapshot":""}),
        json!({"action":"save","snapshot":"","point":[1,2,3]}),
    ] {
        assert!(serde_json::from_value::<PivotDefaultsArgs>(invalid).is_err());
    }
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    document.shutdown().unwrap();
    let before = lock(&shared).pivot_preferences_state();
    let bytes = std::fs::read(directory.path().join(crate::pivot_preferences::FILE_NAME)).unwrap();
    let request = CallToolRequestParams::new("set_transform_pivot_defaults").with_arguments(
        json!({"action":"reset","snapshot":before["snapshot"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let context = RequestContext::new(NumberOrString::Number(2), service.peer().clone());
    let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap()
    {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected response: {result:?}"),
    };
    assert_eq!(result.is_error, Some(true));
    assert_eq!(lock(&shared).pivot_preferences_state(), before);
    assert_eq!(
        std::fs::read(directory.path().join(crate::pivot_preferences::FILE_NAME)).unwrap(),
        bytes
    );
    tokio::time::timeout(TIMEOUT, service.cancel())
        .await
        .unwrap()
        .unwrap();
}
