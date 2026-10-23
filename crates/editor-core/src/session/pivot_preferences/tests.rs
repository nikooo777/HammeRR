use super::*;
use crate::saved_defaults::SavedDefaults;
use glam::DVec3;
use map_core::query::Ray;
use render::handles::Mode;

fn action(session: &mut Session, name: &str) -> Result<Value, String> {
    session.change_pivot_preferences(Actor::User, name, &session.pivot_preferences_stamp())
}

fn set(session: &mut Session, point: Option<DVec3>) {
    session
        .set_transform_tool(
            Actor::User,
            TransformTool {
                pivot: point,
                ..session.transform_tool()
            },
        )
        .unwrap();
}

#[test]
fn shared_startup_is_once_and_explicit_apply_changes_only_the_originating_pivot() {
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = Session::with_saved_defaults(Err("no assets".into()), defaults.clone());
    let mut second = Session::with_saved_defaults(Err("no assets".into()), defaults.clone());
    let point = DVec3::new(1.125, -2.375, 3.0625);
    set(&mut first, Some(point));
    let path = directory.path().join(crate::pivot_preferences::FILE_NAME);
    assert!(!path.exists());
    let stale_peer = second.pivot_preferences_stamp();
    action(&mut first, "save").unwrap();
    assert_eq!(second.transform_tool().pivot, None);
    assert!(
        second
            .change_pivot_preferences(Actor::Agent, "apply", &stale_peer)
            .is_err()
    );
    let selected = second
        .create_block(
            Actor::User,
            DVec3::splat(-16.0),
            DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let original = TransformTool {
        mode: Some(Mode::Rotate),
        local: true,
        texture_lock: false,
        texture_scale_lock: Some(true),
        angle_step: 22.5,
        ..Default::default()
    };
    second.set_transform_tool(Actor::User, original).unwrap();
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let camera = second.camera();
    action(&mut second, "reload").unwrap();
    assert_eq!(second.transform_tool(), original);
    action(&mut second, "apply").unwrap();
    assert_eq!(
        second.transform_tool(),
        TransformTool {
            pivot: Some(point),
            ..original
        }
    );
    assert_eq!(second.selected_ids(), [selected]);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
    assert_eq!(second.camera(), camera);
    let later = Session::with_saved_defaults(Err("no assets".into()), defaults);
    assert_eq!(later.transform_tool().pivot, Some(point));
    assert_eq!(later.transform_tool().mode, None);
    let bytes = std::fs::read(&path).unwrap();
    set(&mut second, Some(DVec3::ZERO));
    second.configure_recovery(Ok(directory.path().join("other")));
    second.new_map(Actor::User, true).unwrap();
    assert_eq!(second.transform_tool().pivot, Some(DVec3::ZERO));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    action(&mut second, "reset").unwrap();
    assert_eq!(first.transform_tool().pivot, Some(point));
    assert_eq!(second.transform_tool().pivot, Some(DVec3::ZERO));
    action(&mut second, "apply").unwrap();
    assert_eq!(second.transform_tool().pivot, None);
    assert_eq!(first.transform_tool().pivot, Some(point));
}

#[test]
fn independent_startup_does_not_repeat_and_pivot_aba_rejects_inspected_actions() {
    let directory = tempfile::tempdir().unwrap();
    Store {
        directory: directory.path().into(),
    }
    .save(Pivot::WorldOrigin {}, false)
    .unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().into()));
    assert_eq!(session.transform_tool().pivot, Some(DVec3::ZERO));
    let stale = session.pivot_preferences_stamp();
    set(&mut session, Some(DVec3::X));
    set(&mut session, Some(DVec3::ZERO));
    for name in ["save", "apply", "reload", "reset"] {
        assert!(
            session
                .change_pivot_preferences(Actor::User, name, &stale)
                .is_err()
        );
    }
    let path = session.pivot_preferences_state()["path"].clone();
    set(&mut session, None);
    session.configure_recovery(Ok(directory.path().join("other")));
    assert_eq!(session.transform_tool().pivot, None);
    assert_eq!(session.pivot_preferences_state()["path"], path);
}

