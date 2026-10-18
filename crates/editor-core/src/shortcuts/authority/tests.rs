use super::*;
use crate::session::{Actor, Session};
use glam::DVec3;

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

#[test]
fn attached_sessions_share_effective_bindings_errors_and_recovery_without_rebinding() {
    let directory = tempfile::tempdir().unwrap();
    let saved = directory.path().join("shared");
    let authority = Authority::configured(Ok(saved.clone()));
    let mut first = Session::with_keyboard_shortcuts(Err("no assets".into()), authority.clone());
    let mut second = Session::with_keyboard_shortcuts(Err("no assets".into()), authority);
    first.configure_recovery(Ok(directory.path().join("first")));
    second.configure_recovery(Ok(directory.path().join("second")));
    let stamp = second.shortcuts_stamp();
    second.configure_recovery(Err("different recovery settings".into()));
    assert_eq!(second.shortcuts_stamp(), stamp);
    assert_eq!(
        second.keyboard_shortcuts_snapshot().path,
        Some(saved.join(FILE_NAME))
    );
    first
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(32.), "first", false)
        .unwrap();
    for session in [&mut first, &mut second] {
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
    }
    let first_source = first.document().vmf().clone();
    let second_source = second.document().vmf().clone();
    let history = (first.history_snapshot(), second.history_snapshot());
    let cameras = (first.camera(), second.camera());
    let tokens = (
        first.property_draft().unwrap().token,
        second.property_draft().unwrap().token,
    );
    let serials = (
        first.scene_serial(),
        first.view_serial(),
        first.camera_serial(),
        second.scene_serial(),
        second.view_serial(),
        second.camera_serial(),
    );
    let old = second.keyboard_shortcuts_snapshot();
    let result = first
        .change_keyboard_shortcuts(
            Actor::User,
            &first.shortcuts_stamp(),
            "save",
            Some(custom()),
        )
        .unwrap();
    assert_eq!(result["current"], json!(custom()));
    assert_eq!(result["snapshot"], second.shortcuts_stamp());
    assert_eq!(old.current, Bindings::default());
    assert_eq!(second.shortcut_bindings(), custom());
    assert_eq!(
        second.resolve_editor_shortcut("F6", true, false, false, false),
        Some(Command::Undo)
    );
    assert!(
        second
            .change_keyboard_shortcuts(Actor::Agent, &old.stamp(), "reset", None)
            .is_err()
    );
    let path = saved.join(FILE_NAME);
    std::fs::write(&path, b"broken").unwrap();
    assert!(
        second
            .change_keyboard_shortcuts(Actor::Agent, &second.shortcuts_stamp(), "reload", None)
            .is_err()
    );
    assert_eq!(first.shortcut_bindings(), custom());
    assert_eq!(
        first.resolve_editor_shortcut("F6", true, false, false, false),
        Some(Command::Undo)
    );
    assert_eq!(
        first.keyboard_shortcuts_state(),
        second.keyboard_shortcuts_state()
    );
    assert_eq!(first.keyboard_shortcuts_state()["ready"], false);
    first
        .change_keyboard_shortcuts(Actor::User, &first.shortcuts_stamp(), "reset", None)
        .unwrap();
    assert_eq!(second.shortcut_bindings(), Bindings::default());
    Store {
        directory: saved.clone(),
    }
    .save(custom())
    .unwrap();
    second
        .change_keyboard_shortcuts(Actor::Agent, &second.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(first.shortcut_bindings(), custom());
    assert!(first.shortcuts_error_message().is_none());
    assert_eq!(first.document().vmf(), &first_source);
    assert_eq!(second.document().vmf(), &second_source);
    assert_eq!(
        (first.history_snapshot(), second.history_snapshot()),
        history
    );
    assert_eq!((first.camera(), second.camera()), cameras);
    assert_eq!(
        (
            first.property_draft().unwrap().token,
            second.property_draft().unwrap().token
        ),
        tokens
    );
    assert_eq!(
        (
            first.scene_serial(),
            first.view_serial(),
            first.camera_serial(),
            second.scene_serial(),
            second.view_serial(),
            second.camera_serial()
        ),
        serials
    );
    let mut independent = Session::new(Err("no assets".into()));
    independent.configure_recovery(Ok(directory.path().join("independent")));
    assert_eq!(independent.shortcut_bindings(), Bindings::default());
    assert!(!directory.path().join("first").join(FILE_NAME).exists());
    assert!(!directory.path().join("second").join(FILE_NAME).exists());
}

