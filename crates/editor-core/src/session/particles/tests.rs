use super::fixtures::{fixture, pcf};
use super::*;
use map_core::ObjectId;

#[test]
fn choices_share_search_inspection_review_guards_and_atomic_undo() {
    let (_directory, mut session, id) = fixture();
    let catalog = session
        .browse_particles("flame fire", "all", 0, 100)
        .unwrap();
    assert_eq!(catalog["total"], 1);
    assert_eq!(
        session
            .browse_particles("flame", "file:particles/test.pcf", 1, 1)
            .unwrap()["entries"],
        json!([])
    );
    assert!(session.browse_particles("", "all", 0, 101).is_err());
    assert!(session.browse_particles("", "unknown", 0, 1).is_err());
    let report = session.inspect_particle("TEST_FLAME").unwrap();
    assert_eq!(report["name"], "test_flame");
    assert_eq!(report["max_particles"], 100);
    assert_eq!(report["precache"], true);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let selection = session.selected_ids();
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert_eq!(
        session.begin_particle_choice(id, true).unwrap()["token"],
        token
    );
    session.choose_particle(token, "test_flame").unwrap();
    assert!(session.choose_particle(token, "guessed_name").is_err());
    assert_eq!(session.particle_choice(token).unwrap()["particle"], report);
    session.reload_material_assets(Actor::User);
    assert!(session.particle_choice(token).is_ok());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), selection);
    session.apply_particle_choice(Actor::Agent, token).unwrap();
    assert_eq!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("effect_name"),
        Some("test_flame")
    );
    assert_eq!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("unrelated"),
        Some("preserved")
    );
    assert_eq!(session.document().undo_labels().count(), history + 1);
    assert_eq!(
        session.browse_particles("", "used", 0, 10).unwrap()["total"],
        1
    );
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &source);
    session.redo(Actor::User).unwrap();
    assert_eq!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("effect_name"),
        Some("test_flame")
    );
}

#[test]
fn stale_and_replaced_choices_cancel_only_the_owned_token() {
    let (_directory, mut session, id) = fixture();
    let old = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert!(session.cancel_particle_choice(old).is_err());
    assert!(session.particle_choice(token).is_ok());
    session.choose_particle(token, "test_flame").unwrap();
    session
        .select(&[], super::super::SelectMode::Replace)
        .unwrap();
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    assert_eq!(session.current_particle_choice()["valid"], false);
    session.cancel_particle_choice(token).unwrap();
    assert_eq!(session.current_particle_choice()["token"], Value::Null);
    session
        .select(&[ObjectId::Entity(id)], super::super::SelectMode::Replace)
        .unwrap();
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    session.choose_particle(token, "test_flame").unwrap();
    session.reload_particle_assets();
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    session.cancel_particle_choice(token).unwrap();
    let other = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(glam::DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap();
    let ObjectId::Entity(other) = other else {
        unreachable!()
    };
    assert!(session.begin_particle_choice(other, false).is_err());
}

#[test]
fn failed_catalogs_are_not_cached_and_active_corruption_blocks_apply() {
    let (directory, mut session, id) = fixture();
    let path = directory.path().join("particles/test.pcf");
    let manifest = directory.path().join("particles/particles_manifest.txt");
    std::fs::remove_file(&manifest).unwrap();
    assert!(session.browse_particles("", "all", 0, 10).is_err());
    std::fs::write(&manifest, "particles_manifest { file particles/test.pcf }").unwrap();
    std::fs::write(&path, b"unsupported guessed_system").unwrap();
    assert!(session.browse_particles("", "all", 0, 10).is_err());
    std::fs::write(&path, pcf("test_flame")).unwrap();
    assert_eq!(
        session.browse_particles("", "all", 0, 10).unwrap()["total"],
        1
    );
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    session.choose_particle(token, "test_flame").unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let mut changed = pcf("test_flame");
    let name = changed
        .windows(b"root\0".len())
        .position(|bytes| bytes == b"root\0")
        .unwrap();
    changed[name + b"root\0".len()] = 1;
    std::fs::write(&path, &changed).unwrap();
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    std::fs::write(&path, pcf("test_flame")).unwrap();
    session.cancel_particle_choice(token).unwrap();
    std::fs::write(
        directory.path().join("particles/duplicate.pcf"),
        pcf("TEST_FLAME"),
    )
    .unwrap();
    std::fs::write(
        &manifest,
        "particles_manifest { file particles/test.pcf file particles/duplicate.pcf }",
    )
    .unwrap();
    session.reload_particle_assets();
    let report = session.browse_particles("flame", "all", 0, 10).unwrap();
    assert_eq!(report["total"], 2);
    assert_eq!(report["entries"][0]["ambiguous"], true);
    assert!(session.inspect_particle("test_flame").is_err());
}

#[test]
fn definition_and_preview_changes_require_review_without_source_changes() {
    let (_directory, mut session, id) = fixture();
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    session.choose_particle(token, "test_flame").unwrap();
    let source = session.document().vmf().clone();
    session
        .preview_transform(
            Actor::Agent,
            &[ObjectId::Entity(id)],
            map_core::edit::Transform {
                translation: glam::DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    assert!(session.begin_particle_choice(id, false).is_err());
    assert_eq!(session.document().vmf(), &source);
    session.cancel_particle_choice(token).unwrap();
    session.cancel_transform_preview();
    let token = session.begin_particle_choice(id, false).unwrap()["token"]
        .as_u64()
        .unwrap();
    session.choose_particle(token, "test_flame").unwrap();
    session.set_definitions(Actor::Agent, source_formats::fgd::Catalog::default());
    assert!(session.apply_particle_choice(Actor::User, token).is_err());
    session.cancel_particle_choice(token).unwrap();
}
