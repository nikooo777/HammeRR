use super::*;

#[test]
fn water_packing_jobs_freeze_settings_and_setting_changes_stale_published_output() {
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
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        assert!(session.launch_readiness().is_ok());
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_water_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.launch_readiness().is_err());
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert_eq!(
            session.compile_status()["job"]["settings"]["pack_water_assets"],
            false
        );
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_water_assets: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.launch_readiness().is_ok());
        assert_eq!(session.compile_status()["job"]["stale"], false);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_water_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let request = session.begin_compile(actor, None).unwrap();
        assert!(request.options.pack_water_assets);
        assert!(!request.options.pack_sky_assets);
        assert!(!request.options.generate_reflections);
        assert!(request.environment.is_none());
        let status = session.compile_status();
        assert_eq!(status["job"]["settings"]["pack_water_assets"], true);
        assert_eq!(status["job"]["native_runtime"], Value::Null);
        assert_eq!(status["settings"]["editable"], false);
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_water_assets: Some(false),
                        ..Default::default()
                    },
                )
                .is_err()
        );
        assert_eq!(
            session.compile_status()["job"]["settings"],
            status["job"]["settings"]
        );
        session
            .cancel_compile(actor, session.compile_token().unwrap())
            .unwrap();
        let result = request.run();
        session.finish_compile(actor, &result);
        assert!(result.is_err());
        assert_eq!(session.compile_settings()["editable"], true);
        assert!(session.compile_options().pack_water_assets);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
}
