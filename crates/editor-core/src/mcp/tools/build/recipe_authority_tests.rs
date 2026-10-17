use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn attached_servers_share_recipe_registry_and_keep_action_guards_and_options_local() {
    let authority = crate::build_recipes::Authority::default();
    let first = Arc::new(Mutex::new(Session::with_build_recipes(
        Err("no assets".into()),
        authority.clone(),
    )));
    let second = Arc::new(Mutex::new(Session::with_build_recipes(
        Err("no assets".into()),
        authority,
    )));
    let a = EditorServer::new(first.clone(), None);
    let b = EditorServer::new(second.clone(), None);
    let saved = value(a.save_build_recipe(Parameters(
        serde_json::from_value(json!({"name":"Shared from MCP"})).unwrap(),
    )));
    let id = saved["active"]["id"].as_str().unwrap();
    let before = value(b.get_build_recipes());
    assert_eq!(before["recipes"], saved["recipes"]);
    value(b.set_compile_settings(Parameters(
        serde_json::from_value(json!({"physics":"engine"})).unwrap(),
    )));
    let stale = b
        .apply_build_recipe(Parameters(
            serde_json::from_value(json!({"id":id,"revision":before["revision"]})).unwrap(),
        ))
        .unwrap();
    assert!(stale.is_error.unwrap_or(false));
    let current = value(b.get_build_recipes());
    value(
        b.rename_build_recipe(Parameters(
            serde_json::from_value(
                json!({"id":id,"name":"Peer named","revision":current["revision"]}),
            )
            .unwrap(),
        )),
    );
    assert_eq!(
        value(a.get_compile_settings())["recipe"]["name"],
        "Peer named"
    );
    assert_eq!(value(a.get_compile_settings())["physics"], "none");
    assert_eq!(value(b.get_compile_settings())["physics"], "engine");
    value(b.delete_build_recipe(Parameters(
        serde_json::from_value(json!({"id":id})).unwrap(),
    )));
    assert!(value(a.get_compile_settings())["recipe"].is_null());
    assert_eq!(lock(&first).document().undo_labels().count(), 0);
    assert_eq!(lock(&second).document().undo_labels().count(), 0);
    assert!(a.offscreen.lock().unwrap().renderer.is_none());
    assert!(b.offscreen.lock().unwrap().renderer.is_none());
}
