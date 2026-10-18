use super::*;
use crate::shortcuts::Store;

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
        assert_eq!(restarted.shortcut_bindings(), custom());
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
        assert_eq!(session.shortcut_bindings(), custom());
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
    assert_eq!(restarted.shortcut_bindings(), Bindings::default());
    assert_eq!(restarted.keyboard_shortcuts_state()["ready"], false);
    assert_eq!(
        std::fs::read(&path).unwrap().len(),
        crate::shortcuts::MAX_FILE_BYTES as usize + 1
    );
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reset", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), Bindings::default());
    assert_eq!(session.keyboard_shortcuts_state()["ready"], true);
    save(&mut session, Actor::Agent, custom());
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), custom());
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
    assert_eq!(session.shortcut_bindings(), custom());
    assert_eq!(std::fs::read(&path).unwrap(), external);
    assert_eq!(session.keyboard_shortcuts_state()["ready"], false);
    session
        .change_keyboard_shortcuts(Actor::User, &session.shortcuts_stamp(), "reload", None)
        .unwrap();
    assert_eq!(session.shortcut_bindings(), Bindings::default());
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

#[test]
fn viewport_toggle_commands_return_actual_booleans_without_authoring_or_preference_writes() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session) = fixture();
        session
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        session
            .set_spatial(
                Actor::User,
                crate::spatial::Update {
                    grid_size: Some(8.0),
                    elevation: Some(32.0),
                    depth: Some(64.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let selected = session.selected_ids();
        let camera = session.camera();
        let spatial = session.spatial();
        let view = session.view();
        let bindings = session.shortcut_bindings();
        for (command, field) in [
            (Command::ShowGrid, "grid_visible"),
            (Command::SnapToGrid, "snap"),
            (Command::ToolTextures, "tool_textures"),
            (Command::SelectionOutlines, "selection_outlines"),
        ] {
            for enabled in [false, true] {
                let result = session
                    .execute_editor_command(actor, command, f32::NAN)
                    .unwrap();
                assert_eq!(result["command"], serde_json::to_value(command).unwrap());
                assert_eq!(result[field], enabled);
                let actual = match command {
                    Command::ShowGrid => session.spatial().grid_visible,
                    Command::SnapToGrid => session.spatial().snap,
                    Command::ToolTextures => session.view().tool_textures,
                    Command::SelectionOutlines => session.view().selection_outlines,
                    _ => unreachable!(),
                };
                assert_eq!(actual, enabled);
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.history_snapshot(), history);
                assert_eq!(session.selected_ids(), selected);
                assert_eq!(session.camera(), camera);
            }
        }
        assert_eq!(session.spatial(), spatial);
        assert_eq!(session.view(), view);
        assert_eq!(session.shortcut_bindings(), bindings);
        assert!(!directory.path().join(crate::shortcuts::FILE_NAME).exists());
        assert!(
            !directory
                .path()
                .join(crate::grid_preferences::FILE_NAME)
                .exists()
        );
    }
}

#[test]
fn viewport_toggle_commands_keep_the_existing_setting_draft_cleanup() {
    let (_directory, mut session) = fixture();
    let ObjectId::Solid(solid) = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap()
    else {
        panic!()
    };
    session.set_camera(render::Camera::planar(
        Projection::Top,
        glam::Vec3::splat(32.0),
        512.0,
    ));
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let start = super::super::Pointer {
        position: glam::DVec2::new(200.0, 100.0),
        size: (1200, 800),
    };
    session
        .preview_area_selection(
            Actor::User,
            session.camera(),
            start,
            glam::DVec2::new(1000.0, 700.0),
            map_core::region::Match::Intersecting,
            SelectMode::Replace,
        )
        .unwrap();
    let epoch = session.interaction_epoch();
    session
        .execute_editor_command(Actor::Agent, Command::SelectionOutlines, 1.0)
        .unwrap();
    assert!(session.area_preview().is_some());
    assert_eq!(session.interaction_epoch(), epoch);
    session
        .execute_editor_command(Actor::User, Command::ToolTextures, 1.0)
        .unwrap();
    assert!(session.area_preview().is_none());
    assert!(session.interaction_epoch() > epoch);
    let face = map_core::surface::FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session
        .edit_faces(
            Actor::User,
            &[face],
            map_core::surface::SurfaceEdit {
                shift_u: Some(11.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let face_token = session.face_preview_token();
    assert!(face_token.is_some());
    for command in [Command::ShowGrid, Command::SnapToGrid] {
        session
            .preview_block(DVec3::splat(128.0), DVec3::splat(160.0))
            .unwrap();
        assert!(session.block_preview().is_some());
        session
            .execute_editor_command(Actor::Agent, command, 1.0)
            .unwrap();
        assert!(session.block_preview().is_none());
        assert_eq!(session.face_preview_token(), face_token);
    }
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn transform_tool_commands_preserve_options_and_same_tool_preview() {
    use render::handles::Mode;

    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session) = fixture();
        let id = session
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        let settings = super::super::TransformTool {
            local: true,
            pivot: Some(DVec3::new(8.0, 16.0, 24.0)),
            texture_lock: false,
            texture_scale_lock: Some(false),
            angle_step: 7.5,
            ..Default::default()
        };
        session.set_transform_tool(actor, settings).unwrap();
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let bindings = session.shortcut_bindings();
        for (command, mode, name) in [
            (Command::MoveTool, Some(Mode::Move), "move"),
            (Command::RotateTool, Some(Mode::Rotate), "rotate"),
            (Command::ScaleTool, Some(Mode::Scale), "scale"),
            (Command::SelectTool, None, "select"),
        ] {
            let result = session
                .execute_editor_command(actor, command, f32::NAN)
                .unwrap();
            assert_eq!(result["tool"], name);
            assert_eq!(
                session.transform_tool(),
                super::super::TransformTool { mode, ..settings }
            );
            assert_eq!(session.selected_ids(), vec![id]);
            assert!(!session.face_mode());
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.shortcut_bindings(), bindings);
        }
        session
            .execute_editor_command(actor, Command::MoveTool, 1.0)
            .unwrap();
        session
            .preview_transform(
                actor,
                &[id],
                map_core::edit::Transform {
                    translation: DVec3::X * 16.0,
                    ..Default::default()
                },
                settings.pivot,
            )
            .unwrap();
        let preview = session.render_document().vmf().clone();
        let epoch = session.interaction_epoch();
        session
            .execute_editor_command(actor, Command::MoveTool, 1.0)
            .unwrap();
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.interaction_epoch(), epoch);
        session
            .execute_editor_command(actor, Command::SelectTool, 1.0)
            .unwrap();
        assert!(session.transform_preview().is_none());
        session.set_face_mode(true);
        session
            .execute_editor_command(actor, Command::RotateTool, 1.0)
            .unwrap();
        assert!(session.face_mode());
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
    }
}

