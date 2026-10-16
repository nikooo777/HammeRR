use super::*;

#[test]
fn reflection_settings_validate_the_whole_update_and_freeze_active_jobs() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game, _) = crate::test_assets::game_process(true);
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(crate::test_assets::room()),
            Some(directory.path().join("room.vmf")),
        );
        let source = session.document.vmf().clone();
        let revision = session.document.revision();
        assert_eq!(session.compile_settings()["generate_reflections"], false);
        let initial = session.compile_settings();
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        generate_reflections: Some(true),
                        physics: Some("engine"),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(session.compile_settings(), initial);
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting: Some("direct_ldr"),
                    generate_reflections: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let enabled = session.compile_settings();
        assert!(session.set_compile_settings(actor, "fullbright").is_err());
        assert_eq!(session.compile_settings(), enabled);
        session
            .set_compile_settings(actor, "direct_ldr_hdr")
            .unwrap();
        assert_eq!(session.compile_settings()["generate_reflections"], true);
        assert_eq!(session.compile_settings()["lighting"], "direct_ldr_hdr");
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting: Some("fullbright"),
                    generate_reflections: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        let request = session.begin_compile(actor, None).unwrap();
        assert_eq!(
            session.compile_status()["job"]["settings"]["generate_reflections"],
            false
        );
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting: Some("direct_ldr"),
                        generate_reflections: Some(true),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(!session.compile_options().generate_reflections);
        assert_eq!(
            session.compile_status()["job"]["settings"]["generate_reflections"],
            false
        );
        session
            .cancel_compile(actor, session.compile_token().unwrap())
            .unwrap();
        let result = request.run();
        session.finish_compile(actor, &result);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.document.undo_labels().count(), 0);
    }
}

#[test]
fn reflection_setting_alone_stales_a_disabled_ldr_build() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let mut session = Session::new(Ok(game));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    session
        .set_compile_settings(Actor::User, "direct_ldr")
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(session.launch_readiness().is_ok());
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                generate_reflections: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_err());
    assert_eq!(session.compile_status()["job"]["stale"], true);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                generate_reflections: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.launch_readiness().is_ok());
}
