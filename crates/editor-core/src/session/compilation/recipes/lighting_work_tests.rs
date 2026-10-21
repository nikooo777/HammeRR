use super::*;
use crate::build_recipes::FILE_NAME;
use crate::session::compilation::BuildSettingsUpdate;
use std::fs;

#[test]
fn lighting_work_recipes_roundtrip_exact_units_and_default_legacy_records() {
    for actor in [Actor::User, Actor::Agent] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut current = Session::new(Err("no assets needed".into()));
        current.configure_build_recipes_at(Ok(path.clone()));
        let initial = current.build_recipes();
        assert_eq!(
            initial["settings"]["lighting_work_limit"],
            compiler::DEFAULT_LIGHTING_WORK_LIMIT
        );
        for recipe in initial["recipes"].as_array().unwrap() {
            assert_eq!(
                recipe["settings"]["lighting_work_limit"],
                compiler::DEFAULT_LIGHTING_WORK_LIMIT
            );
        }
        current
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting_work_limit: Some(1_234_567_890),
                    ..Default::default()
                },
            )
            .unwrap();
        current
            .save_build_recipe(actor, "Outdoor", None, None)
            .unwrap();
        let mut stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored["recipes"][0]["settings"]["lighting_work_limit"],
            1_234_567_890u64
        );
        let mut restarted = Session::new(Err("no assets needed".into()));
        restarted.configure_build_recipes_at(Ok(path.clone()));
        restarted
            .apply_build_recipe(actor, "custom:1", None)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting_work_limit,
            1_234_567_890
        );
        restarted.apply_build_recipe(actor, "fast", None).unwrap();
        assert_eq!(
            restarted.compile_options().lighting_work_limit,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT
        );
        stored["recipes"][0]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("lighting_work_limit");
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        restarted.reload_build_recipes(actor, None).unwrap();
        restarted
            .apply_build_recipe(actor, "custom:1", None)
            .unwrap();
        assert_eq!(
            restarted.compile_options().lighting_work_limit,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT
        );
        restarted.apply_build_recipe(actor, "final", None).unwrap();
        assert_eq!(
            restarted.compile_options().lighting_work_limit,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT
        );
    }
}

#[test]
fn malformed_lighting_work_recipe_values_preserve_registry_settings_and_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    let mut current = Session::new(Err("no assets needed".into()));
    current.configure_build_recipes_at(Ok(path.clone()));
    current
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                lighting_work_limit: Some(1_234_567_890),
                ..Default::default()
            },
        )
        .unwrap();
    current
        .save_build_recipe(Actor::Agent, "Outdoor", None, None)
        .unwrap();
    let valid = fs::read(&path).unwrap();
    let file: Value = serde_json::from_slice(&valid).unwrap();
    let options = current.compile_options();
    let recipes = current.build_recipes()["recipes"].clone();
    let history = current.history_snapshot();
    let mut invalid = Vec::new();
    for value in [
        json!(null),
        json!(0),
        json!(-1),
        json!(1.5),
        json!("1000000000"),
        json!(compiler::MAX_LIGHTING_WORK_LIMIT + 1),
        json!(u64::MAX),
        json!(true),
    ] {
        let mut malformed = file.clone();
        malformed["recipes"][0]["settings"]["lighting_work_limit"] = value;
        invalid.push(serde_json::to_vec(&malformed).unwrap());
    }
    invalid.push(
        String::from_utf8(valid)
            .unwrap()
            .replace(
                "\"lighting_work_limit\": 1234567890",
                "\"lighting_work_limit\": 1234567890, \"lighting_work_limit\": 2000000000",
            )
            .into_bytes(),
    );
    for bytes in invalid {
        fs::write(&path, &bytes).unwrap();
        assert!(current.reload_build_recipes(Actor::Agent, None).is_err());
        let state = current.build_recipes();
        assert_eq!(state["persistence"], "blocked");
        assert_eq!(state["recipes"], recipes);
        assert_eq!(current.compile_options(), options);
        assert_eq!(current.history_snapshot(), history);
        assert!(
            current
                .save_build_recipe(Actor::Agent, "Changed", Some("custom:1"), None)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
