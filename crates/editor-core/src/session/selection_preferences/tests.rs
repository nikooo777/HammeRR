use super::*;
use crate::saved_defaults::SavedDefaults;
use crate::session::{SelectMode, volume};
use glam::DVec3;
use map_core::region::Match;

fn action(session: &mut Session, name: &str) -> Result<Value, String> {
    session.change_selection_preferences(Actor::User, name, &session.selection_preferences_stamp())
}

fn object_volume(session: &mut Session) -> u64 {
    session
        .preview_volume_selection(volume::Spec {
            mins: DVec3::splat(-32.0),
            maxs: DVec3::splat(32.0),
            rule: Match::Contained,
            mode: SelectMode::Replace,
            faces: false,
        })
        .unwrap()
}

#[test]
fn defaults_apply_retires_only_originating_object_gestures_and_startup_does_not_repeat() {
    let directory = tempfile::tempdir().unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = Session::with_saved_defaults(Err("no assets".into()), defaults.clone());
    let mut second = Session::with_saved_defaults(Err("no assets".into()), defaults);
    let filters = SelectionFilters {
        world_brushes: false,
        brush_entities: false,
        point_entities: false,
    };
    first.set_selection_filters(Actor::User, filters).unwrap();
    action(&mut first, "save").unwrap();
    let selected = second
        .create_block(
            Actor::User,
            DVec3::splat(-16.0),
            DVec3::splat(16.0),
            "test",
            false,
        )
        .unwrap();
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let camera = second.camera();
    let first_token = object_volume(&mut first);
    let second_token = object_volume(&mut second);
    action(&mut second, "reload").unwrap();
    assert_eq!(second.volume_preview().unwrap().token, second_token);
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    action(&mut second, "apply").unwrap();
    assert_eq!(second.selection_filters(), filters);
    assert!(
        second
            .commit_volume_selection(Actor::User, second_token)
            .is_err()
    );
    assert_eq!(first.volume_preview().unwrap().token, first_token);
    assert_eq!(second.selected_ids(), [selected]);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
    assert_eq!(second.camera(), camera);
    second
        .set_selection_filters(Actor::User, SelectionFilters::default())
        .unwrap();
    second.configure_recovery(Ok(directory.path().join("other")));
    second.new_map(Actor::User, true).unwrap();
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    assert_eq!(
        second.selection_preferences_state()["saved"],
        json!(filters)
    );

    let mut independent = Session::new(Err("no assets".into()));
    independent.configure_recovery(Ok(directory.path().into()));
    assert_eq!(independent.selection_filters(), filters);
    independent
        .set_selection_filters(Actor::User, SelectionFilters::default())
        .unwrap();
    let path = independent.selection_preferences_state()["path"].clone();
    independent.configure_recovery(Ok(directory.path().join("independent-other")));
    assert_eq!(independent.selection_filters(), SelectionFilters::default());
    assert_eq!(independent.selection_preferences_state()["path"], path);
}

#[test]
fn independent_defaults_conflicts_and_failed_reload_require_explicit_recovery_without_applying() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = Session::new(Err("no assets".into()));
    let mut second = Session::new(Err("no assets".into()));
    first.configure_recovery(Ok(directory.path().into()));
    second.configure_recovery(Ok(directory.path().into()));
    let filters = SelectionFilters {
        world_brushes: false,
        ..Default::default()
    };
    first.set_selection_filters(Actor::User, filters).unwrap();
    action(&mut first, "save").unwrap();
    let path = directory
        .path()
        .join(crate::selection_preferences::FILE_NAME);
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        action(&mut second, "save")
            .unwrap_err()
            .contains("changed outside")
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(second.selection_preferences_state()["ready"], false);
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    action(&mut second, "reload").unwrap();
    assert_eq!(
        second.selection_preferences_state()["saved"],
        json!(filters)
    );
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    let stale = second.selection_preferences_stamp();
    second.set_selection_filters(Actor::User, filters).unwrap();
    second
        .set_selection_filters(Actor::User, SelectionFilters::default())
        .unwrap();
    assert!(
        second
            .change_selection_preferences(Actor::User, "apply", &stale)
            .is_err()
    );
    let oversized = vec![b'x'; crate::selection_preferences::MAX_FILE_BYTES as usize + 1];
    std::fs::write(&path, &oversized).unwrap();
    assert!(
        action(&mut second, "reload")
            .unwrap_err()
            .contains("exceeds")
    );
    assert_eq!(second.selection_preferences_state()["ready"], false);
    assert_eq!(
        second.selection_preferences_state()["saved"],
        json!(filters)
    );
    assert!(action(&mut second, "save").is_err());
    assert!(action(&mut second, "apply").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), oversized);
    assert_eq!(
        action(&mut second, "reset").unwrap()["write_committed"],
        true
    );
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    assert_eq!(second.selection_preferences_state()["ready"], true);
    let mut legacy = serde_json::to_vec(&json!({"version":1,"filters":filters})).unwrap();
    legacy.resize(4097, b' ');
    std::fs::write(&path, &legacy).unwrap();
    assert!(
        action(&mut second, "reload")
            .unwrap_err()
            .contains("4096 bytes for version 1")
    );
    assert_eq!(std::fs::read(&path).unwrap(), legacy);
}

