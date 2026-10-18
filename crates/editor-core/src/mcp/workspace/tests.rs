use super::*;
use crate::runtime::DocumentLifecycle;
use rmcp::ServerHandler;
use rmcp::model::{CallToolRequestParams, CallToolResponse, RequestId};
use rmcp::service::{RequestContext, serve_directly};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

const TIMEOUT: Duration = Duration::from_secs(15);

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("Workspace fixture timed out")
}

fn value(result: &Value) -> Value {
    assert_ne!(result["isError"], true, "{result}");
    result.get("structuredContent").cloned().unwrap_or_else(|| {
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    })
}

async fn rpc(
    write: &mut WriteHalf<DuplexStream>,
    read: &mut Lines<BufReader<ReadHalf<DuplexStream>>>,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let request = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    bounded(write.write_all(format!("{request}\n").as_bytes()))
        .await
        .unwrap();
    let line = bounded(read.next_line())
        .await
        .unwrap()
        .expect("MCP stopped before response");
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["id"], id);
    response
}

#[tokio::test]
async fn production_stdio_pins_two_documents_and_only_last_close_stops_open_input() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("no assets required".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().documents()[0].clone();
    let original = first.document();
    let (service_io, client) = tokio::io::duplex(256 * 1024);
    let (service_read, service_write) = tokio::io::split(service_io);
    let service = tokio::spawn(run_workspace_stdio(
        owner.server(),
        service_read,
        service_write,
    ));
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut lines = BufReader::new(client_read).lines();
    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"workspace-fixture","version":"1"}}});
    bounded(client_write.write_all(format!("{initialize}\n").as_bytes()))
        .await
        .unwrap();
    let initialized: Value =
        serde_json::from_str(&bounded(lines.next_line()).await.unwrap().unwrap()).unwrap();
    assert_eq!(initialized["result"]["serverInfo"]["name"], "hammerr");
    bounded(
        client_write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"),
    )
    .await
    .unwrap();
    let one = rpc(&mut client_write, &mut lines, 1, "get_map_info", json!({})).await;
    assert_eq!(
        one["result"]["_meta"][DOCUMENT_META]["document_id"],
        first.selector()
    );
    let created = rpc(&mut client_write, &mut lines, 2, "new_document", json!({})).await;
    let created = value(&created["result"]);
    let second = created["document_id"].as_str().unwrap().to_owned();
    assert_ne!(first.selector(), second);
    let peer = application
        .workspace()
        .resolve(Some(&second))
        .unwrap()
        .document();
    assert!(!Arc::ptr_eq(original.session(), peer.session()));
    let ambiguous = rpc(&mut client_write, &mut lines, 3, "get_map_info", json!({})).await;
    assert_eq!(ambiguous["result"]["isError"], true);
    let full = rpc(&mut client_write, &mut lines, 4, "new_document", json!({})).await;
    assert_eq!(full["result"]["isError"], true);
    for (id, selector, x) in [(5, first.selector(), 16), (6, second.as_str(), 128)] {
        let edited = rpc(
            &mut client_write,
            &mut lines,
            id,
            "create_entity",
            json!({"document_id":selector,"classname":"info_target","origin":[x,0,0]}),
        )
        .await;
        assert_ne!(edited["result"]["isError"], true, "{edited}");
        assert_eq!(
            edited["result"]["_meta"][DOCUMENT_META]["document_id"],
            selector
        );
    }
    assert_eq!(
        lock(original.session()).document().vmf().entities[0]
            .origin()
            .unwrap()
            .x,
        16.0
    );
    assert_eq!(
        lock(peer.session()).document().vmf().entities[0]
            .origin()
            .unwrap()
            .x,
        128.0
    );
    let before = lock(peer.session()).document().vmf().clone();
    lock(peer.session())
        .preview_block(glam::DVec3::splat(512.0), glam::DVec3::splat(576.0))
        .unwrap();
    let peer_tool = lock(peer.session()).transform_tool();
    let peer_preview = lock(peer.session()).block_preview();
    let peer_selection = lock(peer.session()).selected_ids();
    let peer_history = lock(peer.session()).history_snapshot();
    let bindings = rpc(
        &mut client_write,
        &mut lines,
        100,
        "get_keyboard_shortcuts",
        json!({"document_id":first.selector()}),
    )
    .await;
    let bindings = value(&bindings["result"]);
    let mut configured = crate::shortcuts::Bindings::default();
    configured.move_tool[0] = Some(crate::shortcuts::Chord {
        key: "F8".into(),
        control: false,
        shift: false,
    });
    configured.draw_brush[0] = Some(crate::shortcuts::Chord {
        key: "F9".into(),
        control: false,
        shift: false,
    });
    configured.clip_mode[0] = Some(crate::shortcuts::Chord {
        key: "F10".into(),
        control: false,
        shift: false,
    });
    let saved = rpc(
        &mut client_write,
        &mut lines,
        101,
        "set_keyboard_shortcuts",
        json!({"document_id":first.selector(),"action":"save","snapshot":bindings["snapshot"],"bindings":configured}),
    )
    .await;
    assert_eq!(value(&saved["result"])["ready"], true);
    assert_eq!(lock(peer.session()).shortcut_bindings(), configured);
    for (index, (command, tool)) in [
        ("move_tool", "move"),
        ("rotate_tool", "rotate"),
        ("scale_tool", "scale"),
        ("select_tool", "select"),
    ]
    .into_iter()
    .enumerate()
    {
        let result = rpc(
            &mut client_write,
            &mut lines,
            102 + index as u64,
            "execute_editor_command",
            json!({"document_id":first.selector(),"command":command}),
        )
        .await;
        assert_eq!(value(&result["result"])["tool"], tool);
        assert_eq!(lock(peer.session()).transform_tool(), peer_tool);
        assert_eq!(lock(peer.session()).block_preview(), peer_preview);
        assert_eq!(lock(peer.session()).selected_ids(), peer_selection);
        assert_eq!(lock(peer.session()).history_snapshot(), peer_history);
        assert_eq!(lock(peer.session()).document().vmf(), &before);
    }
    let created = rpc(
        &mut client_write,
        &mut lines,
        110,
        "create_block",
        json!({"document_id":first.selector(),"mins":[0,0,0],"maxs":[32,32,32]}),
    )
    .await;
    value(&created["result"]);
    let source = lock(original.session()).document().vmf().clone();
    let history = lock(original.session()).history_snapshot();
    let peer_camera = lock(peer.session()).camera();
    let peer_spatial = lock(peer.session()).spatial();
    for (index, (command, field, enabled)) in [
        ("draw_brush", "draw_block", true),
        ("draw_brush", "draw_block", true),
        ("clip_mode", "clip_tool", true),
        ("clip_mode", "clip_tool", true),
    ]
    .into_iter()
    .enumerate()
    {
        let result = rpc(
            &mut client_write,
            &mut lines,
            111 + index as u64,
            "execute_editor_command",
            json!({"document_id":first.selector(),"command":command}),
        )
        .await;
        assert_eq!(value(&result["result"])[field], enabled);
        assert_eq!(lock(original.session()).document().vmf(), &source);
        assert_eq!(lock(original.session()).history_snapshot(), history);
        assert_eq!(lock(peer.session()).camera(), peer_camera);
        assert_eq!(lock(peer.session()).spatial(), peer_spatial);
        assert!(!lock(peer.session()).clip_tool());
        assert_eq!(lock(peer.session()).transform_tool(), peer_tool);
        assert_eq!(lock(peer.session()).block_preview(), peer_preview);
        assert_eq!(lock(peer.session()).selected_ids(), peer_selection);
        assert_eq!(lock(peer.session()).history_snapshot(), peer_history);
        assert_eq!(lock(peer.session()).document().vmf(), &before);
    }
    let token = original.document_close_review().unwrap().token();
    let strict = rpc(&mut client_write, &mut lines, 7, "close_document", json!({"document_id":first.selector(),"review_token":token,"discard_changes":true,"unexpected":1})).await;
    assert_eq!(strict["error"]["code"], -32602);
    assert_eq!(
        strict["error"]["data"][DOCUMENT_META]["document_id"],
        first.selector()
    );
    assert_eq!(original.lifecycle(), DocumentLifecycle::Live);
    let closed = rpc(
        &mut client_write,
        &mut lines,
        8,
        "close_document",
        json!({"document_id":first.selector(),"review_token":token,"discard_changes":true}),
    )
    .await;
    assert_eq!(value(&closed["result"])["terminal"], false);
    assert_eq!(value(&closed["result"])["closed"], true);
    assert_eq!(original.lifecycle(), DocumentLifecycle::Finished);
    assert!(
        application
            .workspace()
            .resolve(Some(first.selector()))
            .is_err()
    );
    assert!(!service.is_finished());
    assert_eq!(lock(peer.session()).document().vmf(), &before);
    let unknown = rpc(
        &mut client_write,
        &mut lines,
        9,
        "get_map_info",
        json!({"document_id":first.selector()}),
    )
    .await;
    assert_eq!(unknown["result"]["isError"], true);
    let rejected = rpc(
        &mut client_write,
        &mut lines,
        115,
        "execute_editor_command",
        json!({"document_id":first.selector(),"command":"draw_brush"}),
    )
    .await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(lock(peer.session()).spatial(), peer_spatial);
    assert_eq!(lock(peer.session()).document().vmf(), &before);
    let remaining = rpc(&mut client_write, &mut lines, 10, "get_map_info", json!({})).await;
    assert_eq!(
        remaining["result"]["_meta"][DOCUMENT_META]["document_id"],
        second
    );
    let token = value(&remaining["result"])["document_close"]["review_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let closed = rpc(
        &mut client_write,
        &mut lines,
        11,
        "close_document",
        json!({"document_id":second,"review_token":token,"discard_changes":true}),
    )
    .await;
    assert_eq!(value(&closed["result"])["terminal"], true);
    assert_eq!(
        closed["result"]["_meta"][DOCUMENT_META]["document_id"],
        second
    );
    assert!(closed["result"]["_meta"].as_object().unwrap().len() >= 2);
    bounded(service).await.unwrap().unwrap();
    owner.shutdown().unwrap();
    assert!(workspace.0.factory.documents().is_empty());
    assert!(workspace.0.entries.lock().unwrap().is_empty());
    drop(client_write);
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn cancelled_or_busy_close_does_not_claim_membership_or_touch_peer() {
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("no assets required".into()),
        directory.path().join("recovery"),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, None).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().documents()[0].clone();
    bounded(workspace.new_document(Actor::Agent, false))
        .await
        .unwrap();
    let server = owner.server();
    let (transport, _client) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let request = || {
        CallToolRequestParams::new("close_document").with_arguments(json!({"document_id":first.selector(),"review_token":first.document().document_close_review().unwrap().token()}).as_object().unwrap().clone())
    };
    let context = RequestContext::new(RequestId::Number(1), running.peer().clone());
    context.ct.cancel();
    let response = bounded(server.call_tool(request(), context)).await.unwrap();
    let CallToolResponse::Complete(response) = response else {
        panic!("Expected completed rejection");
    };
    assert_eq!(response.is_error, Some(true));
    assert!(!application.workspace().busy());
    let operation = first.document().admit().unwrap();
    let context = RequestContext::new(RequestId::Number(2), running.peer().clone());
    let response = bounded(server.call_tool(request(), context)).await.unwrap();
    let CallToolResponse::Complete(response) = response else {
        panic!("Expected completed rejection");
    };
    assert_eq!(response.is_error, Some(true));
    assert!(!application.workspace().busy());
    drop(operation);
    assert_eq!(application.workspace().documents().len(), 2);
    assert_eq!(first.document().lifecycle(), DocumentLifecycle::Live);
    bounded(running.cancel()).await.unwrap();
    owner.shutdown().unwrap();
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn accepted_peer_close_survives_handler_loss_and_releases_capacity_after_real_ack() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("no assets required".into()),
        directory.path().join("recovery"),
    );
    let (entered, observed) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let (release, waiting) = oneshot::channel();
    let waiting = Arc::new(Mutex::new(Some(waiting)));
    let quit = Arc::new(AtomicBool::new(false));
    let quitting = quit.clone();
    let ui = WorkspaceUi::new(
        |_, _, _, _| Box::pin(async { Ok(()) }),
        |_| Box::pin(async { Ok(()) }),
        move |_| {
            let entered = entered.lock().unwrap().take();
            let waiting = waiting.lock().unwrap().take();
            Box::pin(async move {
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                if let Some(waiting) = waiting {
                    waiting.await.map_err(|_| "UI ack owner lost".to_owned())?;
                }
                Ok(())
            })
        },
        |_| Box::pin(async { Ok(()) }),
        move || {
            let quit = quitting.clone();
            Box::pin(async move {
                quit.store(true, Ordering::Release);
                Ok(())
            })
        },
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, Some(ui)).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().documents()[0].clone();
    let created = bounded(workspace.new_document(Actor::Agent, false))
        .await
        .unwrap();
    let peer = application
        .workspace()
        .resolve(Some(created["document_id"].as_str().unwrap()))
        .unwrap();
    let server = owner.server();
    let (transport, _client) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let context = RequestContext::new(RequestId::Number(3), running.peer().clone());
    let request = CallToolRequestParams::new("close_document").with_arguments(json!({"document_id":first.selector(),"review_token":first.document().document_close_review().unwrap().token()}).as_object().unwrap().clone());
    let caller = tokio::spawn(async move { server.call_tool(request, context).await });
    bounded(observed).await.unwrap();
    caller.abort();
    let _ = bounded(caller).await;
    assert!(application.workspace().busy());
    assert_eq!(first.document().lifecycle(), DocumentLifecycle::Finished);
    let operation = peer.document().admit().unwrap();
    drop(operation);
    release.send(()).unwrap();
    bounded(async {
        while application.workspace().busy() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        application
            .workspace()
            .resolve(Some(first.selector()))
            .is_err()
    );
    assert_eq!(peer.document().lifecycle(), DocumentLifecycle::Live);
    assert!(!quit.load(Ordering::Acquire));
    bounded(workspace.new_document(Actor::Agent, false))
        .await
        .unwrap();
    assert_eq!(application.workspace().documents().len(), 2);
    bounded(running.cancel()).await.unwrap();
    owner.shutdown().unwrap();
    application.finish(Ok(())).unwrap();
}

