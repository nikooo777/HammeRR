use super::*;
use glam::DVec3;

fn action(session: &mut Session, name: &str) -> Result<Value, String> {
    session.change_navigation_preferences(
        Actor::User,
        name,
        &session.navigation_preferences_stamp(),
    )
}

#[test]
fn temporary_speed_and_wheel_changes_share_bounds_without_auto_saving() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    assert_eq!(session.fly_speed(), 600.0);
    let camera = session.camera;
    let serials = (
        session.camera_serial(),
        session.scene_serial(),
        session.view_serial,
        session.interaction_epoch,
    );
    let path = directory
        .path()
        .join(crate::navigation_preferences::FILE_NAME);
    session.adjust_fly_speed(240.0).unwrap();
    assert_eq!(session.fly_speed(), 1200.0);
    session.adjust_fly_speed(-480.0).unwrap();
    assert_eq!(session.fly_speed(), 300.0);
    session.adjust_fly_speed(f32::MAX).unwrap();
    assert_eq!(session.fly_speed(), 20_000.0);
    session.adjust_fly_speed(-f32::MAX).unwrap();
    assert_eq!(session.fly_speed(), 25.0);
    session.set_fly_speed(987.625).unwrap();
    let snapshot = session.navigation_preferences_stamp();
    for invalid in [
        0.0,
        24.0,
        20_001.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert!(session.set_fly_speed(invalid).is_err());
        assert_eq!(session.fly_speed(), 987.625);
        assert_eq!(session.navigation_preferences_stamp(), snapshot);
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(session.adjust_fly_speed(invalid).is_err());
        assert_eq!(session.navigation_preferences_stamp(), snapshot);
    }
    session.set_fly_speed(987.625).unwrap();
    assert_eq!(session.navigation_preferences_stamp(), snapshot);
    assert!(!path.exists());
    assert_eq!(session.camera, camera);
    assert_eq!(
        (
            session.camera_serial(),
            session.scene_serial(),
            session.view_serial,
            session.interaction_epoch
        ),
        serials
    );
    assert!(!session.document.is_dirty());
    assert_eq!(session.document.undo_labels().count(), 0);
}

#[test]
fn explicit_actions_preserve_owned_pick_and_apply_only_speed_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            true,
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let selection = session.selected_ids();
    let history: Vec<_> = session.document.undo_labels().map(str::to_string).collect();
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
    let picking = session
        .begin_measurement_pick(
            Actor::Agent,
            token,
            super::super::measurements::Endpoint::End,
            session.camera(),
            (128, 128),
            &session.measurement_pick_stamp(),
        )
        .unwrap();
    let token = picking["draft"]["token"].as_u64().unwrap();
    let draft = session.measurement_state()["draft"].clone();
    assert_eq!(draft["picking"]["valid"], true);
    let camera = session.camera;
    let view = session.view;
    let spatial = session.spatial;
    let serials = (
        session.camera_serial(),
        session.scene_serial(),
        session.view_serial,
        session.interaction_epoch,
    );
    session.set_fly_speed(987.625).unwrap();
    assert_eq!(
        action(&mut session, "save").unwrap()["write_committed"],
        true
    );
    let path = directory
        .path()
        .join(crate::navigation_preferences::FILE_NAME);
    let bytes = std::fs::read(&path).unwrap();
    session.adjust_fly_speed(240.0).unwrap();
    action(&mut session, "reload").unwrap();
    assert_eq!(session.fly_speed(), 1975.25);
    action(&mut session, "apply").unwrap();
    assert_eq!(session.fly_speed(), 987.625);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut reopened = Session::new(Err("no assets needed".into()));
    reopened.configure_recovery(Ok(directory.path().into()));
    assert_eq!(reopened.fly_speed(), 987.625);
    assert_eq!(reopened.document.undo_labels().count(), 0);
    action(&mut session, "reset").unwrap();
    assert_eq!(session.fly_speed(), 987.625);
    action(&mut session, "apply").unwrap();
    assert_eq!(session.fly_speed(), 600.0);
    assert_eq!(session.measurement_state()["draft"], draft);
    assert_eq!(session.camera, camera);
    assert_eq!(session.view, view);
    assert_eq!(session.spatial, spatial);
    assert_eq!(
        (
            session.camera_serial(),
            session.scene_serial(),
            session.view_serial,
            session.interaction_epoch
        ),
        serials
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
    session
        .cancel_measurement_pick(Actor::Agent, token)
        .unwrap();
    assert!(session.measurement_state()["draft"]["picking"].is_null());
    assert!(
        session.measurement_state()["draft"]["token"]
            .as_u64()
            .unwrap()
            > token
    );
}

#[test]
fn wheel_snapshots_and_other_sessions_reject_stale_actions_without_writing_or_applying() {
    for name in ["save", "apply", "reset"] {
        for delete in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut first = Session::new(Err("no assets needed".into()));
            first.configure_recovery(Ok(directory.path().into()));
            action(&mut first, "save").unwrap();
            let snapshot = first.navigation_preferences_stamp();
            first.adjust_fly_speed(240.0).unwrap();
            assert!(
                first
                    .change_navigation_preferences(Actor::Agent, name, &snapshot)
                    .unwrap_err()
                    .contains("changed")
            );
            let mut second = Session::new(Err("no assets needed".into()));
            second.configure_recovery(Ok(directory.path().into()));
            second.set_fly_speed(2000.0).unwrap();
            action(&mut second, "save").unwrap();
            let path = directory
                .path()
                .join(crate::navigation_preferences::FILE_NAME);
            if delete {
                std::fs::remove_file(&path).unwrap();
            }
            let bytes = std::fs::read(&path).ok();
            assert!(
                action(&mut first, name)
                    .unwrap_err()
                    .contains("outside this session")
            );
            assert_eq!(first.fly_speed(), 1200.0);
            assert_eq!(std::fs::read(&path).ok(), bytes);
            assert_eq!(first.navigation_preferences_state()["ready"], false);
            action(&mut first, "reload").unwrap();
            assert_eq!(first.fly_speed(), 1200.0);
            action(&mut first, "apply").unwrap();
            assert_eq!(first.fly_speed(), if delete { 600.0 } else { 2000.0 });
        }
    }
}

#[test]
fn malformed_and_unavailable_storage_never_revert_temporary_speed_or_destroy_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(crate::navigation_preferences::FILE_NAME);
    std::fs::write(&path, b"broken defaults").unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_fly_speed(1500.0).unwrap();
    session.configure_recovery(Ok(directory.path().into()));
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let preview = session.block_preview;
    assert_eq!(session.fly_speed(), 1500.0);
    for name in ["save", "apply", "reload"] {
        assert!(action(&mut session, name).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken defaults");
    }
    action(&mut session, "reset").unwrap();
    assert_eq!(session.fly_speed(), 1500.0);
    action(&mut session, "apply").unwrap();
    assert_eq!(session.fly_speed(), 600.0);
    assert_eq!(session.block_preview, preview);
    session.configure_navigation_preferences(Err("preferences unavailable".into()));
    session.adjust_fly_speed(240.0).unwrap();
    assert!(action(&mut session, "reset").is_err());
    assert_eq!(session.fly_speed(), 1200.0);
    assert_eq!(
        session.navigation_preferences_error().as_deref(),
        Some("preferences unavailable")
    );
    assert_eq!(session.block_preview, preview);
    assert_eq!(session.document.undo_labels().count(), 0);
}
