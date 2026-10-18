use super::*;
use rmcp::ServerHandler;
use rmcp::model::{CallToolRequestParams, CallToolResponse, RequestId};
use rmcp::service::{RequestContext, serve_directly};

#[tokio::test]
async fn typed_save_all_workspace_tool_preserves_partial_report_and_rejects_selectors() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No content required".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().resolve(None).unwrap();
    let first_document = first.document();
    lock(first_document.session())
        .save(Actor::User, Some(&directory.path().join("first.vmf")))
        .unwrap();
    let created = workspace.new_document(Actor::Agent, false).await.unwrap();
    let second = application
        .workspace()
        .resolve(created["document_id"].as_str())
        .unwrap();
    for entry in [&first, &second] {
        lock(entry.document().session())
            .create_block(
                Actor::User,
                glam::DVec3::ZERO,
                glam::DVec3::splat(16.0),
                "test",
                false,
            )
            .unwrap();
    }
    let second_document = second.document();
    let peer_source = lock(second_document.session()).document().vmf().clone();
    let peer_camera = lock(second_document.session()).camera();
    let server = owner.server();
    let tool = server.get_tool("save_all_named_maps").unwrap();
    assert_eq!(tool.input_schema["additionalProperties"], false);
    assert!(
        tool.input_schema
            .get("properties")
            .and_then(|properties| properties.get("document_id"))
            .is_none()
    );
    let (transport, _client) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let request =
        CallToolRequestParams::new("save_all_named_maps").with_arguments(serde_json::Map::new());
    let context = RequestContext::new(RequestId::Number(1), running.peer().clone());
    let response = tokio::time::timeout(Duration::from_secs(5), server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap();
    let CallToolResponse::Complete(result) = response else {
        panic!("unexpected response")
    };
    assert_eq!(result.is_error, Some(true));
    let value: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(value["complete"], false);
    assert_eq!(value["saved"], 1);
    assert!(
        value["documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["document_id"] == second.selector() && row["status"] == "needs_save_as")
    );
    assert_eq!(
        lock(second_document.session()).document().vmf(),
        &peer_source
    );
    assert_eq!(lock(second_document.session()).camera(), peer_camera);
    assert_eq!(first_document.active_operations(), 0);
    assert_eq!(second_document.active_operations(), 0);
    for arguments in [
        json!({"document_id":first.selector()}),
        json!({"path":"other.vmf"}),
        json!({"background":true}),
    ] {
        let request = CallToolRequestParams::new("save_all_named_maps")
            .with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(RequestId::Number(2), running.peer().clone());
        let error = server.call_tool(request, context).await.unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
    tokio::time::timeout(Duration::from_secs(5), running.cancel())
        .await
        .unwrap()
        .unwrap();
    owner.shutdown().unwrap();
    application.finish(Ok(())).unwrap();
}

#[test]
fn accepted_save_all_survives_waiter_loss_and_owner_shutdown_drains_fixed_operations() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("No content required".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let document = application.document();
    let path = directory.path().join("accepted.vmf");
    lock(document.session())
        .save(Actor::User, Some(&path))
        .unwrap();
    lock(document.session())
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let (entered, observed) = std::sync::mpsc::sync_channel(1);
    let (release, waiting) = std::sync::mpsc::sync_channel(1);
    *workspace.0.before_command.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let request = application
        .workspace()
        .prepare_save_all_named(Actor::Agent)
        .unwrap();
    let (send, receive) = oneshot::channel();
    workspace.enqueue(Command::SaveAll(request, send)).unwrap();
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(document.active_operations(), 1);
    assert!(application.workspace().busy());
    drop(receive);
    release.send(()).unwrap();
    owner.shutdown().unwrap();
    assert_eq!(document.active_operations(), 0);
    assert!(!application.workspace().busy());
    assert!(!lock(document.session()).document().is_dirty());
    let text = std::fs::read_to_string(path).unwrap();
    let written = source_formats::vmf::Vmf::parse(&text).unwrap();
    assert_eq!(written.world.solids.len(), 1);
    application.finish(Ok(())).unwrap();
}
