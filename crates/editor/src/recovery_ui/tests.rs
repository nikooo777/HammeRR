use super::*;
use std::sync::Mutex;

#[test]
fn recovery_warning_keeps_error_domains_with_listing_failures_and_skipped_entries() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery");
    let mut state = Session::new(Err("no assets".into()));
    state.configure_recovery(Ok(path.clone()));
    std::fs::create_dir_all(path.join("settings.json")).unwrap();
    let settings_error = state
        .set_recovery_settings(Actor::User, Settings::default(), None)
        .unwrap_err();
    let request = state.begin_recovery_snapshot(Kind::Manual).unwrap();
    state
        .finish_recovery_snapshot(Actor::Agent, &request, Err("snapshot write failed".into()))
        .unwrap_err();
    let source = state.document().vmf().clone();
    let history = state.history_snapshot();
    let shared = Arc::new(Mutex::new(state));
    let window = RecoveryWindow::new().unwrap();
    bind(&window, &shared);
    let moved = directory.path().join("moved");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::write(&path, b"not a directory").unwrap();
    refresh(&window, &lock(&shared));
    assert!(window.get_warning().contains(settings_error.as_str()));
    assert!(window.get_warning().contains("snapshot write failed"));
    assert!(window.get_warning().contains(path.to_str().unwrap()));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&moved, &path).unwrap();
    std::fs::write(
        path.join(format!("{}.json", recovery::unique_id())),
        b"broken",
    )
    .unwrap();
    refresh(&window, &lock(&shared));
    assert!(window.get_warning().contains(settings_error.as_str()));
    assert!(window.get_warning().contains("snapshot write failed"));
    assert!(window.get_warning().contains("incomplete or unreadable"));
    std::fs::remove_dir(path.join("settings.json")).unwrap();
    window.set_interval_seconds(60);
    window.set_settings_dirty(true);
    window.invoke_apply_settings();
    assert!(lock(&shared).recovery_status()["settings_error"].is_null());
    assert!(window.get_warning().contains("snapshot write failed"));
    let state = lock(&shared);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
}
