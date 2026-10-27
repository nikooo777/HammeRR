use super::*;
use glam::DVec3;
use render::{SurfaceView, ViewOptions, sky::Channel};

fn action(session: &mut Session, actor: Actor, action: &str) -> Result<Value, String> {
    session.change_preview_preferences(actor, action, &session.preview_preferences_stamp())
}

fn custom() -> Preview {
    Preview {
        selection_outlines: false,
        selection_tint: false,
        entity_images: false,
        entity_model_guides: false,
        rope_preview: true,
        sky_preview: true,
    }
}

fn set(session: &mut Session, preview: Preview) {
    let view = session.resolve_view_update(preview.update()).unwrap();
    session.set_view(view);
}

#[test]
fn explicit_save_survives_restart_and_apply_preserves_unrelated_view_and_map_state() {
    for actor in [Actor::User, Actor::Agent] {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().into()));
        session
            .create_block(actor, DVec3::ZERO, DVec3::splat(32.0), "brick/test", false)
            .unwrap();
        let source = session.document.vmf().clone();
        let history: Vec<_> = session.document.undo_labels().map(str::to_string).collect();
        let selection = session.selected_ids();
        let camera = session.camera;
        let path = directory.path().join(crate::preview_preferences::FILE_NAME);
        set(&mut session, custom());
        assert!(!path.exists());
        let serial = session.view_serial;
        assert_eq!(
            action(&mut session, actor, "save").unwrap()["write_committed"],
            true
        );
        assert_eq!(session.view_serial, serial);
        let bytes = std::fs::read(&path).unwrap();
        set(&mut session, Preview::default());
        let current = session.view();
        session.set_view(ViewOptions {
            surface: SurfaceView::Wireframe,
            tool_textures: false,
            leak_path: false,
            sky_channel: Channel::Hdr,
            sky_exposure_ev: 2.25,
            ..current
        });
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        action(&mut session, actor, "apply").unwrap();
        assert_eq!(Preview::from(session.view()), custom());
        assert_eq!(session.view.surface, SurfaceView::Wireframe);
        assert!(!session.view.tool_textures);
        assert!(!session.view.leak_path);
        assert_eq!(session.view.sky_channel, Channel::Hdr);
        assert_eq!(session.view.sky_exposure_ev, 2.25);
        assert_eq!(session.camera, camera);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recovery(Ok(directory.path().into()));
        assert_eq!(Preview::from(reopened.view()), custom());
        assert_eq!(reopened.view.surface, ViewOptions::default().surface);
        assert_eq!(reopened.view.sky_channel, Channel::Ldr);
        assert_eq!(reopened.view.sky_exposure_ev, 0.0);
        assert_eq!(reopened.preview_preferences_state()["stored"], true);
        assert_eq!(reopened.document.undo_labels().count(), 0);
        assert!(!reopened.document.is_dirty());
    }
}

#[test]
fn malformed_startup_retry_and_reset_never_change_current_settings_until_apply() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(crate::preview_preferences::FILE_NAME);
    std::fs::write(&path, b"unsupported content").unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    set(&mut session, custom());
    let current = session.view();
    let serial = session.view_serial;
    session.configure_recovery(Ok(directory.path().into()));
    assert_eq!(Preview::from(session.view()), custom());
    assert_eq!(session.preview_preferences_state()["ready"], false);
    assert!(session.preview_preferences_error().is_some());
    for action_name in ["save", "apply", "reload"] {
        assert!(action(&mut session, Actor::Agent, action_name).is_err());
        assert_eq!(Preview::from(session.view()), custom());
        assert_eq!(std::fs::read(&path).unwrap(), b"unsupported content");
    }
    let preferences_serial = session.view_serial;
    action(&mut session, Actor::User, "reset").unwrap();
    assert_eq!(session.view_serial, preferences_serial);
    assert_eq!(Preview::from(session.view()), custom());
    action(&mut session, Actor::User, "apply").unwrap();
    assert_eq!(Preview::from(session.view()), Preview::default());
    Store {
        directory: directory.path().into(),
    }
    .save(custom(), false)
    .unwrap();
    let serial_after_apply = session.view_serial;
    action(&mut session, Actor::User, "reload").unwrap();
    assert_eq!(session.view_serial, serial_after_apply);
    assert_eq!(Preview::from(session.view()), Preview::default());
    action(&mut session, Actor::User, "apply").unwrap();
    assert_eq!(Preview::from(session.view()), custom());
    assert!(session.view_serial > serial);
    assert_eq!(session.view().surface, current.surface);
    assert_eq!(session.document.undo_labels().count(), 0);
}

