use super::*;

#[test]
fn gallery_tool_checks_review_limits_and_per_entry_errors_without_initializing_graphics() {
    let (session, directory, _brush) = crate::test_assets::prefabs();
    std::fs::write(directory.path().join("Broken.vmf"), "invalid source").unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let clipboard = lock(&shared).clipboard_report();
    let history = lock(&shared).history_snapshot();
    let report = lock(&shared).prefab_report("", 0, 8, None);
    for invalid in [
        json!({"snapshot":report["snapshot"],"limit":9}),
        json!({"snapshot":report["snapshot"],"limit":0}),
        json!({"snapshot":"stale"}),
    ] {
        let result = server
            .render_prefab_gallery(Parameters(serde_json::from_value(invalid).unwrap()))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result
                .content
                .iter()
                .all(|block| block.as_image().is_none())
        );
    }
    for style in ["flat", "textured"] {
        let result = server
            .render_prefab_gallery(Parameters(
                serde_json::from_value(json!({
                    "snapshot":report["snapshot"],"query":"Broken","style":style,"limit":8,
                }))
                .unwrap(),
            ))
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        assert_eq!(result.content.len(), 1);
        let metadata: Value =
            serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(metadata["entries"][0]["name"], "Broken");
        assert!(metadata["entries"][0]["error"].is_string());
        assert!(metadata["entries"][0]["image_content_index"].is_null());
    }
    for invalid in [
        json!({"snapshot":report["snapshot"],"style":"automatic"}),
        json!({"snapshot":report["snapshot"],"names":[]}),
        json!({}),
    ] {
        assert!(serde_json::from_value::<RenderPrefabGalleryArgs>(invalid).is_err());
    }
    assert_eq!(
        lock(&shared).prefab_thumbnail_report("Broken")["graphics"]["initialized"],
        false
    );
    assert_eq!(lock(&shared).clipboard_report(), clipboard);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
#[ignore = "requires a graphics adapter for actual prefab gallery MCP PNG validation"]
fn gallery_png_content_indices_skip_failed_entries_and_match_the_named_cached_images() {
    let (mut session, directory, assets, brush) = crate::test_assets::textured_prefabs();
    for name in ["B assembly", "C assembly"] {
        let stamp = session.prefab_report("", 0, 8, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        session
            .save_prefab(Actor::User, name, &[brush], &stamp)
            .unwrap();
    }
    std::fs::write(directory.path().join("A broken.vmf"), "invalid source").unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let stamp = lock(&shared).prefab_report("", 0, 8, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    let result = server
        .render_prefab_gallery(Parameters(
            serde_json::from_value(json!({"snapshot":stamp,"style":"textured"})).unwrap(),
        ))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let metadata: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert!(metadata["entries"][0]["error"].is_string());
    assert!(metadata["entries"][0]["image_content_index"].is_null());
    assert_eq!(result.content.len(), 3);
    for index in 1..3 {
        let content_index = metadata["entries"][index]["image_content_index"]
            .as_u64()
            .unwrap() as usize;
        assert_eq!(content_index, index);
        let image = serde_json::to_value(&result.content[content_index]).unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(image["data"].as_str().unwrap())
            .unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        let cached = lock(&shared)
            .cached_prefab_thumbnail_with_style(
                metadata["entries"][index]["name"].as_str().unwrap(),
                crate::session::prefabs::ThumbnailStyle::Textured,
            )
            .unwrap()
            .unwrap();
        assert_eq!(&pixels[..info.buffer_size()], cached.image.pixels);
    }
    assert!(assets.path().join("materials/test/checker.vtf").exists());
}
