use super::*;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Application, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery");
    let application =
        Application::isolated(Err("No content required".into()), path.clone()).unwrap();
    lock(application.document().session())
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    (directory, application, path)
}

#[cfg(unix)]
#[test]
fn application_rejects_raw_recovery_configuration_before_sharing_preferences() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"state-\xff".to_vec()));
    let mut application =
        Application::isolated(Err("No content required".into()), path.clone()).unwrap();
    let shared = application.document().session().clone();
    let session = lock(&shared);
    let status = session.recovery_status_snapshot().1;
    assert_eq!(status["available"], false);
    assert!(status["directory"].is_null());
    assert_eq!(
        status["storage_error"],
        "Recovery directory must be valid UTF-8"
    );
    assert_eq!(
        status["settings_error"],
        "Recovery directory must be valid UTF-8"
    );
    for report in [
        session.grid_preferences_state(),
        session.preview_preferences_state(),
        session.navigation_preferences_state(),
        session.selection_preferences_state(),
        session.keyboard_shortcuts_state(),
    ] {
        assert_eq!(report["ready"], false);
        assert!(report["path"].is_null());
        assert!(report["error"].as_str().unwrap().contains("valid UTF-8"));
        serde_json::to_vec(&report).unwrap();
    }
    let favorites = serde_json::to_value(session.material_favorite_status()).unwrap();
    assert_eq!(favorites["persistence"], "blocked");
    assert!(favorites["directory"].is_null());
    serde_json::to_vec(&session.recent_maps()).unwrap();
    drop(session);
    application.shutdown().unwrap();
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn application_preserves_utf8_recovery_alias_to_raw_storage() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};

    let directory = tempfile::tempdir().unwrap();
    let physical = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"state-\xff".to_vec()));
    std::fs::create_dir(&physical).unwrap();
    let alias = directory.path().join("state-alias");
    symlink(&physical, &alias).unwrap();
    let mut application =
        Application::isolated(Err("No content required".into()), alias.clone()).unwrap();
    let shared = application.document().session().clone();
    let mut session = lock(&shared);
    assert_eq!(
        session.recovery_status()["directory"],
        alias.to_str().unwrap()
    );
    let stamp = session.grid_preferences_stamp();
    session
        .change_grid_preferences(Actor::User, "save", &stamp)
        .unwrap();
    session
        .set_recovery_settings(Actor::User, recovery::Settings::default(), None)
        .unwrap();
    for report in [
        session.grid_preferences_state(),
        session.preview_preferences_state(),
        session.navigation_preferences_state(),
        session.selection_preferences_state(),
        session.keyboard_shortcuts_state(),
    ] {
        assert_eq!(report["ready"], true);
        assert!(report["error"].is_null());
        assert!(
            report["path"]
                .as_str()
                .unwrap()
                .starts_with(alias.to_str().unwrap())
        );
    }
    drop(session);
    application.shutdown().unwrap();
    assert!(physical.join("settings.json").is_file());
    assert!(physical.join(crate::grid_preferences::FILE_NAME).is_file());
    assert!(std::fs::symlink_metadata(&alias).unwrap().is_symlink());
}

#[test]
fn application_shutdown_drains_once_with_retained_session_and_drop_fallback() {
    for explicit in [false, true] {
        let (_directory, mut application, path) = fixture();
        let retained = application.document().session().clone();
        let original = lock(&retained).document().vmf().clone();
        if explicit {
            application.shutdown().unwrap();
            application.shutdown().unwrap();
        }
        drop(application);
        let state = lock(&retained);
        assert_eq!(state.document().vmf(), &original);
        assert_eq!(state.recovery_status()["pending_changes"], false);
        assert_eq!(state.recovery_status()["busy"], false);
        assert!(state.recovery_status()["error"].is_null());
        assert_eq!(
            recovery::Store { directory: path }
                .list()
                .unwrap()
                .entries
                .len(),
            1
        );
    }
}

