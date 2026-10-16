use super::*;

fn setup() -> (EditorServer, SharedSession, tempfile::TempDir) {
    let (mut session, directory, brush) = crate::test_assets::prefabs();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .save_prefab(Actor::User, "Fixture", &[brush], &stamp)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    (EditorServer::new(shared.clone(), None), shared, directory)
}

#[test]
fn prefab_thumbnail_mcp_inspection_and_failed_render_are_headless_and_strict() {
    let (server, shared, directory) = setup();
    let before = lock(&shared).history_snapshot();
    let clipboard = lock(&shared).clipboard_report();
    let camera = lock(&shared).camera();
    let result = server
        .inspect_prefab_thumbnail(Parameters(InspectPrefabThumbnailArgs {
            name: "Fixture".into(),
        }))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let report: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(report, lock(&shared).prefab_thumbnail_report("Fixture"));
    assert_eq!(report["graphics"]["initialized"], false);
    assert_eq!(result.content.len(), 1);
    std::fs::write(directory.path().join("Fixture.vmf"), "invalid source").unwrap();
    let result = server
        .render_prefab_thumbnail(Parameters(RenderPrefabThumbnailArgs {
            name: "Fixture".into(),
            refresh: Some(true),
        }))
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert!(
        result
            .content
            .iter()
            .all(|block| serde_json::to_value(block).unwrap()["type"] != "image")
    );
    let report = lock(&shared).prefab_thumbnail_report("Fixture");
    assert!(report["error"].is_string());
    assert_eq!(report["graphics"]["initialized"], false);
    assert!(
        serde_json::from_value::<InspectPrefabThumbnailArgs>(json!({"name":"Fixture","width":64}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<RenderPrefabThumbnailArgs>(json!({"name":"Fixture","height":64}))
            .is_err()
    );
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(lock(&shared).clipboard_report(), clipboard);
    assert_eq!(lock(&shared).camera(), camera);
}

#[test]
#[ignore = "requires a graphics adapter for actual prefab thumbnail MCP validation"]
fn prefab_thumbnail_mcp_png_contains_shared_cached_pixels_and_isolated_state() {
    let (server, shared, _directory) = setup();
    let history = lock(&shared).history_snapshot();
    let clipboard = lock(&shared).clipboard_report();
    let result = server
        .render_prefab_thumbnail(Parameters(RenderPrefabThumbnailArgs {
            name: "Fixture".into(),
            refresh: None,
        }))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    assert_eq!(result.content.len(), 2);
    let metadata: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(metadata["style"], "flat_geometry");
    let image = serde_json::to_value(&result.content[1]).unwrap();
    assert_eq!(image["type"], "image");
    let png = base64::engine::general_purpose::STANDARD
        .decode(image["data"].as_str().unwrap())
        .unwrap();
    let mut decoder = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
    let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
    let info = decoder.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
    let cached = lock(&shared)
        .cached_prefab_thumbnail("Fixture")
        .unwrap()
        .unwrap();
    assert_eq!(&pixels[..info.buffer_size()], cached.image.pixels);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).clipboard_report(), clipboard);
}