fn profile_action(session: &mut Session, operation: ProfileAction) -> Value {
    session
        .change_selection_profile(
            Actor::User,
            operation,
            &session.selection_preferences_stamp(),
        )
        .unwrap()
}

#[test]
fn legacy_startup_and_shared_profiles_remain_independent_through_edits_apply_reload_and_reset() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(crate::selection_preferences::FILE_NAME);
    let startup = SelectionFilters {
        world_brushes: false,
        ..Default::default()
    };
    let legacy = serde_json::to_vec(&json!({"version":1,"filters":startup})).unwrap();
    std::fs::write(&path, &legacy).unwrap();
    let defaults = SavedDefaults::configured(Ok(directory.path().into()));
    let mut first = Session::with_saved_defaults(Err("no assets".into()), defaults.clone());
    let mut second = Session::with_saved_defaults(Err("no assets".into()), defaults.clone());
    assert_eq!(std::fs::read(&path).unwrap(), legacy);
    assert_eq!(first.selection_filters(), startup);
    assert_eq!(second.selection_filters(), startup);
    let profile_filters = SelectionFilters {
        world_brushes: true,
        brush_entities: false,
        point_entities: false,
    };
    first
        .set_selection_filters(Actor::User, profile_filters)
        .unwrap();
    let stale = second.selection_preferences_stamp();
    let saved = profile_action(
        &mut first,
        ProfileAction::Save {
            name: "  World only  ".into(),
        },
    );
    let id = saved["profile_id"].as_str().unwrap().to_owned();
    assert_eq!(saved["profiles"][0]["name"], "World only");
    assert_eq!(saved["saved"], json!(startup));
    assert_eq!(
        second.selection_profiles_state()["profiles"],
        saved["profiles"]
    );
    assert_eq!(second.selection_filters(), startup);
    assert!(
        second
            .change_selection_profile(
                Actor::User,
                ProfileAction::Delete { id: id.clone() },
                &stale
            )
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
    let source = second.document().vmf().clone();
    let history = second.history_snapshot();
    let token = object_volume(&mut second);
    let applied = profile_action(&mut second, ProfileAction::Apply { id: id.clone() });
    assert_eq!(applied["write_committed"], false);
    assert_eq!(second.selection_filters(), profile_filters);
    assert!(second.commit_volume_selection(Actor::User, token).is_err());
    assert_eq!(second.selected_ids(), [selected]);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
    action(&mut first, "save").unwrap();
    assert_eq!(
        first.selection_profiles_state()["profiles"],
        saved["profiles"]
    );
    let renamed = profile_action(
        &mut second,
        ProfileAction::Rename {
            id: id.clone(),
            name: "Structure".into(),
        },
    );
    assert_eq!(renamed["profiles"][0]["id"], id);
    assert_eq!(renamed["saved"], json!(profile_filters));
    second
        .set_selection_filters(Actor::User, SelectionFilters::default())
        .unwrap();
    let replaced = profile_action(&mut second, ProfileAction::Replace { id: id.clone() });
    assert_eq!(
        replaced["profiles"][0]["filters"],
        json!(SelectionFilters::default())
    );
    assert_eq!(replaced["profiles"][0]["name"], "Structure");
    assert_eq!(first.selection_filters(), profile_filters);
    let bytes = std::fs::read(&path).unwrap();
    let snapshot = first.selection_preferences_stamp();
    assert!(
        first
            .change_selection_profile(
                Actor::User,
                ProfileAction::Save {
                    name: "STRUCTURE".into()
                },
                &snapshot
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::write(&path, b"malformed catalogue").unwrap();
    assert!(action(&mut first, "reload").is_err());
    assert!(
        first
            .change_selection_profile(
                Actor::User,
                ProfileAction::Apply { id: id.clone() },
                &first.selection_preferences_stamp()
            )
            .is_err()
    );
    let reset = action(&mut first, "reset").unwrap();
    assert!(reset.get("profiles").is_none());
    assert_eq!(
        first.selection_profiles_state()["profiles"],
        replaced["profiles"]
    );
    assert_eq!(first.selection_filters(), profile_filters);
    assert_eq!(reset["saved"], json!(SelectionFilters::default()));
    let restarted = Session::with_saved_defaults(Err("no assets".into()), defaults);
    assert_eq!(restarted.selection_filters(), SelectionFilters::default());
    profile_action(&mut second, ProfileAction::Delete { id: id.clone() });
    assert_eq!(second.selection_filters(), SelectionFilters::default());
    let saved = profile_action(
        &mut second,
        ProfileAction::Save {
            name: "Replacement".into(),
        },
    );
    assert_ne!(saved["profile_id"], id);
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.history_snapshot(), history);
}