#[test]
fn selection_mode_commands_keep_same_mode_drafts_and_retire_changed_mode_gestures() {
    let (_directory, mut session) = fixture();
    let ObjectId::Solid(solid) = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap()
    else {
        panic!()
    };
    session.set_camera(render::Camera::planar(
        Projection::Top,
        glam::Vec3::splat(32.0),
        512.0,
    ));
    let face = map_core::surface::FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    session
        .edit_faces(
            Actor::User,
            &[face],
            map_core::surface::SurfaceEdit {
                shift_u: Some(13.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let token = session.face_preview_token();
    let result = session
        .execute_editor_command(Actor::Agent, Command::Objects, f32::NAN)
        .unwrap();
    assert_eq!(result["mode"], "objects");
    assert_eq!(result["mode_changed"], false);
    assert_eq!(session.face_preview_token(), token);
    assert_eq!(session.selected_ids(), vec![ObjectId::Solid(solid)]);
    session.cancel_face_preview();
    session
        .set_component_picking(
            Some(super::super::component_picking::Kind::Vertex),
            &[solid],
            &session.component_stamp(),
        )
        .unwrap();
    session
        .execute_editor_command(Actor::User, Command::Objects, 1.0)
        .unwrap();
    assert!(session.component_picking().is_none());
    session.set_clip_tool(Actor::User, true).unwrap();
    let result = session
        .execute_editor_command(Actor::User, Command::Faces, 1.0)
        .unwrap();
    assert_eq!(result["mode"], "faces");
    assert_eq!(result["mode_changed"], true);
    assert!(!session.clip_tool());
    assert!(session.selected_ids().is_empty());
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    session
        .edit_faces(
            Actor::User,
            &[face],
            map_core::surface::SurfaceEdit {
                shift_v: Some(7.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let token = session.face_preview_token();
    let result = session
        .execute_editor_command(Actor::Agent, Command::Faces, 1.0)
        .unwrap();
    assert_eq!(result["mode_changed"], false);
    assert_eq!(session.selected_faces(), vec![face]);
    assert_eq!(session.face_preview_token(), token);
    let pointer = super::super::Pointer {
        position: glam::DVec2::new(400.0, 250.0),
        size: (1200, 800),
    };
    let end = super::super::Pointer {
        position: glam::DVec2::new(800.0, 550.0),
        ..pointer
    };
    let mut interaction = crate::interaction::Interaction::default();
    interaction.down(&mut session, pointer, false).unwrap();
    interaction.moved(&mut session, end).unwrap();
    assert!(session.area_preview().is_some());
    session
        .execute_editor_command(Actor::User, Command::Objects, 1.0)
        .unwrap();
    assert!(session.face_preview_token().is_none());
    assert!(session.area_preview().is_none());
    assert!(interaction.up(&mut session, end).unwrap());
    assert!(session.selected_faces().is_empty());
    assert!(session.selected_ids().is_empty());
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                draw_block: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    session
        .execute_editor_command(Actor::User, Command::Faces, 1.0)
        .unwrap();
    assert!(!session.spatial().draw_block);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}
