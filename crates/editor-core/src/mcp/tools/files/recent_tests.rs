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

#[test]
fn attached_mcp_servers_share_recent_maps_and_keep_the_originating_documents() {
    let directory = tempfile::tempdir().unwrap();
    let authority = crate::recent_maps::Authority::configured(Ok(directory.path().to_owned()));
    let first = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_recent_maps(
        Err("no assets".into()),
        authority,
    )));
    let first_server = EditorServer::new(first.clone(), None);
    let second_server = EditorServer::new(second.clone(), None);
    let a = directory.path().join("a.vmf");
    let b = directory.path().join("b.vmf");
    for (server, path) in [(&first_server, &a), (&second_server, &b)] {
        std::fs::write(path, "world { id 1 classname worldspawn }").unwrap();
        value(server.open_map(Parameters(OpenMapArgs {
            path: path.display().to_string(),
            discard_changes: false,
        })));
    }
    assert_eq!(
        value(first_server.get_recent_maps())["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        value(first_server.get_recent_maps()),
        value(second_server.get_recent_maps())
    );
    {
        let mut session = lock(&second);
        let stamp = session.measurement_stamp();
        session
            .preview_measurement(
                Actor::Agent,
                crate::session::measurements::Request {
                    id: None,
                    name: "Owned clearance".into(),
                    start: DVec3::ZERO,
                    end: DVec3::X,
                },
                &stamp,
            )
            .unwrap();
    }
    let draft = lock(&second).measurement_state()["draft"].clone();
    let source = lock(&second).document().vmf().clone();
    let history = lock(&second).history_snapshot();
    let serials = {
        let session = lock(&second);
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial(),
        )
    };
    value(
        first_server.remove_recent_map(Parameters(RecentMapPathArgs {
            path: b.display().to_string(),
        })),
    );
    assert_eq!(
        value(second_server.get_recent_maps())["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(lock(&second).path(), Some(b.as_path()));
    let saved = std::fs::read(directory.path().join(crate::recent_maps::FILE_NAME)).unwrap();
    std::fs::write(
        directory.path().join(crate::recent_maps::FILE_NAME),
        b"broken",
    )
    .unwrap();
    assert!(
        second_server
            .reload_recent_maps()
            .unwrap()
            .is_error
            .unwrap_or(false)
    );
    assert_eq!(
        value(first_server.get_recent_maps())["persistence"],
        "blocked"
    );
    let copy = directory.path().join("copy.vmf");
    value(first_server.save_map(Parameters(SaveMapArgs {
        path: Some(copy.display().to_string()),
    })));
    assert!(copy.is_file());
    std::fs::write(directory.path().join(crate::recent_maps::FILE_NAME), saved).unwrap();
    value(first_server.reload_recent_maps());
    assert_eq!(
        value(second_server.get_recent_maps())["persistence"],
        "persistent"
    );
    let session = lock(&second);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial()
        ),
        serials
    );
    assert_eq!(session.path(), Some(b.as_path()));
    drop(session);
    assert!(first_server.offscreen.lock().unwrap().renderer.is_none());
    assert!(second_server.offscreen.lock().unwrap().renderer.is_none());
}
