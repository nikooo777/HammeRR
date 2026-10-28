use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, RunningService, serve_directly};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

async fn request(
    server: &EditorServer,
    service: &RunningService<rmcp::RoleServer, EditorServer>,
    name: &'static str,
    arguments: Value,
) -> Result<CallToolResult, ErrorData> {
    let request =
        CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
    let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
    match tokio::time::timeout(TIMEOUT, server.call_tool(request, context))
        .await
        .unwrap()?
    {
        CallToolResponse::Complete(result) => Ok(result),
        result => panic!("unexpected carve response: {result:?}"),
    }
}

fn value(result: CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn carve_policy_dispatch_preserves_defaults_and_retains_drafts_on_refusal() {
    let mut session = Session::new(Err("no assets needed".into()));
    let mut brushes = Vec::new();
    for (mins, maxs, material) in [
        (DVec3::ZERO, DVec3::splat(128.0), "brick/wall"),
        (DVec3::splat(16.0), DVec3::splat(80.0), "tools/toolsnodraw"),
        (DVec3::splat(48.0), DVec3::splat(112.0), "tools/toolsnodraw"),
    ] {
        let ObjectId::Solid(id) = session
            .create_block(Actor::User, mins, maxs, material, false)
            .unwrap()
        else {
            unreachable!()
        };
        brushes.push(id);
    }
    let [target, first, second] = brushes[..] else {
        unreachable!()
    };
    let source = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    for tool in ["preview_carve", "preview_carve_many"] {
        let schema = server.get_tool(tool).unwrap().input_schema;
        let schema = Value::Object((*schema).clone());
        assert!(schema.pointer("/properties/cut_material").is_some());
        for policy in [
            None,
            Some(json!({"policy":"target"})),
            Some(json!({"policy":"material","material":" brick/jamb "})),
        ] {
            let mut arguments = if tool == "preview_carve" {
                json!({"cutter":first,"targets":[target]})
            } else {
                json!({"cutters":[second,first],"targets":[target]})
            };
            arguments["stamp"] = json!(lock(&shared).carve_stamp());
            if let Some(policy) = &policy {
                arguments["cut_material"] = policy.clone();
            }
            let preview = value(request(&server, &service, tool, arguments).await.unwrap());
            let expected_policy = match policy {
                None => json!({"policy":"cutter"}),
                Some(policy) if policy["policy"] == "target" => policy,
                _ => json!({"policy":"material","material":"BRICK/JAMB"}),
            };
            assert_eq!(
                preview["preview"]["result"]["cut_material"],
                expected_policy
            );
            let retained = lock(&shared).carve_state();
            let draft = lock(&shared).render_document().vmf().clone();
            let history = lock(&shared).document().undo_labels().count();
            for invalid in [
                json!({"policy":"material","material":" "}),
                json!({"policy":"target","material":"BRICK/UNUSED"}),
                json!({"policy":"unknown"}),
                json!("target"),
            ] {
                let arguments = json!({"cutter":first,"targets":[target],"stamp":lock(&shared).carve_stamp(),"cut_material":invalid});
                let refusal = request(&server, &service, "preview_carve", arguments).await;
                assert!(refusal.is_err() || refusal.unwrap().is_error.unwrap_or(false));
                assert_eq!(lock(&shared).carve_state(), retained);
                assert_eq!(lock(&shared).render_document().vmf(), &draft);
                assert_eq!(lock(&shared).document().vmf(), &source);
                assert_eq!(lock(&shared).document().undo_labels().count(), history);
            }
            let committed = value(
                request(
                    &server,
                    &service,
                    "commit_carve",
                    json!({"token":preview["preview"]["token"]}),
                )
                .await
                .unwrap(),
            );
            assert_eq!(committed["cut_material"], expected_policy);
            assert_eq!(lock(&shared).document().vmf(), &draft);
            lock(&shared).undo(Actor::User).unwrap();
            assert_eq!(lock(&shared).document().vmf(), &source);
        }
    }
    tokio::time::timeout(TIMEOUT, service.cancel())
        .await
        .unwrap()
        .unwrap();
}