#[tokio::test]
async fn unpublished_host_failure_retains_failed_capacity_and_source_claims() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("candidate.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    let application = crate::test_assets::application(
        Err("no assets required".into()),
        directory.path().join("recovery"),
    );
    let factory = application.workspace();
    let closing = factory.clone();
    let ui = WorkspaceUi::new(
        move |_, _, _, _| {
            let factory = closing.clone();
            Box::pin(async move {
                factory.close_admission();
                Ok(())
            })
        },
        |_| Box::pin(async { Ok(()) }),
        |_| Box::pin(async { Err("actual candidate retirement failed".into()) }),
        |_| Box::pin(async { panic!("An unconfirmed retired host must remain owned") }),
        || Box::pin(async { Ok(()) }),
    );
    let original = factory.documents()[0].document();
    let before = lock(original.session()).document().vmf().clone();
    let mut owner = WorkspaceOwner::start(factory.clone(), None, Some(ui)).unwrap();
    let error = bounded(
        owner
            .workspace()
            .open_document(Actor::Agent, &source, false),
    )
    .await
    .unwrap_err();
    assert!(error.contains("application closed"), "{error}");
    assert!(
        error.contains("actual candidate retirement failed"),
        "{error}"
    );
    let documents = factory.documents();
    assert_eq!(documents.len(), 2);
    let failed = documents
        .iter()
        .find(|entry| entry.document().id() != original.id())
        .unwrap();
    assert_eq!(failed.state().label(), "failed");
    assert_eq!(failed.document().lifecycle(), DocumentLifecycle::Finished);
    assert!(!factory.busy());
    assert!(!factory.can_create());
    assert!(
        lock(original.session())
            .save(Actor::Agent, Some(&source))
            .is_err()
    );
    assert_eq!(lock(original.session()).document().vmf(), &before);
    assert!(
        owner
            .workspace()
            .0
            .entries
            .lock()
            .unwrap()
            .contains_key(&failed.document().id())
    );
    owner.shutdown().unwrap();
    let error = application.finish(Ok(())).unwrap_err().to_string();
    assert!(
        error.contains("actual candidate retirement failed"),
        "{error}"
    );
}

