use super::*;
use base64::Engine;

fn inspect(server: &EditorServer, arguments: Value) -> (Value, Option<Vec<u8>>, Option<Vec<u8>>) {
    let result = server
        .preview_material(Parameters(serde_json::from_value(arguments).unwrap()))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    let report = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    let bytes = result.content.get(1).map(|content| {
        base64::engine::general_purpose::STANDARD
            .decode(&content.as_image().unwrap().data)
            .unwrap()
    });
    let pixels = bytes.as_ref().map(|bytes| {
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        pixels.truncate(info.buffer_size());
        pixels
    });
    (report, bytes, pixels)
}

#[test]
fn material_channel_mcp_is_strict_cpu_only_and_preserves_default_png_and_missing_state() {
    let (_directory, game) = crate::test_assets::alpha_game();
    let mut session = Session::new(Ok(game));
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let before = session.document().vmf().clone();
    let token = session.property_draft().unwrap().token;
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let default = inspect(&server, json!({"material":"test/checker"}));
    let color = inspect(
        &server,
        json!({"material":"test/checker","channel":"color"}),
    );
    assert_eq!(default.1, color.1);
    assert_eq!(
        default.1,
        inspect(&server, json!({"material":"test/checker","channel":null})).1
    );
    assert_eq!(default.0["channel"], "color");
    let alpha = inspect(
        &server,
        json!({"material":"test/checker","channel":"alpha"}),
    );
    assert_eq!(alpha.0["source"], color.0["source"]);
    for (color, alpha) in color
        .2
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(alpha.2.unwrap().as_chunks::<4>().0.iter())
    {
        assert_eq!(*alpha, [color[3], color[3], color[3], 255]);
    }
    let missing = inspect(&server, json!({"material":"test/absent","channel":"alpha"}));
    assert_eq!(missing.0["missing"], true);
    assert!(missing.0["source"].is_null() && missing.1.is_none());
    for channel in [json!("Alpha"), json!("opacity"), json!(1), json!(true)] {
        assert!(
            serde_json::from_value::<MaterialPreviewArgs>(
                json!({"material":"test/checker","channel":channel})
            )
            .is_err()
        );
    }
    let info = server.get_map_info().unwrap();
    let info: Value = serde_json::from_str(&info.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(info["rendering"]["initialized"], false);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.property_draft().unwrap().token, token);
}
