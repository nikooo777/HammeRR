use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn prefabs(server: &EditorServer) -> Value {
    value(server.get_prefabs(Parameters(serde_json::from_value(json!({})).unwrap())))
}

#[test]
fn paired_servers_share_catalog_receipts_and_errors_while_paste_remains_local() {
    let directory = tempfile::tempdir().unwrap();
    let authority = crate::prefabs::Authority::from_directory(Ok(directory.path().into()));
    let mut first = Session::with_prefabs(Err("No assets required".into()), authority.clone());
    let second = Session::with_prefabs(Err("No assets required".into()), authority);
    first
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let first_shared = Arc::new(Mutex::new(first));
    let second_shared = Arc::new(Mutex::new(second));
    let first = EditorServer::new(first_shared.clone(), None);
    let second = EditorServer::new(second_shared.clone(), None);
    let first_history = lock(&first_shared).history_snapshot();
    let second_history = lock(&second_shared).history_snapshot();
    let old_second = prefabs(&second);
    let initial = prefabs(&first);
    let saved = value(first.save_prefab(Parameters(SavePrefabArgs {
        name: "Shared".into(),
        snapshot: initial["snapshot"].as_str().unwrap().into(),
        ids: None,
    })));
    assert_eq!(saved["write_committed"], true);
    assert_eq!(prefabs(&second)["entries"][0]["name"], "Shared");
    assert_eq!(
        prefabs(&first)["registry_revision"],
        prefabs(&second)["registry_revision"]
    );
    let stale = second
        .delete_prefab(Parameters(PrefabEntryArgs {
            name: "Shared".into(),
            snapshot: old_second["snapshot"].as_str().unwrap().into(),
        }))
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    let preview = value(
        second.preview_prefab(Parameters(
            serde_json::from_value(json!({
                "name":"Shared","snapshot":prefabs(&second)["snapshot"],"position":[64.0,0.0,0.0]
            }))
            .unwrap(),
        )),
    );
    assert!(lock(&first_shared).clipboard_report()["preview"].is_null());
    assert!(preview["paste"]["preview"]["token"].is_u64());
    let renamed = value(first.rename_prefab(Parameters(RenamePrefabArgs {
        name: "Shared".into(),
        new_name: "Renamed".into(),
        snapshot: prefabs(&first)["snapshot"].as_str().unwrap().into(),
    })));
    assert_eq!(renamed["write_committed"], true);
    assert_eq!(prefabs(&second)["entries"][0]["name"], "Renamed");
    assert_eq!(
        lock(&second_shared).clipboard_report()["preview"]["valid"],
        false
    );
    std::fs::create_dir(directory.path().join("Blocked.vmf")).unwrap();
    let blocked = prefabs(&second);
    assert!(blocked["error"].is_string());
    assert_eq!(blocked["entries"], json!([]));
    assert_eq!(prefabs(&first)["error"], blocked["error"]);
    std::fs::remove_dir(directory.path().join("Blocked.vmf")).unwrap();
    assert!(prefabs(&first)["error"].is_null());
    let deleted = value(second.delete_prefab(Parameters(PrefabEntryArgs {
        name: "Renamed".into(),
        snapshot: prefabs(&second)["snapshot"].as_str().unwrap().into(),
    })));
    assert_eq!(deleted["write_committed"], true);
    assert_eq!(prefabs(&first)["total"], 0);
    assert_eq!(lock(&first_shared).history_snapshot(), first_history);
    assert_eq!(lock(&second_shared).history_snapshot(), second_history);
    assert!(first.offscreen.lock().unwrap().renderer.is_none());
    assert!(second.offscreen.lock().unwrap().renderer.is_none());
}
