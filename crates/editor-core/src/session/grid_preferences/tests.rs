use super::*;
use crate::spatial::Update;
use glam::DVec3;

fn action(session: &mut Session, actor: Actor, action: &str) -> Result<Value, String> {
    session.change_grid_preferences(actor, action, &session.grid_preferences_stamp())
}

#[test]
fn explicit_save_survives_restart_while_temporary_changes_and_workplanes_do_not() {
    for actor in [Actor::User, Actor::Agent] {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().into()));
        let source = session.document.vmf().clone();
        let saved = Grid {
            grid_size: 17.375,
            snap: false,
            grid_visible: false,
        };
        session
            .set_spatial(
                actor,
                Update {
                    elevation: Some(67.25),
                    depth: Some(13.5),
                    draw_block: Some(true),
                    ..saved.update()
                },
            )
            .unwrap();
        assert!(
            !directory
                .path()
                .join(crate::grid_preferences::FILE_NAME)
                .exists()
        );
        action(&mut session, actor, "save").unwrap();
        let bytes =
            std::fs::read(directory.path().join(crate::grid_preferences::FILE_NAME)).unwrap();
        session
            .set_spatial(actor, Grid::default().update())
            .unwrap();
        assert_eq!(
            std::fs::read(directory.path().join(crate::grid_preferences::FILE_NAME)).unwrap(),
            bytes
        );
        action(&mut session, actor, "apply").unwrap();
        assert_eq!(Grid::from(session.spatial), saved);
        assert_eq!(
            (
                session.spatial.elevation,
                session.spatial.depth,
                session.spatial.draw_block
            ),
            (67.25, 13.5, true)
        );
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), 0);
        assert!(!session.document.is_dirty());
        let mut reopened = Session::new(Err("no assets needed".into()));
        reopened.configure_recovery(Ok(directory.path().into()));
        assert_eq!(Grid::from(reopened.spatial), saved);
        assert_eq!(
            (
                reopened.spatial.elevation,
                reopened.spatial.depth,
                reopened.spatial.draw_block
            ),
            (0.0, 128.0, false)
        );
        assert_eq!(reopened.grid_preferences_state()["stored"], true);
    }
}

#[test]
fn startup_errors_preserve_bad_content_and_current_settings_until_retry_or_reset() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(crate::grid_preferences::FILE_NAME);
    std::fs::write(&path, b"unsupported content").unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .set_spatial(
            Actor::User,
            Update {
                grid_size: Some(42.5),
                ..Default::default()
            },
        )
        .unwrap();
    session.configure_recovery(Ok(directory.path().into()));
    let current = session.spatial;
    assert_eq!(session.grid_preferences_state()["ready"], false);
    assert!(session.grid_preferences_state()["error"].is_string());
    for action_name in ["save", "apply", "reload"] {
        assert!(action(&mut session, Actor::Agent, action_name).is_err());
        assert_eq!(session.spatial, current);
        assert_eq!(std::fs::read(&path).unwrap(), b"unsupported content");
    }
    action(&mut session, Actor::User, "reset").unwrap();
    assert_eq!(session.spatial, current);
    assert_eq!(
        session.grid_preferences_state()["defaults"],
        json!(Grid::default())
    );
    action(&mut session, Actor::User, "apply").unwrap();
    assert_eq!(Grid::from(session.spatial), Grid::default());
    let custom = Grid {
        grid_size: 0.125,
        snap: false,
        grid_visible: true,
    };
    Store {
        directory: directory.path().into(),
    }
    .save(custom, false)
    .unwrap();
    action(&mut session, Actor::User, "reload").unwrap();
    assert_eq!(Grid::from(session.spatial), Grid::default());
    action(&mut session, Actor::User, "apply").unwrap();
    assert_eq!(Grid::from(session.spatial), custom);
}

