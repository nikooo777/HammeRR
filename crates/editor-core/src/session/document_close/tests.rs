use super::*;

fn fixture() -> (tempfile::TempDir, Session) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    (directory, session)
}

#[test]
fn save_identity_replacement_and_foreign_scope_expire_close_review() {
    let (directory, mut session) = fixture();
    let review = session.document_close_review();
    let path = directory.path().join("source.vmf");
    session.save(Actor::User, Some(&path)).unwrap();
    assert!(session.ensure_document_close_review(&review, true).is_err());
    let review = session.document_close_review();
    session
        .save(Actor::User, Some(&directory.path().join("copy.vmf")))
        .unwrap();
    assert!(session.ensure_document_close_review(&review, true).is_err());
    let review = session.document_close_review();
    session.new_map(Actor::User, true).unwrap();
    assert!(session.ensure_document_close_review(&review, true).is_err());
    let (_, foreign) = fixture();
    assert!(
        foreign
            .ensure_document_close_review(&session.document_close_review(), true)
            .is_err()
    );
}

#[test]
fn same_token_nonrender_group_update_expires_review_without_committing() {
    let (_directory, mut session) = fixture();
    let draft = session.begin_group_edit(None, false).unwrap();
    let token = draft["token"].as_u64().unwrap();
    let review = session.document_close_review();
    let history = session.history_snapshot();
    session
        .update_group_edit(token, Some("pending group".into()), None, None)
        .unwrap();
    assert_eq!(session.group_edit_report()["token"], token);
    assert!(session.ensure_document_close_review(&review, true).is_err());
    assert_eq!(session.history_snapshot(), history);
    assert!(session.document_close_review().requires_discard());
}

#[test]
fn repeated_close_inspection_preserves_the_observation() {
    let (_directory, session) = fixture();
    let review = session.document_close_review();
    for _ in 0..3 {
        assert_eq!(session.document_close_review(), review);
        assert_eq!(review.inspection()["discard_required"], false);
        assert!(!review.token().is_empty());
    }
    assert_eq!(session.document_close_review(), review);
}

#[test]
fn same_token_empty_paint_cursor_update_expires_review_without_a_scene_change() {
    let (_directory, mut session) = fixture();
    let pointer = Pointer {
        position: DVec2::splat(32.0),
        size: (320, 240),
    };
    let token = session
        .begin_paint_selection(paint::Spec {
            camera: session.camera(),
            pointer,
            radius: 4.0,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap();
    let review = session.document_close_review();
    let scene = session.scene_serial();
    let view = session.view_serial();
    session
        .update_active_stroke(token, &[DVec2::new(40.0, 32.0)])
        .unwrap();
    assert_eq!(session.paint_stroke().unwrap().token, token);
    assert_eq!(session.scene_serial(), scene);
    assert!(session.view_serial() > view);
    assert!(session.ensure_document_close_review(&review, true).is_err());
}

#[test]
fn invalid_transform_update_expires_review_without_committing_a_preview() {
    let (_directory, mut session) = fixture();
    let object = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(32.0), "test", false)
        .unwrap();
    session.select(&[object], SelectMode::Replace).unwrap();
    session
        .set_transform_tool(
            Actor::User,
            TransformTool {
                mode: Some(render::handles::Mode::Scale),
                ..TransformTool::default()
            },
        )
        .unwrap();
    session
        .begin_transform_drag(render::handles::Handle::Uniform, None)
        .unwrap();
    let review = session.document_close_review();
    let scene = session.scene_serial();
    let view = session.view_serial();
    assert!(
        session
            .update_transform_drag(None, Some(DVec2::new(-2.0, 0.0)))
            .is_err()
    );
    assert_eq!(session.scene_serial(), scene);
    assert_eq!(session.view_serial(), view);
    assert!(session.ensure_document_close_review(&review, true).is_err());
}

#[test]
fn changed_shared_recovery_settings_expire_an_otherwise_unchanged_review() {
    let (_directory, mut session) = fixture();
    let review = session.document_close_review();
    session
        .update_recovery_settings(
            Actor::User,
            crate::recovery::SettingsUpdate {
                enabled: Some(false),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert!(session.ensure_document_close_review(&review, true).is_err());
}

#[test]
fn accepted_reviewed_close_stops_the_owned_run_with_retained_session_owners() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let game_dir = game.game_dir.clone();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    let compiled = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &compiled);
    drop(compiled.unwrap());
    let request = session.begin_launch(Actor::Agent).unwrap();
    let run = session
        .finish_launch(Actor::Agent, &request.origin(), request.run())
        .unwrap();
    let shared = Arc::new(std::sync::Mutex::new(session));
    let document = crate::runtime::DocumentHandle::start(shared.clone()).unwrap();
    let review = document.document_close_review().unwrap();
    document.try_commit_reviewed_close(&review, true).unwrap();
    document.shutdown().unwrap();
    assert!(!Path::new(&format!("/proc/{}", run.pid)).exists());
    assert!(
        !game_dir
            .join("maps")
            .join(format!("{}.bsp", run.map))
            .exists()
    );
    assert!(!crate::runtime::lock(&shared).game_running());
}

#[test]
fn physical_compile_completion_after_early_closing_finishes_without_adoption() {
    let (directory, game, _) = crate::test_assets::game_process(false);
    let claims = crate::path_claims::Authority::default();
    let mut session = Session::new(Ok(game));
    session.use_application_path_claims(claims.clone());
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    session.save(Actor::User, None).unwrap();
    let output = directory.path().join("room.bsp");
    let previous_output = b"previous output before Close";
    std::fs::write(&output, previous_output).unwrap();
    let request = session.begin_compile(Actor::User, None).unwrap();
    let shared = Arc::new(std::sync::Mutex::new(session));
    let document = crate::runtime::DocumentHandle::start(shared.clone()).unwrap();
    let review = document.document_close_review().unwrap();
    document.try_commit_reviewed_close(&review, true).unwrap();
    let completed = request.run();
    assert!(completed.as_ref().unwrap().output.bsp.is_some());
    assert!(!completed.as_ref().unwrap().published());
    {
        let mut session = crate::runtime::lock(&shared);
        session.finish_compile(Actor::User, &completed);
        assert!(session.compiled.is_none());
        assert!(session.leak.is_none());
        assert!(!session.compiling);
        assert_eq!(session.compile_status()["job"]["status"], "failed");
        assert_eq!(
            session.compile_status()["job"]["result"]["published"],
            false
        );
    }
    let error = completed.as_ref().unwrap().publication_error().unwrap();
    assert_eq!(
        error,
        "Map, content, runtime or build settings changed during build publication; previous output preserved"
    );
    assert_eq!(std::fs::read(&output).unwrap(), previous_output);
    assert!(!output.with_extension("lin").exists());
    document.shutdown().unwrap();
    document.finalize_closed_claims().unwrap();
    let mut peer = Session::new(Err("No content needed".into()));
    peer.use_application_path_claims(claims);
    for name in ["room.vmf", "room.bsp", "room.lin"] {
        peer.save(Actor::Agent, Some(&directory.path().join(name)))
            .unwrap();
    }
    assert!(!completed.as_ref().unwrap().published());
    assert_eq!(completed.as_ref().unwrap().publication_error(), Some(error));
    drop(completed);
}
