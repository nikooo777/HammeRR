use super::*;

#[test]
fn static_prop_packing_settings_support_all_lighting_modes_and_reject_updates_atomically() {
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
        assert_eq!(session.compile_settings()["pack_static_prop_assets"], false);
        assert!(
            session.compile_settings()["static_prop_packing_description"]
                .as_str()
                .unwrap()
                .contains("DX90 or newer")
        );
        for lighting in ["fullbright", "direct_ldr", "direct_ldr_hdr"] {
            let settings = session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_static_prop_assets: Some(true),
                        lighting: Some(lighting),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(settings["pack_static_prop_assets"], true);
            assert_eq!(settings["pack_sky_assets"], false);
            assert_eq!(settings["bake_static_prop_vertices"], false);
            assert_eq!(settings["generate_reflections"], false);
            assert_eq!(settings["lighting"], lighting);
            assert!(session.compile_options().pack_static_prop_assets);
            assert!(session.compile_options().requires_guarded_publication());
        }
        for lighting in ["direct_ldr", "direct_ldr_hdr"] {
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting: Some(lighting),
                        bake_static_prop_vertices: Some(true),
                        pack_static_prop_assets: Some(false),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(session.compile_options().bake_static_prop_vertices);
            assert!(!session.compile_options().pack_static_prop_assets);
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_static_prop_assets: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(session.compile_options().bake_static_prop_vertices);
            assert!(session.compile_options().pack_static_prop_assets);
        }
        let enabled = session.compile_settings();
        for invalid in [
            BuildSettingsUpdate {
                pack_static_prop_assets: Some(false),
                lighting: Some("unknown"),
                ..Default::default()
            },
            BuildSettingsUpdate {
                pack_static_prop_assets: Some(false),
                smoothing_angle: Some(91),
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
        session.closing = true;
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_static_prop_assets: Some(false),
                        ..Default::default()
                    },
                )
                .is_err()
        );
        session.closing = false;
        assert_eq!(session.compile_settings(), enabled);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(json!(session.property_draft()), draft);
    }
}

#[test]
fn static_prop_packing_jobs_freeze_settings_and_setting_changes_stale_published_output() {
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
                    pack_static_prop_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.launch_readiness().is_err());
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert_eq!(
            session.compile_status()["job"]["settings"]["pack_static_prop_assets"],
            false
        );
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_static_prop_assets: Some(false),
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
                    pack_static_prop_assets: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let request = session.begin_compile(actor, None).unwrap();
        assert!(request.options.pack_static_prop_assets);
        assert!(!request.options.pack_sky_assets);
        assert!(!request.options.generate_reflections);
        assert!(request.environment.is_none());
        let status = session.compile_status();
        assert_eq!(status["job"]["settings"]["pack_static_prop_assets"], true);
        assert_eq!(status["job"]["native_runtime"], Value::Null);
        assert_eq!(status["settings"]["editable"], false);
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        pack_static_prop_assets: Some(false),
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
        assert!(session.compile_options().pack_static_prop_assets);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
}
