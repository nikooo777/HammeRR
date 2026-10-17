use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn explicit_mcp_reload_recovers_conflicts_without_touching_the_owned_document_review() {
    let directory = tempfile::tempdir().unwrap();
    let mut current = Session::new(Err("no assets needed".into()));
    current.configure_recent_maps(Ok(directory.path().into()));
    let first = directory.path().join("first.vmf");
    std::fs::write(
        &first,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    current.open(Actor::Agent, &first, false).unwrap();
    current
        .preview_measurement(
            Actor::Agent,
            crate::session::measurements::Request {
                id: None,
                name: "Owned clearance".into(),
                start: DVec3::ZERO,
                end: DVec3::X,
            },
            &current.measurement_stamp(),
        )
        .unwrap();
    let owned = current.measurement_state()["draft"].clone();
    let source = current.document().vmf().clone();
    let camera = current.camera();
    let shared = Arc::new(Mutex::new(current));
    let server = EditorServer::new(shared.clone(), None);
    let map = directory.path().join("remembered.vmf");
    std::fs::write(&map, crate::test_assets::room().write(&Default::default())).unwrap();
    let mut other = Session::new(Err("no assets needed".into()));
    other.configure_recent_maps(Ok(directory.path().into()));
    other.open(Actor::User, &map, false).unwrap();
    assert!(
        server
            .clear_recent_maps()
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(value(server.get_recent_maps())["persistence"], "blocked");
    let loaded = value(server.reload_recent_maps());
    assert_eq!(loaded, value(server.get_recent_maps()));
    assert_eq!(loaded, lock(&shared).recent_maps());
    assert_eq!(loaded["entries"][0]["path"], map.display().to_string());
    let path = directory.path().join(crate::recent_maps::FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"malformed preferences").unwrap();
    assert!(
        server
            .reload_recent_maps()
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(
        value(server.get_recent_maps())["entries"],
        loaded["entries"]
    );
    std::fs::write(&path, valid).unwrap();
    value(server.reload_recent_maps());
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.camera(), camera);
    assert_eq!(state.measurement_state()["draft"], owned);
    assert_eq!(state.document().undo_labels().count(), 0);
    drop(state);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
