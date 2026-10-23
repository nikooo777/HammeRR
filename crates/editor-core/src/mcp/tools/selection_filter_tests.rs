use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn dispatched_filters_share_explicit_ids_and_reject_terminal_calls() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let selected = session
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
    let filters = json!({"world_brushes":false,"brush_entities":false,"point_entities":false});
    for (name, mut arguments) in [
        ("get_selection_filters", json!({})),
        ("set_selection_filters", filters.clone()),
        ("get_selection_defaults", json!({})),
        ("set_selection_defaults", json!({"action":"save"})),
        ("set_selection_defaults", json!({"action":"reload"})),
        ("set_selection_defaults", json!({"action":"apply"})),
        (
            "get_object_hit_candidates",
            json!({"origin":[0,0,128],"direction":[0,0,-1]}),
        ),
        ("select", json!({"ids":[id_string(selected)]})),
    ] {
        if name == "set_selection_defaults" {
            arguments["snapshot"] = json!(lock(&shared).selection_preferences_stamp());
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
        if name == "set_selection_filters" {
            assert_eq!(value["filters"], filters);
        }
        if name == "set_selection_defaults" {
            assert_eq!(value["saved"], filters);
            assert_eq!(value["current"], filters);
            assert_eq!(value["ready"], true);
        }
        if name == "get_object_hit_candidates" {
            assert_eq!(value["total"], 0);
        }
        assert_eq!(document.active_operations(), 0);
    }
    assert_eq!(lock(&shared).selected_ids(), [selected]);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(
        peer.selection_filters(),
        crate::session::selection_filters::SelectionFilters::default()
    );
    for invalid in [
        json!({"world_brushes":false}),
        json!({"world_brushes":false,"brush_entities":false,"point_entities":false,"hide":true}),
    ] {
        assert!(
            serde_json::from_value::<crate::session::selection_filters::SelectionFilters>(invalid)
                .is_err()
        );
    }
    for invalid in [
        json!({"action":"save"}),
        json!({"action":"unknown","snapshot":""}),
        json!({"action":"save","snapshot":"","filters":filters}),
    ] {
        assert!(serde_json::from_value::<SelectionDefaultsArgs>(invalid).is_err());
    }
    document.shutdown().unwrap();
    let before = lock(&shared).selection_filter_state();
    let request = CallToolRequestParams::new("set_selection_filters").with_arguments(
        json!({"world_brushes":true,"brush_entities":true,"point_entities":true})
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
    assert_eq!(lock(&shared).selection_filter_state(), before);
    let before = lock(&shared).selection_preferences_state();
    let request = CallToolRequestParams::new("set_selection_defaults").with_arguments(
        json!({"action":"reset","snapshot":before["snapshot"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let context = RequestContext::new(NumberOrString::Number(3), service.peer().clone());
    let result = match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap()
    {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected response: {result:?}"),
    };
    assert_eq!(result.is_error, Some(true));
    assert_eq!(lock(&shared).selection_preferences_state(), before);
    tokio::time::timeout(TIMEOUT, service.cancel())
        .await
        .unwrap()
        .unwrap();
}