#[test]
fn all_commands_mutate_only_the_originating_document() {
    for command in Command::ALL {
        let authority = Authority::default();
        let mut first =
            Session::with_keyboard_shortcuts(Err("no assets".into()), authority.clone());
        let mut second = Session::with_keyboard_shortcuts(Err("no assets".into()), authority);
        first
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(32.), "first", true)
            .unwrap();
        second
            .create_block(
                Actor::Agent,
                DVec3::splat(1000.),
                DVec3::splat(1064.),
                "second",
                true,
            )
            .unwrap();
        if command == Command::Redo {
            second.undo(Actor::Agent).unwrap();
        }
        let first_source = first.document().vmf().clone();
        let first_history = first.history_snapshot();
        let first_camera = first.camera();
        let first_selection = first.selected_ids();
        let stamp = first.shortcuts_stamp();
        second
            .execute_editor_command(Actor::Agent, command, 1.)
            .unwrap();
        assert_eq!(first.document().vmf(), &first_source, "{command:?}");
        assert_eq!(first.history_snapshot(), first_history, "{command:?}");
        assert_eq!(first.camera(), first_camera, "{command:?}");
        assert_eq!(first.selected_ids(), first_selection, "{command:?}");
        assert_eq!(first.shortcuts_stamp(), stamp);
    }
}

#[cfg(unix)]
#[test]
fn committed_verification_failure_keeps_intended_values_and_original_directory_until_reload() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original");
    let other = directory.path().join("other");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&other).unwrap();
    let requested = directory.path().join("requested");
    symlink(&original, &requested).unwrap();
    let authority = Authority::configured(Ok(requested.clone()));
    let mut session =
        Session::with_keyboard_shortcuts(Err("no assets needed".into()), authority.clone());
    let current = session.keyboard_shortcuts_state()["current"].clone();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let expected = authority.lock().source.as_ref().unwrap().clone();
    let intended = custom();
    let data = serde_json::to_vec(&json!({"version":1,"bindings":intended})).unwrap();
    let target = crate::preference_file::CoordinatedFile {
        directory: &requested,
        file_name: crate::shortcuts::FILE_NAME,
        lock_name: ".shortcuts.lock",
        label: "Keyboard shortcuts",
        reload: "Reload saved",
        saved_description: "saved bindings",
        max_bytes: crate::shortcuts::MAX_FILE_BYTES,
    };
    let publication = target
        .publish_with(&data, &expected, |path, data| {
            crate::recovery::atomic_write(path, data)?;
            std::fs::remove_file(&requested).map_err(|error| error.to_string())?;
            symlink(&other, &requested).map_err(|error| error.to_string())
        })
        .unwrap();
    assert!(
        publication
            .source
            .as_ref()
            .unwrap_err()
            .contains("were written")
    );
    authority
        .lock()
        .accept_publication(intended.clone(), publication);
    let state = session.keyboard_shortcuts_state();
    assert_eq!(state["ready"], false);
    assert!(state["error"].as_str().unwrap().contains("were written"));
    assert_eq!(state["current"], json!(intended));
    assert_ne!(state["current"], current);
    assert_eq!(
        std::fs::read(original.join(target.file_name)).unwrap(),
        data
    );
    for _ in 0..2 {
        assert!(
            session
                .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reset", None)
                .unwrap_err()
                .contains("directory")
        );
    }
    assert!(!other.join(target.file_name).exists());
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(
        session
            .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reset", None)
            .unwrap()["write_committed"],
        true
    );
    assert!(other.join(target.file_name).is_file());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), 0);
    assert_eq!(session.camera(), camera);
}
