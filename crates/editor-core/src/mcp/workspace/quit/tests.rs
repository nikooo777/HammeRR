use super::*;
use crate::runtime::DocumentLifecycle;
use crate::session::PropertyTarget;
use rmcp::ServerHandler;
use rmcp::model::{CallToolRequestParams, CallToolResponse, RequestId};
use rmcp::service::serve_directly;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const TIMEOUT: Duration = Duration::from_secs(15);

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("Quit all fixture timed out")
}

#[tokio::test]
async fn actual_stdio_quit_reviews_all_drafts_rejects_stale_and_delivers_terminal_result_before_eof()
 {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No assets needed".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let a = application.document();
    lock(a.session())
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let created = workspace.new_document(Actor::Agent, false).await.unwrap();
    let b = application
        .workspace()
        .resolve(created["document_id"].as_str())
        .unwrap()
        .document();
    lock(b.session())
        .begin_property_edit(PropertyTarget::World)
        .unwrap();
    let (service_io, client) = tokio::io::duplex(256 * 1024);
    let (read, write) = tokio::io::split(service_io);
    let service = tokio::spawn(run_workspace_stdio(owner.server(), read, write));
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut lines = BufReader::new(client_read).lines();
    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"quit-fixture","version":"1"}}});
    bounded(client_write.write_all(format!("{initialize}\n").as_bytes()))
        .await
        .unwrap();
    bounded(lines.next_line()).await.unwrap().unwrap();
    bounded(
        client_write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"),
    )
    .await
    .unwrap();
    let requests = [
        (1, "get_quit_review", json!({})),
        (
            2,
            "quit_all",
            json!({"review_token":workspace.quit_review().unwrap().token(),"discard_changes":false}),
        ),
        (3, "get_quit_review", json!({})),
    ];
    let mut token = Value::Null;
    for (id, name, arguments) in requests {
        let request = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
        bounded(client_write.write_all(format!("{request}\n").as_bytes()))
            .await
            .unwrap();
        let response: Value =
            serde_json::from_str(&bounded(lines.next_line()).await.unwrap().unwrap()).unwrap();
        assert_eq!(response["id"], id);
        if name == "quit_all" {
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(a.lifecycle(), DocumentLifecycle::Live);
            assert_eq!(b.lifecycle(), DocumentLifecycle::Live);
        } else {
            let report: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(report["discard_required"], true);
            assert_eq!(report["documents"].as_array().unwrap().len(), 2);
            token = report["review_token"].clone();
        }
    }
    lock(b.session())
        .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(16.0))
        .unwrap();
    let stale = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"quit_all","arguments":{"review_token":token,"discard_changes":true}}});
    bounded(client_write.write_all(format!("{stale}\n").as_bytes()))
        .await
        .unwrap();
    let response: Value =
        serde_json::from_str(&bounded(lines.next_line()).await.unwrap().unwrap()).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(a.lifecycle(), DocumentLifecycle::Live);
    assert_eq!(b.lifecycle(), DocumentLifecycle::Live);
    assert!(lock(b.session()).block_preview().is_some());
    let token = workspace.quit_review().unwrap().token();
    let quit = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"quit_all","arguments":{"review_token":token,"discard_changes":true}}});
    bounded(client_write.write_all(format!("{quit}\n").as_bytes()))
        .await
        .unwrap();
    let response: Value =
        serde_json::from_str(&bounded(lines.next_line()).await.unwrap().unwrap()).unwrap();
    assert_eq!(response["id"], 5);
    assert_eq!(response["result"]["structuredContent"]["complete"], true);
    assert_eq!(response["result"]["structuredContent"]["terminal"], true);
    assert_eq!(
        response["result"]["structuredContent"]["documents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(response["result"]["_meta"]["io.hammerr/terminal-attempt"].is_string());
    bounded(service).await.unwrap().unwrap();
    assert!(bounded(lines.next_line()).await.unwrap().is_none());
    assert_eq!(a.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(b.lifecycle(), DocumentLifecycle::Finished);
    assert!(application.workspace().documents().is_empty());
    drop(client_write);
    owner.shutdown().unwrap();
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn typed_quit_rejects_selectors_and_missing_consent_and_requires_owned_output_before_acceptance()
 {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No assets needed".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let server = owner.server();
    let token = owner.workspace().quit_review().unwrap().token();
    let tool = server.get_tool("quit_all").unwrap();
    assert_eq!(tool.input_schema["additionalProperties"], false);
    assert!(tool.input_schema["properties"].get("document_id").is_none());
    let (transport, _client) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    for arguments in [
        json!({"review_token":token}),
        json!({"review_token":token,"discard_changes":false,"document_id":"unrelated"}),
    ] {
        let request = CallToolRequestParams::new("quit_all")
            .with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(RequestId::Number(1), running.peer().clone());
        assert_eq!(
            server.call_tool(request, context).await.unwrap_err().code,
            rmcp::model::ErrorCode::INVALID_PARAMS
        );
    }
    let request = CallToolRequestParams::new("quit_all").with_arguments(
        json!({"review_token":token,"discard_changes":false})
            .as_object()
            .unwrap()
            .clone(),
    );
    let context = RequestContext::new(RequestId::Number(2), running.peer().clone());
    let CallToolResponse::Complete(result) = server.call_tool(request, context).await.unwrap()
    else {
        panic!("Unexpected response")
    };
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("witness")
    );
    assert_eq!(application.document().lifecycle(), DocumentLifecycle::Live);
    assert!(!application.workspace().busy());
    assert_eq!(owner.workspace().quit_review().unwrap().token(), token);
    bounded(running.cancel()).await.unwrap();
    owner.shutdown().unwrap();
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn accepted_native_quit_remains_owned_when_shutdown_stops_command_admission() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No assets needed".into()),
        directory.path().join("recovery"),
    );
    let retired = Arc::new(Mutex::new(Vec::new()));
    let observed = retired.clone();
    let ui = WorkspaceUi::new(
        |_, _, _, _| Box::pin(async { Ok(()) }),
        |_, _| Box::pin(async { Ok(()) }),
        move |id| {
            observed.lock().unwrap().push(id);
            Box::pin(async { Ok(()) })
        },
        |_| Box::pin(async { Ok(()) }),
        || Box::pin(async { Ok(()) }),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, Some(ui)).unwrap();
    let workspace = owner.workspace();
    workspace.new_document(Actor::Agent, false).await.unwrap();
    let ids = application
        .workspace()
        .documents()
        .iter()
        .map(|entry| entry.document().id())
        .collect::<Vec<_>>();
    let review = workspace.quit_review().unwrap();
    workspace.quit_all_native(&review, false).unwrap();
    assert!(workspace.quit_all_native(&review, false).is_err());
    owner.shutdown().unwrap();
    assert_eq!(*retired.lock().unwrap(), ids);
    assert!(application.workspace().documents().is_empty());
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn failed_retirement_keeps_its_failed_entry_while_other_documents_complete() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No assets needed".into()),
        directory.path().join("recovery"),
    );
    let a = application.document();
    let failed_id = a.id();
    let released = Arc::new(Mutex::new(Vec::new()));
    let observed = released.clone();
    let ui = WorkspaceUi::new(
        |_, _, _, _| Box::pin(async { Ok(()) }),
        |_, _| Box::pin(async { Ok(()) }),
        move |id| {
            Box::pin(async move {
                if id == failed_id {
                    Err("injected hide failure".into())
                } else {
                    Ok(())
                }
            })
        },
        move |id| {
            observed.lock().unwrap().push(id);
            Box::pin(async { Ok(()) })
        },
        || Box::pin(async { Ok(()) }),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, Some(ui)).unwrap();
    let workspace = owner.workspace();
    let created = workspace.new_document(Actor::Agent, false).await.unwrap();
    let b = application
        .workspace()
        .resolve(created["document_id"].as_str())
        .unwrap()
        .document();
    workspace
        .quit_all_native(&workspace.quit_review().unwrap(), false)
        .unwrap();
    assert!(
        owner
            .shutdown()
            .unwrap_err()
            .contains("injected hide failure")
    );
    assert_eq!(*released.lock().unwrap(), [b.id()]);
    let remaining = application.workspace().documents();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].document().id(), a.id());
    assert_eq!(
        remaining[0].state(),
        startup::WorkspaceDocumentState::Failed
    );
    assert!(
        remaining[0]
            .error()
            .unwrap()
            .contains("injected hide failure")
    );
    assert_eq!(a.lifecycle(), DocumentLifecycle::Finished);
    assert_eq!(b.lifecycle(), DocumentLifecycle::Finished);
    assert!(
        application
            .finish(Ok(()))
            .unwrap_err()
            .to_string()
            .contains("injected hide failure")
    );
}
