use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, RunningService, serve_directly};
use std::time::Duration;

async fn dispatch(
    server: &EditorServer,
    service: &RunningService<rmcp::RoleServer, EditorServer>,
    name: &'static str,
    arguments: Value,
) -> CallToolResult {
    let request =
        CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
    let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
    match tokio::time::timeout(Duration::from_secs(5), server.call_tool(request, context))
        .await
        .unwrap()
        .unwrap()
    {
        CallToolResponse::Complete(result) => result,
        result => panic!("Unexpected tool response: {result:?}"),
    }
}

fn report(result: CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn asset_catalog_dispatch_exposes_shared_filters_provenance_and_schema_bounds_without_graphics()
 {
    let (directory, game) = crate::test_assets::model_lighting::game();
    std::fs::create_dir_all(directory.path().join("materials/maps/demo")).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/maps/demo/checker_wvt_patch.vmt"),
        "LightmappedGeneric {}",
    )
    .unwrap();
    let session = Session::new(Ok(game));
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let catalog = report(dispatch(&server, &service, "browse_materials", json!({"query":"checker","source":"directory","exclude_generated_map_materials":true,"limit":1})).await);
    assert_eq!(catalog["total"], 1);
    assert_eq!(catalog["entries"][0]["name"], "test/checker");
    assert_eq!(
        catalog["entries"][0]["source"]["mount"],
        directory.path().to_str().unwrap()
    );
    let source_id = catalog["entries"][0]["source_id"].as_str().unwrap();
    let search = report(
        dispatch(
            &server,
            &service,
            "search_materials",
            json!({"query":"checker","source":source_id,"exclude_generated_map_materials":true}),
        )
        .await,
    );
    assert_eq!(search["materials"], json!(["test/checker"]));
    assert_eq!(search["entries"], catalog["entries"]);
    let models = report(
        dispatch(
            &server,
            &service,
            "browse_models",
            json!({"query":"receiver","source":source_id,"limit":1}),
        )
        .await,
    );
    assert_eq!(models["total"], 1);
    assert_eq!(models["entries"][0]["source_id"], source_id);
    let inspected = report(
        dispatch(
            &server,
            &service,
            "inspect_model",
            json!({"path":"models/test/receiver.mdl"}),
        )
        .await,
    );
    assert_eq!(inspected["sources"].as_array().unwrap().len(), 3);
    assert_eq!(inspected["sources"][0]["source_id"], source_id);
    for (name, arguments) in [
        ("browse_models", json!({"limit":101})),
        ("browse_materials", json!({"source":"stock"})),
        ("search_materials", json!({"query":"","limit":201})),
    ] {
        assert!(
            dispatch(&server, &service, name, arguments)
                .await
                .is_error
                .unwrap_or(false)
        );
    }
    let schema = serde_json::to_value(schemars::schema_for!(BrowseModelsArgs)).unwrap();
    assert_eq!(
        schema["properties"]["limit"]["maximum"].as_f64(),
        Some(100.0)
    );
    assert_eq!(schema["properties"]["limit"]["minimum"].as_f64(), Some(1.0));
    assert!(schema["properties"]["source"].is_object());
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    service.cancel().await.unwrap();
}