#[test]
fn independent_conflicts_failed_reload_and_storage_errors_remain_explicit_without_applying() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Session::new(Err("no assets".into()));
    let mut second = Session::new(Err("no assets".into()));
    first.configure_recovery(Ok(directory.path().into()));
    second.configure_recovery(Ok(directory.path().into()));
    set(&mut first, Some(DVec3::X * 1.25));
    action(&mut first, "save").unwrap();
    let path = directory.path().join(crate::pivot_preferences::FILE_NAME);
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        action(&mut second, "save")
            .unwrap_err()
            .contains("changed outside")
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(second.pivot_preferences_state()["ready"], false);
    action(&mut second, "reload").unwrap();
    assert_eq!(second.transform_tool().pivot, None);
    let saved = second.pivot_preferences_state()["saved"].clone();
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(action(&mut second, "reload").is_err());
    assert_eq!(second.pivot_preferences_state()["saved"], saved);
    assert_eq!(second.pivot_preferences_state()["defaults"], Value::Null);
    assert!(action(&mut second, "save").is_err());
    assert!(action(&mut second, "apply").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    action(&mut second, "reset").unwrap();
    assert_eq!(second.transform_tool().pivot, None);
    let mut unavailable = Session::new(Err("no assets".into()));
    unavailable.configure_recovery(Err("storage unavailable".into()));
    assert_eq!(unavailable.pivot_preferences_state()["ready"], false);
    assert_eq!(
        unavailable.pivot_preferences_state()["defaults"],
        Value::Null
    );
    assert!(action(&mut unavailable, "reset").is_err());
}

#[test]
fn apply_refuses_active_pivot_pick_and_nonfinite_values_reject_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.configure_recovery(Ok(directory.path().into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-64.0, -64.0, -16.0),
            DVec3::new(64.0, 64.0, 0.0),
            "test",
            false,
        )
        .unwrap();
    set(&mut session, Some(DVec3::X));
    let camera = session.camera();
    let token = session
        .begin_transform_pivot_pick(
            Actor::User,
            camera,
            (511, 511),
            &session.transform_pivot_pick_stamp(),
        )
        .unwrap()
        .picking
        .unwrap()
        .token;
    let before = session.transform_pivot_pick_state();
    assert!(action(&mut session, "apply").is_err());
    assert_eq!(session.transform_pivot_pick_state(), before);
    action(&mut session, "save").unwrap();
    assert!(session.transform_pivot_pick_state().picking.unwrap().valid);
    session
        .pick_transform_pivot_ray(
            Actor::User,
            token,
            Ray {
                origin: DVec3::new(3.125, -5.25, 96.0),
                direction: -DVec3::Z,
            },
        )
        .unwrap();
    let pick = session.transform_pivot_pick_state();
    action(&mut session, "save").unwrap();
    let saved = std::fs::read_to_string(directory.path().join(crate::pivot_preferences::FILE_NAME))
        .unwrap();
    assert!(!saved.contains("target"));
    assert!(!saved.contains("revision"));
    let mut reopened = Session::new(Err("no assets".into()));
    reopened.configure_recovery(Ok(directory.path().into()));
    assert_eq!(
        reopened
            .transform_tool()
            .pivot
            .map(|point| point.to_array()),
        pick.current_pivot
    );
    assert!(reopened.transform_pivot_pick_state().last_pick.is_none());
    set(
        &mut session,
        Some(DVec3::new(1_000_000.125, -70000.25, 0.0)),
    );
    action(&mut session, "save").unwrap();
    let mut distant = Session::new(Err("no assets".into()));
    distant.configure_recovery(Ok(directory.path().into()));
    assert_eq!(
        distant.transform_tool().pivot,
        session.transform_tool().pivot
    );
    let before = session.transform_tool();
    for coordinate in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        assert!(
            session
                .set_transform_tool(
                    Actor::Agent,
                    TransformTool {
                        pivot: Some(DVec3::splat(coordinate)),
                        ..before
                    }
                )
                .is_err()
        );
        assert_eq!(session.transform_tool(), before);
    }
    session.closing = true;
    let state = session.pivot_preferences_state();
    for name in ["save", "apply", "reload", "reset"] {
        assert!(
            session
                .change_pivot_preferences(Actor::Agent, name, state["snapshot"].as_str().unwrap())
                .is_err()
        );
    }
    assert_eq!(session.pivot_preferences_state(), state);
}
