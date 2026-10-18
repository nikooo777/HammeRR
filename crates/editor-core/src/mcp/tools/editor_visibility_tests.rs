use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn dispatched_editor_visibility_uses_exact_snapshots_and_admission_without_source_or_peer_changes()
 {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let id = session
        .create_block(
            Actor::User,
            DVec3::splat(-16.0),
            DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let peer = Session::new(Err("peer".into()));
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let server = EditorServer::for_document(document.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let listed = tokio::time::timeout(
        TIMEOUT,
        server.list_tools(
            None,
            RequestContext::new(NumberOrString::Number(100), service.peer().clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let tool = serde_json::to_value(
        listed
            .tools
            .iter()
            .find(|tool| tool.name == "set_editor_visibility")
            .unwrap(),
    )
    .unwrap();
    let schema = &tool["inputSchema"];
    assert_eq!(schema["type"], "object");
    let actions = schema["oneOf"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    for name in ["hide", "show_all"] {
        let action = actions
            .iter()
            .find(|action| action["properties"]["action"]["const"] == name)
            .unwrap();
        assert_eq!(action["type"], "object");
        assert_eq!(action["additionalProperties"], false);
        let required = action["required"].as_array().unwrap();
        assert!(required.contains(&json!("action")) && required.contains(&json!("snapshot")));
        assert_eq!(action["properties"].get("ids").is_some(), name == "hide");
        assert!(!required.contains(&json!("ids")));
    }
    for step in 0..7 {
        let snapshot = lock(&shared).working_region_stamp();
        let (name, arguments, error) = match step {
            0 => ("get_editor_visibility", json!({}), false),
            1 => (
                "set_editor_visibility",
                json!({"action":"hide","snapshot":snapshot}),
                false,
            ),
            2 => ("get_working_region", json!({}), false),
            3 => (
                "set_editor_visibility",
                json!({"action":"show_all","snapshot":"stale"}),
                true,
            ),
            4 => (
                "set_editor_visibility",
                json!({"action":"hide","snapshot":snapshot,"ids":[id_string(id),"entity:4294967295"]}),
                true,
            ),
            5 => ("select", json!({"ids":[id_string(id)]}), false),
            _ => (
                "set_editor_visibility",
                json!({"action":"show_all","snapshot":snapshot}),
                false,
            ),
        };
        let request =
            CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(step), service.peer().clone());
        let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
            .await
            .unwrap()
            .unwrap()
        {
            CallToolResponse::Complete(result) => result,
            result => panic!("unexpected response: {result:?}"),
        };
        assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
        if step == 1 {
            let value: Value =
                serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
            assert_eq!(value["hidden_total"], 1);
            assert_eq!(value["hidden"], json!([id_string(id)]));
        }
        if step == 2 {
            let value: Value =
                serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
            assert_eq!(value["objects"][0]["reasons"], json!(["editor_hidden"]));
        }
        assert_eq!(document.active_operations(), 0);
    }
    assert_eq!(lock(&shared).editor_hidden_count(), 0);
    assert_eq!(peer.editor_hidden_count(), 0);
    assert_eq!(lock(&shared).selected_ids(), [id]);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    for invalid in [
        json!({"action":"show_all","snapshot":"x","ids":[]}),
        json!({"action":"hide"}),
        json!({"action":"hide","snapshot":"x","saved":true}),
    ] {
        assert!(serde_json::from_value::<EditorVisibilitySetArgs>(invalid).is_err());
    }
    document.shutdown().unwrap();
    let request=CallToolRequestParams::new("set_editor_visibility").with_arguments(json!({"action":"hide","snapshot":lock(&shared).working_region_stamp(),"ids":[id_string(id)]}).as_object().unwrap().clone());
    let context = RequestContext::new(NumberOrString::Number(20), service.peer().clone());
    let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap()
    {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected response: {result:?}"),
    };
    assert_eq!(result.is_error, Some(true));
    assert_eq!(lock(&shared).editor_hidden_count(), 0);
    tokio::time::timeout(TIMEOUT, service.cancel())
        .await
        .unwrap()
        .unwrap();
}
