use super::*;

fn pixels(rendered: &RenderedView) -> Vec<u8> {
    let mut reader = png::Decoder::new(Cursor::new(&rendered.png))
        .read_info()
        .unwrap();
    let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut bytes).unwrap();
    bytes.truncate(info.buffer_size());
    bytes
}

#[test]
#[ignore = "requires offscreen GPU; root runs the color-correction acceptance gate explicitly"]
fn color_correction_render_view_uses_final_pass_and_refuses_missing_or_oversized_input() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-correction-tests");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("render-")
        .tempdir_in(root)
        .unwrap();
    let path = directory.path().join("grade.raw");
    std::fs::write(&path, [240, 120, 60].repeat(32768)).unwrap();
    let mut session = Session::new(Err("fixture content unavailable".into()));
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let view = ViewOptions {
        grid_visible: false,
        selection_outlines: false,
        ..Default::default()
    };
    let camera = session.camera();
    session.load_color_correction(&path).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let baseline = server.render_png(&camera, (8, 8), view).unwrap();
    assert_eq!(&pixels(&baseline)[..4], &[29, 31, 36, 255]);
    lock(&shared).set_color_correction(true, 0.0, None).unwrap();
    let zero = server.render_png(&camera, (8, 8), view).unwrap();
    assert_eq!(pixels(&zero), pixels(&baseline));
    lock(&shared).set_color_correction(true, 1.0, None).unwrap();
    let graded = server.render_png(&camera, (8, 8), view).unwrap();
    for &pixel in pixels(&graded).as_chunks::<4>().0 {
        assert_eq!(pixel, [240, 120, 60, 255]);
    }
    let result = graded
        .into_tool_result(camera, (8, 8), view, json!({}))
        .unwrap();
    let report: Value = serde_json::from_str(&result.content[1].as_text().unwrap().text).unwrap();
    assert_eq!(report["color_correction"]["applied"], true);
    assert_eq!(report["color_correction"]["weight"], 1.0);
    assert!(server.render_png(&camera, (4097, 1), view).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(server.render_png(&camera, (8, 8), view).is_err());
    assert!(server.render_png(&camera, (8, 8), view).is_err());
    lock(&shared)
        .set_color_correction(false, 1.0, None)
        .unwrap();
    let disabled = server.render_png(&camera, (8, 8), view).unwrap();
    assert_eq!(pixels(&disabled), pixels(&baseline));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
}
