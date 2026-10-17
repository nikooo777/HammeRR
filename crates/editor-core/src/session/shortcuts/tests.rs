use super::*;

fn fixture() -> (tempfile::TempDir, Session) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_shortcuts(Ok(directory.path().to_owned()));
    (directory, session)
}

fn custom() -> Bindings {
    Bindings {
        undo: [
            Some(Chord {
                key: "F6".into(),
                control: true,
                shift: false,
            }),
            None,
        ],
        duplicate_selection: [None, None],
        ..Default::default()
    }
}

fn save(session: &mut Session, actor: Actor, bindings: Bindings) -> Value {
    session
        .change_keyboard_shortcuts(actor, &session.shortcuts_stamp(), "save", Some(bindings))
        .unwrap()
}

#[test]
fn shared_saved_bindings_survive_restart_without_map_history_or_camera_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session) = fixture();
        let before = session.document.vmf().clone();
        let camera = session.camera();
        let dirty = session.document.is_dirty();
        let revision = session.document.revision();
        let selection = session.selected_ids();
        let state = save(&mut session, actor, custom());
        assert_eq!(state["write_committed"], true);
        assert_eq!(state["ready"], true);
        assert_eq!(state["stored"], true);
        assert_eq!(
            session.resolve_editor_shortcut("f6", true, false, false, false),
            Some(Command::Undo)
        );
        assert_eq!(
            session.resolve_editor_shortcut("F6", true, true, false, false),
            None
        );
        assert_eq!(
            session.resolve_editor_shortcut("F6", true, false, true, false),
            None
        );
        assert_eq!(
            session.resolve_editor_shortcut("F6", true, false, false, true),
            None
        );
        assert_eq!(
            session.resolve_editor_shortcut("D", true, false, false, false),
            None
        );
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.document.is_dirty(), dirty);
        assert_eq!(session.document.undo_labels().count(), 0);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), selection);
        let mut restarted = Session::new(Err("no assets needed".into()));
        restarted.configure_shortcuts(Ok(directory.path().to_owned()));
        assert_eq!(restarted.shortcut_bindings(), &custom());
    }
}

#[test]
fn invalid_reload_retains_current_and_blocks_save_until_successful_reload_or_explicit_reset() {
    let (directory, mut session) = fixture();
    save(&mut session, Actor::User, custom());
    let path = directory.path().join(crate::shortcuts::FILE_NAME);
    for bytes in [
        b"malformed".to_vec(),
        vec![b' '; crate::shortcuts::MAX_FILE_BYTES as usize + 1],
    ] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            session
                .change_keyboard_shortcuts(Actor::Agent, &session.shortcuts_stamp(), "reload", None)
                .is_err()
        );
        assert_eq!(session.shortcut_bindings(), &custom());
        assert_eq!(session.keyboard_shortcuts_state()["ready"], false);
        assert!(session.shortcuts_error_message().is_some());
        assert!(
            session
                .change_keyboard_shortcuts(
                    Actor::User,
                    &session.shortcuts_stamp(),
                    "save",
                    Some(Bindings::default())
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let mut restarted = Session::new(Err("no assets needed".into()));
    restarted.configure_shortcuts(Ok(directory.path().to_owned()));
    assert_eq!(restarted.shortcut_bindings(), &Bindings::default());
    assert_eq!(restarted.keyboard_shortcuts_state()["ready"], false);
    assert_eq!(
        std::fs::read(&path).unwrap().len(),
        crate::shortcuts::MAX_FILE_BYTES as usize + 1
    );
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reset", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), &Bindings::default());
    assert_eq!(session.keyboard_shortcuts_state()["ready"], true);
    save(&mut session, Actor::Agent, custom());
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), &custom());
}

#[test]
fn stale_actor_forms_and_external_source_replacement_never_overwrite_saved_bindings() {
    let (directory, mut session) = fixture();
    let initial = session.shortcuts_stamp();
    save(&mut session, Actor::Agent, custom());
    let path = directory.path().join(crate::shortcuts::FILE_NAME);
    let saved = std::fs::read(&path).unwrap();
    assert!(
        session
            .change_keyboard_shortcuts(Actor::User, &initial, "save", Some(Bindings::default()))
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    let store = Store {
        directory: directory.path().to_owned(),
    };
    store.save(Bindings::default()).unwrap();
    let external = std::fs::read(&path).unwrap();
    assert!(
        session
            .change_keyboard_shortcuts(
                Actor::User,
                &session.shortcuts_stamp(),
                "save",
                Some(custom())
            )
            .is_err()
    );
    assert_eq!(session.shortcut_bindings(), &custom());
    assert_eq!(std::fs::read(&path).unwrap(), external);
    assert_eq!(session.keyboard_shortcuts_state()["ready"], false);
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), &Bindings::default());
}

#[test]
fn configured_and_explicit_object_commands_share_transactions_selection_and_exact_history() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session) = fixture();
        let id = session
            .create_block(
                actor,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "test/material",
                false,
            )
            .unwrap();
        let before = session.document.vmf().clone();
        let mut explicit = Session::new(Err("no assets needed".into()));
        explicit.replace_document(map_core::Document::new(before.clone()), None);
        let history = session.document.undo_labels().count();
        let result = session
            .execute_editor_command(actor, Command::DuplicateSelection, 1.0)
            .unwrap();
        assert_eq!(result["copies"].as_array().unwrap().len(), 1);
        let after = session.document.vmf().clone();
        let copies = session.selected_ids();
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session
            .execute_editor_command(actor, Command::Undo, 1.0)
            .unwrap();
        assert_eq!(session.document.vmf(), &before);
        session
            .execute_editor_command(actor, Command::Redo, 1.0)
            .unwrap();
        assert_eq!(session.document.vmf(), &after);
        session.undo(actor).unwrap();
        let grid = explicit.spatial.grid_size;
        explicit
            .duplicate_objects(actor, &[id], DVec3::new(grid, grid, 0.0))
            .unwrap();
        assert_eq!(explicit.document.vmf(), &after);
        session.redo(actor).unwrap();
        session.select(&copies, SelectMode::Replace).unwrap();
        let selected = session.selected_ids();
        session
            .execute_editor_command(actor, Command::DeleteSelection, 1.0)
            .unwrap();
        assert!(
            selected
                .into_iter()
                .all(|id| !session.document.contains(id))
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &after);
        let history = session.document.undo_labels().count();
        session
            .execute_editor_command(actor, Command::FrameSelection, 1.5)
            .unwrap();
        assert_eq!(session.document.vmf(), &after);
        assert_eq!(session.document.undo_labels().count(), history);
        assert!(
            session
                .execute_editor_command(actor, Command::FrameSelection, f32::NAN)
                .is_err()
        );
    }
}

#[test]
fn object_shortcut_commands_reject_component_scope_without_discarding_its_state() {
    let (_directory, mut session) = fixture();
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    session
        .set_component_picking(
            Some(super::super::component_picking::Kind::Vertex),
            &[solid],
            &session.component_stamp(),
        )
        .unwrap();
    let before = session.document.vmf().clone();
    let stamp = session.component_stamp();
    for actor in [Actor::User, Actor::Agent] {
        for command in [Command::DeleteSelection, Command::DuplicateSelection] {
            assert!(session.execute_editor_command(actor, command, 1.0).is_err());
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.component_stamp(), stamp);
            assert!(session.component_picking().is_some());
        }
    }
}