#[test]
fn stale_actions_and_write_failure_preserve_map_history_and_current_grid() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let history: Vec<_> = session.document.undo_labels().map(str::to_string).collect();
    let snapshot = session.grid_preferences_stamp();
    session
        .set_spatial(
            Actor::User,
            Update {
                snap: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    for action in ["save", "apply", "reload", "reset"] {
        assert!(
            session
                .change_grid_preferences(Actor::Agent, action, &snapshot)
                .is_err()
        );
    }
    let current = session.spatial;
    let path = directory.path().join(crate::grid_preferences::FILE_NAME);
    std::fs::create_dir(&path).unwrap();
    assert!(action(&mut session, Actor::Agent, "save").is_err());
    assert_eq!(session.grid_preferences_state()["ready"], false);
    assert!(session.grid_preferences_state()["error"].is_string());
    assert_eq!(session.spatial, current);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
    assert!(path.is_dir());
}

#[test]
fn applying_defaults_rejects_active_geometry_without_cancelling_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let preview = session.block_preview;
    assert!(preview.is_some());
    let source = session.document.vmf().clone();
    assert!(action(&mut session, Actor::User, "apply").is_err());
    assert_eq!(session.block_preview, preview);
    assert_eq!(session.document.vmf(), &source);
    action(&mut session, Actor::User, "save").unwrap();
    assert_eq!(session.block_preview, preview);
}

#[test]
fn two_sessions_reject_external_replacement_for_save_apply_and_reset_then_allow_reload() {
    for action_name in ["save", "apply", "reset"] {
        let directory = tempfile::tempdir().unwrap();
        let mut first = Session::new(Err("no assets needed".into()));
        let mut second = Session::new(Err("no assets needed".into()));
        first.configure_recovery(Ok(directory.path().into()));
        second.configure_recovery(Ok(directory.path().into()));
        second
            .set_spatial(
                Actor::User,
                Update {
                    grid_size: Some(31.5),
                    ..Default::default()
                },
            )
            .unwrap();
        action(&mut second, Actor::User, "save").unwrap();
        let path = directory.path().join(crate::grid_preferences::FILE_NAME);
        let bytes = std::fs::read(&path).unwrap();
        let current = first.spatial;
        let source = first.document.vmf().clone();
        assert!(
            action(&mut first, Actor::Agent, action_name)
                .unwrap_err()
                .contains("outside this session")
        );
        assert_eq!(first.spatial, current);
        assert_eq!(first.document.vmf(), &source);
        assert_eq!(first.document.undo_labels().count(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(first.grid_preferences_state()["ready"], false);
        action(&mut first, Actor::Agent, "reload").unwrap();
        assert_eq!(
            first.grid_preferences_state()["defaults"]["grid_size"],
            31.5
        );
        action(&mut first, Actor::Agent, "apply").unwrap();
        assert_eq!(first.spatial.grid_size, 31.5);
    }
}

#[test]
fn corruption_after_inspection_rejects_all_stale_actions_before_explicit_reset() {
    for action_name in ["save", "apply", "reset"] {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().into()));
        action(&mut session, Actor::User, "save").unwrap();
        let path = directory.path().join(crate::grid_preferences::FILE_NAME);
        std::fs::write(&path, b"corrupt after inspection").unwrap();
        let current = session.spatial;
        assert!(action(&mut session, Actor::Agent, action_name).is_err());
        assert_eq!(session.spatial, current);
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt after inspection");
        assert_eq!(session.grid_preferences_state()["ready"], false);
        action(&mut session, Actor::User, "reset").unwrap();
        assert_eq!(
            Store {
                directory: directory.path().into()
            }
            .load()
            .unwrap(),
            Some(Grid::default())
        );
    }
}

#[cfg(unix)]
#[test]
fn atomic_write_failure_preserves_existing_bytes_current_grid_and_history() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    action(&mut session, Actor::User, "save").unwrap();
    session
        .set_spatial(
            Actor::Agent,
            Update {
                grid_size: Some(20.5),
                ..Default::default()
            },
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let current = session.spatial;
    let path = directory.path().join(crate::grid_preferences::FILE_NAME);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let error = action(&mut session, Actor::Agent, "save").unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(session.spatial, current);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), 0);
    assert_eq!(session.grid_preferences_state()["ready"], false);
    assert_eq!(
        session.grid_preferences_state()["saved"],
        json!(Grid::default())
    );
}

#[test]
fn apply_preserves_another_actors_armed_entity_placement_without_a_picked_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
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
    let current = session.spatial;
    assert!(action(&mut session, Actor::User, "apply").is_err());
    assert_eq!(session.get_entity_placement()["draft"], draft["draft"]);
    assert_eq!(session.spatial, current);
    assert_eq!(session.document.undo_labels().count(), 0);
}
