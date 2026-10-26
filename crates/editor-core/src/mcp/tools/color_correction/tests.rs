use super::*;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn routed_lut_tools_share_source_failures_and_settings_without_initializing_graphics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-correction-tests");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("mcp-")
        .tempdir_in(root)
        .unwrap();
    let path = directory.path().join("grade.raw");
    std::fs::write(&path, [240, 120, 60].repeat(32768)).unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err(
        "fixture content unavailable".into()
    ))));
    let server = EditorServer::new(shared.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    for (name, arguments, success) in [
        (
            "set_color_correction_preview",
            json!({"enabled":true,"weight":1}),
            false,
        ),
        ("load_color_correction_lut", json!({"path":path}), true),
        (
            "set_color_correction_preview",
            json!({"enabled":true,"weight":0.5}),
            true,
        ),
        (
            "set_color_correction_preview",
            json!({"enabled":false,"weight":2}),
            false,
        ),
    ] {
        let request =
            CallToolRequestParams::new(name).with_arguments(arguments.as_object().unwrap().clone());
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
    }
    assert_eq!(
        value(server.get_color_correction_preview())["enabled"],
        true
    );
    assert_eq!(value(server.get_color_correction_preview())["weight"], 0.5);
    std::fs::remove_file(path).unwrap();
    let failed = value(server.get_color_correction_preview());
    assert_eq!(failed["ready"], false);
    assert_eq!(failed["enabled"], true);
    assert!(lock(&shared).color_correction_for_render().is_err());
    let camera = lock(&shared).camera();
    assert!(
        server
            .render_png(&camera, (8, 8), ViewOptions::default())
            .is_err()
    );
    assert_eq!(value(server.clear_color_correction_lut())["enabled"], false);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    for arguments in [
        json!({"enabled":true}),
        json!({"enabled":true,"weight":1,"path":"x.raw"}),
    ] {
        assert!(serde_json::from_value::<SetColorCorrectionArgs>(arguments).is_err());
    }
    tokio::time::timeout(Duration::from_secs(5), service.cancel())
        .await
        .unwrap()
        .unwrap();
}