#[test]
fn snapshots_guard_all_preview_settings_without_capturing_unrelated_settings() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    for preview in [
        Preview {
            selection_tint: false,
            ..Preview::default()
        },
        Preview {
            selection_outlines: false,
            ..Preview::default()
        },
        Preview {
            entity_images: false,
            ..Preview::default()
        },
        Preview {
            entity_model_guides: false,
            ..Preview::default()
        },
        Preview {
            rope_preview: true,
            ..Preview::default()
        },
        Preview {
            sky_preview: true,
            ..Preview::default()
        },
    ] {
        set(&mut session, Preview::default());
        let snapshot = session.preview_preferences_stamp();
        set(&mut session, preview);
        for action in ["save", "apply", "reload", "reset"] {
            assert!(
                session
                    .change_preview_preferences(Actor::Agent, action, &snapshot)
                    .is_err()
            );
        }
    }
    let snapshot = session.preview_preferences_stamp();
    let view = session.view();
    session.set_view(ViewOptions {
        tool_textures: false,
        ..view
    });
    session
        .change_preview_preferences(Actor::User, "save", &snapshot)
        .unwrap();
    assert!(!session.view.tool_textures);
}

#[test]
fn two_sessions_guard_outside_replacement_and_deletion_before_save_apply_and_reset() {
    for action_name in ["save", "apply", "reset"] {
        for delete in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut first = Session::new(Err("no assets needed".into()));
            first.configure_recovery(Ok(directory.path().into()));
            action(&mut first, Actor::User, "save").unwrap();
            let mut second = Session::new(Err("no assets needed".into()));
            second.configure_recovery(Ok(directory.path().into()));
            set(&mut second, custom());
            action(&mut second, Actor::Agent, "save").unwrap();
            let path = directory.path().join(crate::preview_preferences::FILE_NAME);
            if delete {
                std::fs::remove_file(&path).unwrap();
            }
            let bytes = std::fs::read(&path).ok();
            let view = first.view();
            assert!(
                action(&mut first, Actor::Agent, action_name)
                    .unwrap_err()
                    .contains("outside this session")
            );
            assert_eq!(first.view(), view);
            assert_eq!(std::fs::read(&path).ok(), bytes);
            assert_eq!(first.preview_preferences_state()["ready"], false);
            action(&mut first, Actor::User, "reload").unwrap();
            assert_eq!(first.view(), view);
            action(&mut first, Actor::User, "apply").unwrap();
            assert_eq!(
                Preview::from(first.view()),
                if delete { Preview::default() } else { custom() }
            );
        }
    }
}

#[test]
fn active_geometry_and_armed_entity_placement_reject_apply_without_changing_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let preview = session.block_preview;
    let view = session.view();
    assert!(action(&mut session, Actor::Agent, "apply").is_err());
    assert_eq!(session.block_preview, preview);
    assert_eq!(session.view(), view);
    session.cancel_block_preview();
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse("@PointClass = light []").unwrap(),
    );
    let stamp = session.entity_placement_stamp();
    let draft = session
        .begin_entity_placement(
            Actor::Agent,
            super::super::entity_placement::Request {
                classname: "light".into(),
                properties: Default::default(),
                offset: 0.0,
                align: false,
                snap: false,
            },
            &stamp,
        )
        .unwrap();
    let view = session.view();
    assert!(action(&mut session, Actor::User, "apply").is_err());
    assert_eq!(session.get_entity_placement()["draft"], draft["draft"]);
    assert_eq!(session.view(), view);
    assert_eq!(session.document.undo_labels().count(), 0);
}

#[test]
fn save_reload_reset_and_storage_errors_preserve_active_measurement_picking() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    let state = session
        .preview_measurement(
            Actor::Agent,
            super::super::measurements::Request {
                id: None,
                name: "Clearance".into(),
                start: DVec3::ZERO,
                end: DVec3::X,
            },
            &session.measurement_stamp(),
        )
        .unwrap();
    let token = state["draft"]["token"].as_u64().unwrap();
    session
        .begin_measurement_pick(
            Actor::Agent,
            token,
            super::super::measurements::Endpoint::End,
            session.camera(),
            (128, 128),
            &session.measurement_pick_stamp(),
        )
        .unwrap();
    let draft = session.measurement_state()["draft"].clone();
    assert_eq!(draft["picking"]["valid"], true);
    let serial = session.view_serial;
    for name in ["save", "reload", "reset"] {
        action(&mut session, Actor::User, name).unwrap();
        assert_eq!(session.measurement_state()["draft"], draft);
        assert_eq!(session.view_serial, serial);
    }
    assert!(action(&mut session, Actor::User, "apply").is_err());
    assert_eq!(session.measurement_state()["draft"], draft);
    let path = directory.path().join(crate::preview_preferences::FILE_NAME);
    std::fs::write(path, b"broken outside edit").unwrap();
    assert!(action(&mut session, Actor::User, "save").is_err());
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(session.view_serial, serial);
    assert_eq!(session.document.undo_labels().count(), 0);
}

#[cfg(unix)]
#[test]
fn failed_atomic_write_preserves_existing_bytes_view_and_history() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    action(&mut session, Actor::User, "save").unwrap();
    set(&mut session, custom());
    let path = directory.path().join(crate::preview_preferences::FILE_NAME);
    let bytes = std::fs::read(&path).unwrap();
    let view = session.view();
    let serial = session.view_serial;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let error = action(&mut session, Actor::Agent, "save").unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(session.view(), view);
    assert_eq!(session.view_serial, serial);
    assert_eq!(session.preview_preferences_state()["ready"], false);
    assert_eq!(
        session.preview_preferences_state()["saved"],
        json!(Preview::default())
    );
    assert_eq!(session.document.undo_labels().count(), 0);
}
