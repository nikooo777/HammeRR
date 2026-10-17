use super::*;

fn fixture() -> (tempfile::TempDir, Application, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery");
    let material_favorites = material_favorites::Authority::configured(Ok(path.clone()));
    let recent_maps = recent_maps::Authority::configured(recent_maps::directory(Ok(path.clone())));
    let shortcuts = shortcuts::Authority::configured(Ok(path.clone()));
    let saved_defaults = crate::saved_defaults::SavedDefaults::configured(Ok(path.clone()));
    let mut state = Session::with_material_favorites(
        Err("No content required".into()),
        material_favorites.clone(),
    );
    state.use_application_recent_maps(recent_maps.clone());
    state.use_application_shortcuts(shortcuts.clone());
    state.use_application_saved_defaults(saved_defaults.clone());
    let build_recipes = build_recipes::Authority::configured(Ok(path.clone()));
    state.use_application_build_recipes(build_recipes.clone());
    let recovery_settings = recovery::Authority::configured(Ok(path.clone()));
    state.use_application_recovery_settings(recovery_settings.clone());
    state.configure_recovery(Ok(path.clone()));
    state
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let session = Arc::new(Mutex::new(state));
    let worker = recovery::Worker::start(&session).unwrap();
    let application = Application {
        _shutdown: ShutdownGuard {
            session: session.clone(),
            drained: false,
        },
        _recovery_worker: worker,
        material_favorites,
        recent_maps,
        shortcuts,
        saved_defaults,
        build_recipes,
        recovery_settings,
        session,
        open_message: None,
        finished: None,
    };
    (directory, application, path)
}

#[test]
fn application_shutdown_drains_once_with_retained_session_and_drop_fallback() {
    for explicit in [false, true] {
        let (_directory, mut application, path) = fixture();
        let retained = application.session.clone();
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
fn application_finish_preserves_primary_and_shutdown_failures_and_retained_status() {
    for primary in [false, true] {
        let (_directory, application, path) = fixture();
        let retained = application.session.clone();
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
    let retained = application.session.clone();
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

#[test]
fn application_shutdown_waits_for_inflight_manual_capture_then_saves_newer_revision() {
    let (_directory, mut application, path) = fixture();
    let session = application.session.clone();
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
