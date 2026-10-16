use super::*;

#[test]
fn sky_packing_settings_are_atomic_and_preserve_document_camera_and_foreign_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("settings need no asset access".into()));
        let brush = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "test/unchanged",
                false,
            )
            .unwrap();
        session
            .preview_transform(
                Actor::Agent,
                &[brush],
                map_core::edit::Transform {
                    translation: DVec3::X * 16.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                PropertyChange::Set {
                    key: "skyname".into(),
                    value: "pending_family".into(),
                },
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let rendered = session.render_document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let draft = json!(session.property_draft());
        let clipboard = session.clipboard_report();
        assert_eq!(session.compile_settings()["pack_sky_assets"], false);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_sky_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let enabled = session.compile_settings();
        assert_eq!(enabled["pack_sky_assets"], true);
        assert_eq!(enabled["lighting"], "fullbright");
        assert_eq!(enabled["generate_reflections"], false);
        assert!(session.compile_options().pack_sky_assets);
        for invalid in [
            BuildSettingsUpdate {
                pack_sky_assets: Some(false),
                lighting: Some("unknown"),
                ..Default::default()
            },
            BuildSettingsUpdate {
                pack_sky_assets: Some(false),
                generate_reflections: Some(true),
                ..Default::default()
            },
        ] {
            assert!(session.update_build_settings(actor, invalid).is_err());
            assert_eq!(session.compile_settings(), enabled);
        }
        session
            .update_build_settings(actor, BuildSettingsUpdate::default())
            .unwrap();
        assert_eq!(session.compile_settings(), enabled);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    bounces: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.compile_options().pack_sky_assets);
        let selected = session.compile_settings();
        session.closing = true;
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_sky_assets: Some(false),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        session.closing = false;
        assert_eq!(session.compile_settings(), selected);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(session.clipboard_report(), clipboard);
    }
}

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
