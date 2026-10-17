use super::*;
use crate::build_recipes::{FILE_NAME, File, MAX_ERROR_BYTES};
use std::fs;

fn owned_state(session: &Session) -> Value {
    json!({
        "source": session.document().vmf().write(&Default::default()),
        "history": session.history_snapshot(),
        "selection": format!("{:?}", session.selected_ids()),
        "draft": session.measurement_state()["draft"],
        "camera": format!("{:?}", session.camera()),
        "serials": [session.scene_serial(), session.view_serial(), session.camera_serial()],
        "options": Settings::from(session.compile_options()),
    })
}

#[test]
fn reload_reconciles_only_matching_active_identity_and_rejects_stale_actions() {
    for actor in [Actor::User, Actor::Agent] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut session = Session::new(Err("no assets".into()));
        session.configure_build_recipes_at(Ok(path.clone()));
        let saved = session
            .save_build_recipe(actor, "Original", None, None)
            .unwrap();
        let revision = saved["revision"].as_u64().unwrap();
        let id = saved["active"]["id"].as_str().unwrap().to_owned();
        session
            .preview_measurement(
                Actor::Agent,
                crate::session::measurements::Request {
                    id: None,
                    name: "Owned clearance".into(),
                    start: glam::DVec3::ZERO,
                    end: glam::DVec3::X,
                },
                &session.measurement_stamp(),
            )
            .unwrap();
        let before = owned_state(&session);
        let mut file: File = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        file.recipes[0].name = "Renamed outside".into();
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let loaded = session.reload_build_recipes(actor, Some(revision)).unwrap();
        assert_eq!(loaded["active"]["name"], "Renamed outside");
        assert!(session.reload_build_recipes(actor, Some(revision)).is_err());
        assert!(
            session
                .delete_build_recipe(actor, &id, Some(revision))
                .is_err()
        );
        assert!(
            session
                .apply_build_recipe(actor, "final", Some(revision))
                .is_err()
        );
        assert_eq!(session.build_recipes(), loaded);
        assert_eq!(owned_state(&session), before);
        file.recipes[0].settings.bounces = 1;
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(session.reload_build_recipes(actor, None).unwrap()["active"].is_null());
        assert_eq!(owned_state(&session), before);
        file.recipes[0].settings.bounces = 0;
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(session.reload_build_recipes(actor, None).unwrap()["active"].is_null());
        session.apply_build_recipe(actor, &id, None).unwrap();
        fs::remove_file(&path).unwrap();
        let removed = session.reload_build_recipes(actor, None).unwrap();
        assert!(removed["active"].is_null());
        assert_eq!(removed["recipes"].as_array().unwrap().len(), 2);
        assert_eq!(owned_state(&session), before);
    }
}

#[test]
fn failed_reload_preserves_valid_registry_until_repair_without_overwriting_storage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    let mut session = Session::new(Err("no assets".into()));
    session.configure_build_recipes_at(Ok(path.clone()));
    let valid = session
        .save_build_recipe(Actor::User, "Retained", None, None)
        .unwrap();
    let saved = fs::read(&path).unwrap();
    let before = owned_state(&session);
    for bytes in [b"broken".to_vec(), vec![b' '; MAX_FILE_BYTES + 1]] {
        fs::write(&path, &bytes).unwrap();
        assert!(session.reload_build_recipes(Actor::Agent, None).is_err());
        let state = session.build_recipes();
        assert_eq!(state["recipes"], valid["recipes"]);
        assert_eq!(state["active"], valid["active"]);
        assert_eq!(state["persistence"], "blocked");
        assert!(!state["custom_editable"].as_bool().unwrap());
        assert!(state["error"].as_str().unwrap().len() <= MAX_ERROR_BYTES);
        assert!(
            session
                .save_build_recipe(Actor::User, "Blocked", None, None)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(owned_state(&session), before);
    }
    fs::write(&path, saved).unwrap();
    let reloaded = session.reload_build_recipes(Actor::User, None).unwrap();
    assert_eq!(reloaded["persistence"], "persistent");
    assert!(reloaded["error"].is_null());
    assert_eq!(reloaded["active"], valid["active"]);
    assert_eq!(owned_state(&session), before);
    session
        .save_build_recipe(Actor::User, "Recovered", None, None)
        .unwrap();
}

#[test]
fn reload_guards_busy_closing_and_retains_captured_job_identity() {
    let (directory, game, _) = crate::test_assets::game_process(true);
    let path = directory.path().join(FILE_NAME);
    let mut session = Session::new(Ok(game));
    session.configure_build_recipes_at(Ok(path.clone()));
    session.replace_document(
        map_core::Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    session
        .save_build_recipe(Actor::Agent, "Captured", None, None)
        .unwrap();
    let request = session.begin_compile(Actor::Agent, None).unwrap();
    let before = session.build_recipes();
    let captured = session.compile_status()["job"]["recipe"].clone();
    assert!(
        session
            .reload_build_recipes(Actor::User, None)
            .unwrap_err()
            .contains("active compile")
    );
    assert_eq!(session.build_recipes(), before);
    session.finish_compile(
        Actor::Agent,
        &Err("fixture stopped before execution".into()),
    );
    assert!(session.compiling());
    assert!(
        session
            .reload_build_recipes(Actor::User, None)
            .unwrap_err()
            .contains("active compile")
    );
    drop(request);
    assert!(!session.compiling());
    let mut file: File = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    file.recipes[0].name = "Later name".into();
    fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    session.reload_build_recipes(Actor::Agent, None).unwrap();
    assert_eq!(session.build_recipes()["active"]["name"], "Later name");
    assert_eq!(session.compile_status()["job"]["recipe"], captured);
    session.shutdown();
    let before = session.build_recipes();
    assert!(
        session
            .reload_build_recipes(Actor::Agent, None)
            .unwrap_err()
            .contains("closing")
    );
    assert_eq!(session.build_recipes(), before);
}

#[test]
fn unavailable_reload_does_not_block_session_only_recipes() {
    let mut session = Session::new(Err("no assets".into()));
    session
        .save_build_recipe(Actor::User, "Local", None, None)
        .unwrap();
    let before = session.build_recipes();
    assert!(session.reload_build_recipes(Actor::Agent, None).is_err());
    assert_eq!(session.build_recipes(), before);
    session
        .save_build_recipe(Actor::User, "Still local", None, None)
        .unwrap();
}
