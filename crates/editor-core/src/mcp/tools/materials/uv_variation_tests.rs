use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, RunningService, serve_directly};

async fn dispatch(
    server: &EditorServer,
    service: &RunningService<rmcp::RoleServer, EditorServer>,
    name: &'static str,
    args: Value,
) -> CallToolResult {
    let request =
        CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone());
    let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
    match server.call_tool(request, context).await.unwrap() {
        CallToolResponse::Complete(result) => result,
        result => panic!("unexpected tool response: {result:?}"),
    }
}

fn report(result: CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn seeded_uv_variation_mcp_dispatch_shares_drafts_noops_guards_and_undo() {
    let mut session = Session::new(Err("no assets needed".into()));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        panic!()
    };
    let faces = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .map(|side| map_core::surface::FaceId {
            solid,
            side: side.id,
        })
        .collect::<Vec<_>>();
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.vmf");
    std::fs::write(&path, source.write(&Default::default())).unwrap();
    let mut expected = Session::new(Err("no assets needed".into()));
    expected.open(Actor::User, &path, false).unwrap();
    expected
        .vary_face_uv(Actor::User, &faces, 123, 32.0, 8.0, true)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    assert!(server.get_tool("vary_face_uv").is_some());
    for args in [
        json!({"seed":-1,"max_shift_u":1,"max_shift_v":1}),
        json!({"seed":4294967296_u64,"max_shift_u":1,"max_shift_v":1}),
        json!({"seed":1.5,"max_shift_u":1,"max_shift_v":1}),
        json!({"seed":1,"max_shift_u":1,"max_shift_v":1,"rotation":45}),
    ] {
        assert!(serde_json::from_value::<VaryFaceUvArgs>(args).is_err());
    }
    let args = json!({"seed":123,"max_shift_u":32,"max_shift_v":8,"preview":true});
    let first = report(dispatch(&server, &service, "vary_face_uv", args.clone()).await);
    assert_eq!(first["faces_changed"], 6);
    assert_eq!(first["preview_created"], true);
    let token = first["preview_token"].clone();
    assert_eq!(
        lock(&shared).render_document().vmf(),
        expected.render_document().vmf()
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    let rejected = dispatch(
        &server,
        &service,
        "vary_face_uv",
        json!({"seed":123,"max_shift_u":-1,"max_shift_v":8,"preview":true}),
    )
    .await;
    assert_eq!(rejected.is_error, Some(true));
    let zero = report(
        dispatch(
            &server,
            &service,
            "vary_face_uv",
            json!({"seed":123,"max_shift_u":0,"max_shift_v":0,"preview":true}),
        )
        .await,
    );
    assert_eq!(zero["faces_changed"], 0);
    assert_eq!(zero["preview_created"], false);
    assert!(zero["preview_token"].is_null());
    assert_eq!(zero["state"]["preview_token"], token);
    report(
        dispatch(
            &server,
            &service,
            "cancel_face_preview",
            json!({"token":token}),
        )
        .await,
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    let second = report(dispatch(&server, &service, "vary_face_uv", args).await);
    let stale = dispatch(
        &server,
        &service,
        "commit_face_preview",
        json!({"token":token}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));
    report(
        dispatch(
            &server,
            &service,
            "commit_face_preview",
            json!({"token":second["preview_token"]}),
        )
        .await,
    );
    assert_eq!(
        lock(&shared).document().vmf(),
        expected.render_document().vmf()
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    service.cancel().await.unwrap();
}
