use super::*;

#[test]
fn sky_packing_jobs_capture_and_freeze_settings_without_native_capture_requirements() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game, _) = crate::test_assets::game_process(true);
        let source_path = directory.path().join("room.vmf");
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(crate::test_assets::room()),
            Some(source_path.clone()),
        );
        std::fs::write(
            &source_path,
            session.document().vmf().write(&Default::default()),
        )
        .unwrap();
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_sky_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let request = session.begin_compile(actor, None).unwrap();
        assert!(request.options.pack_sky_assets);
        assert!(!request.options.generate_reflections);
        assert!(request.environment.is_none());
        let status = session.compile_status();
        assert_eq!(status["job"]["settings"]["pack_sky_assets"], true);
        assert_eq!(status["job"]["settings"]["generate_reflections"], false);
        assert_eq!(status["job"]["native_runtime"], Value::Null);
        assert_eq!(status["settings"]["editable"], false);
        assert_eq!(status["job"]["stale"], false);
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_sky_assets: Some(false),
                        lighting: Some("direct_ldr"),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(
            session.compile_status()["job"]["settings"],
            status["job"]["settings"]
        );
        session.reload_material_assets(actor);
        assert_eq!(session.compile_status()["job"]["stale"], true);
        session
            .cancel_compile(actor, session.compile_token().unwrap())
            .unwrap();
        let result = request.run();
        session.finish_compile(actor, &result);
        assert!(result.is_err());
        assert_eq!(session.compile_settings()["editable"], true);
        assert!(session.compile_options().pack_sky_assets);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn sky_packing_setting_alone_stales_previous_output_and_restoring_it_restores_readiness() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let mut session = Session::new(Ok(game));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(session.launch_readiness().is_ok());
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                pack_sky_assets: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_err());
    assert_eq!(session.compile_status()["job"]["stale"], true);
    assert_eq!(
        session.compile_status()["job"]["settings"]["pack_sky_assets"],
        false
    );
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                pack_sky_assets: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_ok());
    assert_eq!(session.compile_status()["job"]["stale"], false);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}