#[tokio::test]
async fn command_owner_panic_drains_accepted_close_and_preserves_all_errors() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let directory = tempfile::tempdir().unwrap();
    let application = crate::test_assets::application(
        Err("no assets required".into()),
        directory.path().join("recovery"),
    );
    let ui = WorkspaceUi::new(
        |_, _, _, _| Box::pin(async { Ok(()) }),
        |_| Box::pin(async { Ok(()) }),
        |_| Box::pin(async { Err("actual retire fault".into()) }),
        |_| Box::pin(async { Ok(()) }),
        || Box::pin(async { Err("actual quit fault".into()) }),
    );
    let mut owner = WorkspaceOwner::start(application.workspace(), None, Some(ui)).unwrap();
    let workspace = owner.workspace();
    let first = application.workspace().documents()[0].clone();
    bounded(workspace.new_document(Actor::Agent, false))
        .await
        .unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopping = stopped.clone();
    workspace.0.stop.lock().unwrap().stop = Some(Arc::new(move || {
        stopping.store(true, Ordering::Release);
    }));
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, waiting) = mpsc::sync_channel(1);
    *workspace.0.before_command.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        waiting.recv_timeout(TIMEOUT).unwrap();
        panic!("injected command owner failure");
    }));
    let (response, _show) = oneshot::channel();
    workspace
        .enqueue(Command::Show(first.selector().to_owned(), response))
        .unwrap();
    observed.recv_timeout(TIMEOUT).unwrap();
    let server = owner.server();
    let (transport, _client) = tokio::io::duplex(4096);
    let running = serve_directly::<rmcp::RoleServer, _, _, _, _>(server, transport, None);
    let context = RequestContext::new(RequestId::Number(4), running.peer().clone());
    let fixed = workspace.fixed_server(&first).unwrap();
    let reservation = application
        .workspace()
        .reserve_close(first.selector())
        .unwrap();
    let response = workspace
        .terminal_for(first.document().id())
        .unwrap()
        .commit_workspace_mcp(
            &fixed,
            &first.document().document_close_review().unwrap().token(),
            false,
            &context,
            workspace.close_plan(reservation, metadata(&first)),
        )
        .unwrap();
    bounded(async {
        while workspace.0.queue.lock().unwrap().completed.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    release.send(()).unwrap();
    let result = bounded(response).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.structured_content.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("workspace command worker panicked")
    );
    assert!(!application.workspace().busy());
    assert_eq!(
        application
            .workspace()
            .documents()
            .iter()
            .find(|entry| entry.document().id() == first.document().id())
            .unwrap()
            .state()
            .label(),
        "failed"
    );
    let error = owner.shutdown().unwrap_err();
    assert!(stopped.load(Ordering::Acquire));
    assert!(
        error.contains("workspace command worker panicked"),
        "{error}"
    );
    assert!(error.contains("actual retire fault"), "{error}");
    assert!(error.contains("actual quit fault"), "{error}");
    bounded(running.cancel()).await.unwrap();
    assert!(
        application
            .finish(Ok(()))
            .unwrap_err()
            .to_string()
            .contains("workspace command worker panicked")
    );
}
