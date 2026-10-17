use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn set(server: &EditorServer, material: &str) -> ToolResult {
    server.set_material_favorite(Parameters(FavoriteMaterialArgs {
        material: material.into(),
        favorite: true,
    }))
}

#[test]
fn favorite_mcp_conflicts_and_reload_share_cpu_only_session_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Session::new(Err("no graphics".into()));
    first.configure_recovery(Ok(directory.path().to_owned()));
    let mut second = Session::new(Err("no graphics".into()));
    second.configure_recovery(Ok(directory.path().to_owned()));
    second
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let token = second.property_draft().unwrap().token;
    let serials = (
        second.scene_serial(),
        second.view_serial(),
        second.camera_serial(),
        second.materials().generation(),
    );
    let first = EditorServer::new(Arc::new(Mutex::new(first)), None);
    let shared = Arc::new(Mutex::new(second));
    let second = EditorServer::new(shared.clone(), None);
    assert_eq!(value(set(&first, "first"))["favorite"], true);
    assert!(set(&second, "second").unwrap().is_error.unwrap());
    let reloaded = value(second.reload_material_favorites());
    assert_eq!(reloaded["favorites"]["count"], 1);
    assert_eq!(reloaded["favorites"]["reload_available"], true);
    assert!(reloaded["favorites"]["error"].is_null());
    assert_eq!(value(set(&second, "second"))["favorites"]["count"], 2);
    let path = directory.path().join(crate::material_favorites::FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    assert!(
        second
            .reload_material_favorites()
            .unwrap()
            .is_error
            .unwrap()
    );
    assert!(lock(&shared).is_material_favorite("second"));
    std::fs::write(&path, valid).unwrap();
    assert_eq!(
        value(second.reload_material_favorites())["favorites"]["persistence"],
        "persistent"
    );
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.property_draft().unwrap().token, token);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial(),
            session.materials().generation()
        ),
        serials
    );
    assert!(first.offscreen.lock().unwrap().renderer.is_none());
    assert!(second.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn attached_mcp_sessions_observe_one_favorites_authority_and_keep_separate_property_drafts() {
    let authority = crate::material_favorites::Authority::default();
    let mut first = Session::with_material_favorites(Err("no graphics".into()), authority.clone());
    let mut second = Session::with_material_favorites(Err("no graphics".into()), authority);
    for session in [&mut first, &mut second] {
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
    }
    let first_token = first.property_draft().unwrap().token;
    let second_token = second.property_draft().unwrap().token;
    let first_shared = Arc::new(Mutex::new(first));
    let second_shared = Arc::new(Mutex::new(second));
    let first = EditorServer::new(first_shared.clone(), None);
    let second = EditorServer::new(second_shared.clone(), None);
    assert_eq!(value(set(&first, "first"))["favorites"]["count"], 1);
    assert_eq!(value(set(&second, "second"))["favorites"]["count"], 2);
    let catalog = value(first.browse_materials(Parameters(
        serde_json::from_value(json!({"group":"favorites"})).unwrap(),
    )));
    assert_eq!(catalog["total"], 2);
    assert_eq!(catalog["favorites"]["count"], 2);
    assert!(
        catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["favorite"] == true)
    );
    assert!(set(&second, "../invalid").unwrap().is_error.unwrap());
    let catalog = value(first.browse_materials(Parameters(
        serde_json::from_value(json!({"group":"favorites"})).unwrap(),
    )));
    assert!(catalog["favorites"]["error"].is_string());
    value(
        first.set_material_favorite(Parameters(FavoriteMaterialArgs {
            material: "first".into(),
            favorite: false,
        })),
    );
    assert!(!lock(&second_shared).is_material_favorite("first"));
    assert!(
        lock(&second_shared)
            .material_favorite_status()
            .error
            .is_none()
    );
    assert_eq!(
        lock(&first_shared).property_draft().unwrap().token,
        first_token
    );
    assert_eq!(
        lock(&second_shared).property_draft().unwrap().token,
        second_token
    );
    assert!(first.offscreen.lock().unwrap().renderer.is_none());
    assert!(second.offscreen.lock().unwrap().renderer.is_none());
}
