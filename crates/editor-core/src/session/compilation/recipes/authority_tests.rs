use super::*;
use std::fs;

fn attached(authority: &Authority) -> Session {
    Session::with_build_recipes(Err("no assets".into()), authority.clone())
}

fn id(state: &Value) -> String {
    state["active"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn attached_registry_shares_labels_errors_and_reload_without_rebinding_or_document_changes() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().to_owned()));
    let mut first = attached(&authority);
    let mut second = attached(&authority);
    first.configure_recovery(Ok(directory.path().join("first-recovery")));
    second.configure_recovery(Err("separate recovery failure".into()));
    first
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    second
        .preview_measurement(
            Actor::Agent,
            crate::session::measurements::Request {
                id: None,
                name: "Separate review".into(),
                start: glam::DVec3::ZERO,
                end: glam::DVec3::X,
            },
            &second.measurement_stamp(),
        )
        .unwrap();
    let before = |session: &Session| {
        json!({
            "source":session.document().vmf().write(&Default::default()),
            "history":session.history_snapshot(), "camera":format!("{:?}",session.camera()),
            "serials":[session.scene_serial(),session.view_serial(),session.camera_serial()],
            "property":session.property_draft().map(|draft|draft.token),
            "measurement":session.measurement_state()["draft"],
        })
    };
    let first_before = before(&first);
    let second_before = before(&second);
    let saved = first
        .save_build_recipe(Actor::User, "Shared", None, None)
        .unwrap();
    let id = id(&saved);
    assert_eq!(
        saved["path"],
        directory
            .path()
            .join(build_recipes::FILE_NAME)
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(second.build_recipes()["recipes"], saved["recipes"]);
    second.apply_build_recipe(Actor::Agent, &id, None).unwrap();
    first
        .rename_build_recipe(Actor::User, &id, "Peer rename", None)
        .unwrap();
    assert_eq!(second.compile_settings()["recipe"]["name"], "Peer rename");
    let options = second.compile_options();
    first
        .update_compile_settings(Actor::User, None, Some("engine"))
        .unwrap();
    first
        .save_build_recipe(Actor::User, "Replaced", Some(&id), None)
        .unwrap();
    assert!(second.compile_settings()["recipe"].is_null());
    assert_eq!(second.compile_options(), options);
    let path = directory.path().join(build_recipes::FILE_NAME);
    let valid = fs::read(&path).unwrap();
    fs::write(&path, b"malformed").unwrap();
    assert!(second.reload_build_recipes(Actor::Agent, None).is_err());
    assert_eq!(first.build_recipes()["persistence"], "blocked");
    assert_eq!(
        first.build_recipes()["error"],
        second.build_recipes()["error"]
    );
    fs::write(&path, valid).unwrap();
    first.reload_build_recipes(Actor::User, None).unwrap();
    assert!(second.build_recipes()["error"].is_null());
    second.delete_build_recipe(Actor::Agent, &id, None).unwrap();
    assert!(first.compile_settings()["recipe"].is_null());
    assert_eq!(before(&first), first_before);
    assert_eq!(before(&second), second_before);
    let mut independent = Session::new(Err("no assets".into()));
    independent
        .save_build_recipe(Actor::User, "Independent", None, None)
        .unwrap();
    assert_eq!(
        first.build_recipes()["recipes"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        independent.build_recipes()["recipes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn action_revision_guards_local_value_cycles_identity_changes_and_peer_lists() {
    let authority = Authority::default();
    let mut first = attached(&authority);
    let mut second = attached(&authority);
    let saved = first
        .save_build_recipe(Actor::User, "Saved fast", None, None)
        .unwrap();
    let id = id(&saved);
    let token = first.build_recipes()["revision"].as_u64().unwrap();
    first.apply_build_recipe(Actor::User, "fast", None).unwrap();
    assert!(
        first
            .rename_build_recipe(Actor::User, &id, "Stale identity", Some(token))
            .is_err()
    );
    let token = first.build_recipes()["revision"].as_u64().unwrap();
    first
        .update_compile_settings(Actor::Agent, None, Some("engine"))
        .unwrap();
    first
        .update_compile_settings(Actor::Agent, None, Some("none"))
        .unwrap();
    assert!(
        first
            .save_build_recipe(Actor::User, "Stale values", None, Some(token))
            .is_err()
    );
    let token = first.build_recipes()["revision"].as_u64().unwrap();
    second
        .rename_build_recipe(Actor::Agent, &id, "New shared list", None)
        .unwrap();
    assert!(
        first
            .delete_build_recipe(Actor::User, &id, Some(token))
            .is_err()
    );
    let token = first.build_recipes()["revision"].as_u64().unwrap();
    first
        .delete_build_recipe(Actor::User, &id, Some(token))
        .unwrap();
}

#[test]
fn omitted_tokens_serialize_concurrent_additions_and_origin_builds_keep_captured_identity() {
    let authority = Authority::default();
    let workers = (0..2)
        .map(|worker| {
            let mut session = attached(&authority);
            std::thread::spawn(move || {
                for recipe in 0..8 {
                    session
                        .save_build_recipe(
                            Actor::Agent,
                            &format!("Worker {worker} recipe {recipe}"),
                            None,
                            None,
                        )
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        attached(&authority).build_recipes()["recipes"]
            .as_array()
            .unwrap()
            .len(),
        18
    );
    let (directory, game, _) = crate::test_assets::game_process(true);
    let mut origin = Session::with_build_recipes(Ok(game), authority.clone());
    origin.replace_document(
        map_core::Document::new(crate::test_assets::room()),
        Some(directory.path().join("room.vmf")),
    );
    let saved = origin
        .save_build_recipe(Actor::User, "Captured original", None, None)
        .unwrap();
    let id = id(&saved);
    let _request = origin.begin_compile(Actor::User, None).unwrap();
    let mut peer = attached(&authority);
    peer.rename_build_recipe(Actor::Agent, &id, "Changed during build", None)
        .unwrap();
    assert_eq!(
        origin.compile_settings()["recipe"]["name"],
        "Changed during build"
    );
    assert_eq!(
        origin.compile_status()["job"]["recipe"]["name"],
        "Captured original"
    );
    assert!(origin.delete_build_recipe(Actor::User, &id, None).is_err());
    peer.delete_build_recipe(Actor::Agent, &id, None).unwrap();
    assert!(origin.compile_settings()["recipe"].is_null());
    assert_eq!(
        origin.compile_status()["job"]["recipe"]["name"],
        "Captured original"
    );
    origin.finish_compile(Actor::User, &Err("fixture stopped before execution".into()));
}
