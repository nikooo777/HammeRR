use super::*;
use crate::recovery::{Authority, Settings};

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn update(server: &EditorServer, update: Value) -> Value {
    value(server.set_recovery_settings(Parameters(serde_json::from_value(update).unwrap())))
}

fn inspect(server: &EditorServer) -> Value {
    value(server.get_recovery(Parameters(serde_json::from_value(json!({})).unwrap())))["status"]
        .clone()
}

#[test]
fn paired_recovery_servers_merge_concurrent_partial_updates_and_reject_stale_forms() {
    let directory = tempfile::tempdir().unwrap();
    let settings_path = directory.path().join("settings");
    let authority = Authority::configured(Ok(settings_path.clone()));
    let mut first = Session::with_recovery_settings(Err("no assets".into()), authority.clone());
    let mut second = Session::with_recovery_settings(Err("no assets".into()), authority.clone());
    first.configure_recovery(Ok(directory.path().join("first")));
    second.configure_recovery(Ok(directory.path().join("second")));
    let first = EditorServer::new(Arc::new(Mutex::new(first)), None);
    let second = EditorServer::new(Arc::new(Mutex::new(second)), None);
    let stale = inspect(&second)["settings_revision"].as_u64().unwrap();
    update(&first, json!({"enabled":false,"interval_seconds":60}));
    update(&second, json!({"keep_per_map":3}));
    assert_eq!(
        inspect(&first)["settings"],
        json!({"enabled":false,"interval_seconds":60,"keep_per_map":3})
    );
    assert_eq!(
        inspect(&first)["settings_revision"],
        inspect(&second)["settings_revision"]
    );
    assert!(
        second
            .set_recovery_settings(Parameters(
                serde_json::from_value(json!({"expected_revision":stale,"enabled":true})).unwrap()
            ))
            .unwrap()
            .is_error
            .unwrap()
    );
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let first_update = scope.spawn(|| {
            barrier.wait();
            update(&first, json!({"enabled":true,"interval_seconds":90}))
        });
        let second_update = scope.spawn(|| {
            barrier.wait();
            update(&second, json!({"keep_per_map":7}))
        });
        let first_result = first_update.join().unwrap();
        let second_result = second_update.join().unwrap();
        assert_eq!(first_result["settings"]["enabled"], true);
        assert_eq!(first_result["settings"]["interval_seconds"], 90);
        assert_eq!(second_result["settings"]["keep_per_map"], 7);
        assert_ne!(
            first_result["settings_revision"],
            second_result["settings_revision"]
        );
    });
    let expected = Settings {
        enabled: true,
        interval_seconds: 90,
        keep_per_map: 7,
    };
    assert_eq!(authority.snapshot().settings, expected);
    let store = crate::recovery::Store {
        directory: settings_path.clone(),
    };
    assert_eq!(store.load_settings().unwrap(), expected);
    let revision = authority.snapshot().revision;
    std::fs::write(settings_path.join("settings.json"), b"broken").unwrap();
    assert!(
        first
            .reload_recovery_settings(Parameters(serde_json::from_value(json!({})).unwrap()))
            .unwrap()
            .is_error
            .unwrap()
    );
    assert_eq!(
        inspect(&first)["settings_error"],
        inspect(&second)["settings_error"]
    );
    assert_eq!(authority.snapshot().revision, revision);
    assert_eq!(authority.snapshot().settings, expected);
    update(&second, json!({"enabled":false}));
    assert!(inspect(&first)["settings_error"].is_null());
    assert!(!authority.snapshot().settings.enabled);
    for server in [&first, &second] {
        assert!(server.offscreen.lock().unwrap().renderer.is_none());
    }
}

#[test]
fn independent_recovery_servers_report_disk_conflicts_and_reload_without_graphics() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Session::new(Err("no assets".into()));
    let mut second = Session::new(Err("no assets".into()));
    first.configure_recovery(Ok(directory.path().into()));
    second.configure_recovery(Ok(directory.path().into()));
    let first = EditorServer::new(Arc::new(Mutex::new(first)), None);
    let second = EditorServer::new(Arc::new(Mutex::new(second)), None);
    let initial = inspect(&second);
    update(&first, json!({"interval_seconds":60,"keep_per_map":4}));
    let result = second
        .set_recovery_settings(Parameters(
            serde_json::from_value(json!({"enabled":false})).unwrap(),
        ))
        .unwrap();
    assert!(result.is_error.unwrap());
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("Reload settings")
    );
    let conflicted = inspect(&second);
    assert_eq!(conflicted["settings"], initial["settings"]);
    assert_eq!(
        conflicted["settings_revision"],
        initial["settings_revision"]
    );
    assert!(conflicted["settings_error"].is_string());
    value(second.reload_recovery_settings(Parameters(
        serde_json::from_value(json!({"expected_revision":initial["settings_revision"]})).unwrap(),
    )));
    let repaired = update(&second, json!({"enabled":false}));
    assert_eq!(
        repaired["settings"],
        json!({"enabled":false,"interval_seconds":60,"keep_per_map":4})
    );
    assert!(repaired["settings_error"].is_null());
    for server in [&first, &second] {
        assert!(server.offscreen.lock().unwrap().renderer.is_none());
    }
}