#[test]
fn application_retains_prefab_owner_across_session_recovery_configuration() {
    let (_directory, mut application, path) = fixture();
    let mut peer = Session::with_prefabs(
        Err("No assets required".into()),
        application.prefabs.clone(),
    );
    peer.configure_recovery(Err("Peer document recovery unavailable".into()));
    let shared = application.document().session().clone();
    let mut session = lock(&shared);
    let ids = session.selected_ids();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .save_prefab(Actor::User, "Application owned", &ids, &stamp)
        .unwrap();
    assert_eq!(
        application.prefabs.snapshot().entries[0].name,
        "Application owned"
    );
    assert_eq!(
        peer.prefab_report("", 0, 64, None)["directory"],
        path.join("prefabs").to_str().unwrap()
    );
    assert_eq!(
        peer.prefab_report("", 0, 64, None)["entries"][0]["name"],
        "Application owned"
    );
    drop(session);
    application.shutdown().unwrap();
    drop(application);
    assert_eq!(
        peer.prefab_report("", 0, 64, None)["entries"][0]["name"],
        "Application owned"
    );
}

#[test]
fn application_finish_preserves_primary_and_shutdown_failures_and_retained_status() {
    for primary in [false, true] {
        let (_directory, application, path) = fixture();
        let retained = application.document().session().clone();
        std::fs::write(&path, b"not a directory").unwrap();
        let result = if primary {
            Err("injected GPU initialization failure".into())
        } else {
            Ok(())
        };
        let error = application.finish(result).unwrap_err().to_string();
        assert!(error.contains("Exit recovery snapshot failed"), "{error}");
        if primary {
            assert!(
                error.contains("injected GPU initialization failure"),
                "{error}"
            );
        }
        let status = lock(&retained).recovery_status();
        assert_eq!(status["pending_changes"], true);
        assert_eq!(status["busy"], false);
        assert!(
            status["error"]
                .as_str()
                .unwrap()
                .contains("Exit recovery snapshot failed")
        );
        assert_eq!(std::fs::read(path).unwrap(), b"not a directory");
    }
}

#[test]
fn application_finish_retains_primary_error_after_successful_exit_capture() {
    let (_directory, application, path) = fixture();
    let retained = application.document().session().clone();
    let error = application
        .finish(Err("injected application failure".into()))
        .unwrap_err();
    assert_eq!(error.to_string(), "injected application failure");
    assert_eq!(
        recovery::Store { directory: path }
            .list()
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(lock(&retained).recovery_status()["pending_changes"], false);
}

#[cfg(feature = "mcp")]
#[test]
fn application_finish_drains_even_while_mcp_and_document_handles_remain() {
    let (_directory, application, path) = fixture();
    let document = application.document();
    let session = document.session().clone();
    let server = crate::mcp::EditorServer::for_document(document.clone(), None);
    application.finish(Ok(())).unwrap();
    assert_eq!(lock(&session).recovery_status()["pending_changes"], false);
    lock(&session)
        .create_block(
            Actor::Agent,
            glam::DVec3::splat(32.0),
            glam::DVec3::splat(48.0),
            "late",
            false,
        )
        .unwrap();
    document.shutdown().unwrap();
    assert_eq!(lock(&session).recovery_status()["pending_changes"], true);
    drop(document);
    drop(server);
    assert_eq!(
        recovery::Store { directory: path }
            .list()
            .unwrap()
            .entries
            .len(),
        1
    );
}

#[test]
fn application_shutdown_waits_for_inflight_manual_capture_then_saves_newer_revision() {
    let (_directory, mut application, path) = fixture();
    let session = application.document().session().clone();
    let request = lock(&session)
        .begin_recovery_snapshot(recovery::Kind::Manual)
        .unwrap();
    lock(&session)
        .create_block(
            Actor::User,
            glam::DVec3::splat(32.0),
            glam::DVec3::splat(48.0),
            "newer",
            false,
        )
        .unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
    let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(0);
    let shutdown = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = application.shutdown();
        finished_tx.send(result).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        finished_rx.recv_timeout(Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    recovery::complete_capture(&session, Actor::User, &request).unwrap();
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    shutdown.join().unwrap();
    let store = recovery::Store { directory: path };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 2);
    for entry in entries {
        let expected = if entry.kind == recovery::Kind::Manual {
            1
        } else {
            2
        };
        let (_, text) = store.read(&entry.id).unwrap();
        assert_eq!(
            source_formats::vmf::Vmf::parse(&text)
                .unwrap()
                .world
                .solids
                .len(),
            expected
        );
    }
    assert_eq!(lock(&session).recovery_status()["pending_changes"], false);
}
