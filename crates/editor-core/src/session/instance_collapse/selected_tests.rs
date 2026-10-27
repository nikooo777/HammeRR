use super::*;
use glam::DVec3;
use source_formats::vmf::Vmf;

fn fixture() -> (tempfile::TempDir, Session, PathBuf, PathBuf, [u32; 3]) {
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let first = session.document.vmf().entities.last().unwrap().id;
    let (later, hidden) = session
        .edit(Actor::User, "other root references", |tx| {
            let later = (1..first)
                .find(|id| !tx.vmf().object_ids().any(|used| used == *id))
                .unwrap();
            let mut reference = Entity::new(later, "func_instance");
            reference.properties.set("file", "missing-unselected.vmf");
            reference.properties.set("origin", "64 0 0");
            tx.add_entity(reference)?;
            let hidden = tx.new_object_id()?;
            let mut reference = Entity::new(hidden, "func_instance");
            reference.hidden = true;
            reference.properties.set("file", "missing-hidden.vmf");
            tx.add_entity(reference)?;
            Ok((later, hidden))
        })
        .unwrap();
    session.save(Actor::User, None).unwrap();
    (directory, session, parent, child, [first, later, hidden])
}

fn prepare_selected(session: &mut Session) -> Request {
    session
        .begin_instance_collapse_scoped(
            Actor::User,
            &session.instance_collapse_stamp(),
            Scope::SelectedLeadingReferences,
        )
        .unwrap()
}

#[test]
fn selected_leading_instance_collapse_preserves_terrain_unselected_roots_and_one_undo() {
    let (_directory, mut session, parent, child, [first, later, hidden]) = fixture();
    let mut content = Document::new(Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap());
    let solid = content.vmf().world.solids[0].id;
    let side = content
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    content
        .edit("terrain", |tx| {
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )
        })
        .unwrap();
    std::fs::write(&child, content.vmf().write(&Default::default())).unwrap();
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let original = session.document.vmf().clone();
    let history = session.history_snapshot();
    let disk = [&parent, &child].map(|path| std::fs::read(path).unwrap());
    let request = prepare_selected(&mut session);
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    assert_eq!(ready["scope"], "selected_leading_references");
    assert_eq!(
        ready["selected"],
        json!([crate::identity::id_string(ObjectId::Entity(first))])
    );
    assert_eq!(ready["removed"], ready["selected"]);
    assert!(later < first);
    assert_eq!(
        ready["visible_root_order"],
        json!([format!("entity:{first}"), format!("entity:{later}")])
    );
    assert_eq!(ready["stats"]["instances"], 1);
    assert_eq!(ready["stats"]["terrain_patches"], 1);
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    let shown = session.render_document().vmf().clone();
    for id in [later, hidden] {
        assert_eq!(
            shown.entities.iter().find(|entity| entity.id == id),
            original.entities.iter().find(|entity| entity.id == id)
        );
    }
    let actual = shown.world.solids.last().unwrap();
    for (actual, expected) in actual
        .sides
        .iter()
        .zip(&content.vmf().world.solids[0].sides)
    {
        let mut expected = expected.clone();
        expected.id = actual.id;
        assert_eq!(*actual, expected);
    }
    let token = ready["token"].as_u64().unwrap();
    let request = session.apply_instance_collapse(Actor::User, token).unwrap();
    let applied = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    assert_eq!(applied["captured_preview"]["scope"], ready["scope"]);
    assert_eq!(session.document.vmf(), &shown);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &original);
    session.redo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &shown);
    for (path, bytes) in [&parent, &child].into_iter().zip(disk) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn selected_leading_instance_collapse_rejects_invalid_scope_selection_and_stale_work_atomically() {
    let (_directory, mut session, parent, child, [first, later, hidden]) = fixture();
    let brush = ObjectId::Solid(session.document.vmf().world.solids[0].id);
    let original = session.document.vmf().clone();
    let history = session.history_snapshot();
    let parent_bytes = std::fs::read(&parent).unwrap();
    assert!(
        instances::selected_collapse_roots(
            &original,
            &[ObjectId::Entity(first), ObjectId::Entity(first)]
        )
        .unwrap_err()
        .contains("unique")
    );
    assert_eq!(
        instances::selected_collapse_roots(
            &original,
            &[ObjectId::Entity(later), ObjectId::Entity(first)]
        )
        .unwrap(),
        vec![ObjectId::Entity(first), ObjectId::Entity(later)]
    );
    for ids in [
        vec![],
        vec![brush],
        vec![ObjectId::Entity(first), brush],
        vec![ObjectId::Entity(hidden)],
        vec![ObjectId::Entity(later)],
    ] {
        session.select(&ids, SelectMode::Replace).unwrap();
        let request = session.begin_instance_collapse_scoped(
            Actor::User,
            &session.instance_collapse_stamp(),
            Scope::SelectedLeadingReferences,
        );
        assert!(request.is_err());
        assert!(session.get_instance_collapse()["token"].is_null());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
    }
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let request = prepare_selected(&mut session);
    let token = request.token();
    session
        .select(&[ObjectId::Entity(later)], SelectMode::Replace)
        .unwrap();
    assert!(session.get_instance_collapse()["token"].is_null());
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert!(session.get_instance_collapse()["token"].is_null());
    assert!(!session.geometry.instance_collapse.worker.busy());
    assert!(session.apply_instance_collapse(Actor::User, token).is_err());
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let request = prepare_selected(&mut session);
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    let shown = session.render_document().vmf().clone();
    let token = ready["token"].as_u64().unwrap();
    assert!(
        session
            .apply_instance_collapse(Actor::Agent, token)
            .is_err()
    );
    assert!(
        session
            .begin_instance_collapse_scoped(
                Actor::Agent,
                &session.instance_collapse_stamp(),
                Scope::AllVisibleReferences
            )
            .is_err()
    );
    assert_eq!(session.get_instance_collapse()["scope"], ready["scope"]);
    assert_eq!(session.render_document().vmf(), &shown);
    session
        .select(&[ObjectId::Entity(later)], SelectMode::Replace)
        .unwrap();
    assert!(
        session
            .begin_instance_collapse_scoped(
                Actor::User,
                &session.instance_collapse_stamp(),
                Scope::SelectedLeadingReferences
            )
            .is_err()
    );
    assert!(session.get_instance_collapse()["token"].is_null());
    assert_eq!(session.render_document().vmf(), &original);
    assert!(session.apply_instance_collapse(Actor::User, token).is_err());
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let request = prepare_selected(&mut session);
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    let token = ready["token"].as_u64().unwrap();
    let reviewed = session
        .review_instance_collapse(Actor::Agent, token)
        .unwrap();
    assert_eq!(reviewed["scope"], ready["scope"]);
    assert_eq!(reviewed["selected"], ready["selected"]);
    let token = reviewed["token"].as_u64().unwrap();
    let request = session
        .apply_instance_collapse(Actor::Agent, token)
        .unwrap();
    let bytes = std::fs::read(&child).unwrap();
    std::fs::write(
        &child,
        format!("{}\n", String::from_utf8(bytes.clone()).unwrap()),
    )
    .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::Agent, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(std::fs::read(&parent).unwrap(), parent_bytes);
    std::fs::write(&child, bytes).unwrap();
    session
        .cancel_instance_collapse(Actor::Agent, token)
        .unwrap();
    let request = prepare_selected(&mut session);
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    let shown = session.render_document().vmf().clone();
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(
        session.get_instance_collapse()["scope"],
        "all_visible_references"
    );
    assert_eq!(ready["scope"], "selected_leading_references");
    assert_eq!(
        session.get_instance_collapse()["preview_scope"],
        "selected_leading_references"
    );
    let token = session.get_instance_collapse()["token"].as_u64().unwrap();
    assert!(session.apply_instance_collapse(Actor::User, token).is_err());
}

#[test]
fn selected_leading_collapse_keeps_retained_root_names_in_the_compiler_target_registry() {
    let (_directory, mut session, _parent, child, [first, later, _hidden]) = fixture();
    let mut content = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    let mut target = Entity::new(1000, "info_target");
    target.properties.set("origin", "0 0 96");
    target.properties.set("targetname", "aim");
    let mut spot = Entity::new(1001, "light_spot");
    for (key, value) in [
        ("origin", "16 0 128"),
        ("target", "aim"),
        ("pitch", "-45"),
        ("_light", "255 128 64 200"),
    ] {
        spot.properties.set(key, value);
    }
    content.entities.extend([spot, target]);
    std::fs::write(&child, content.write(&Default::default())).unwrap();
    session
        .edit(Actor::User, "retained root name collision", |tx| {
            tx.entity_mut(first)?.properties.set("targetname", "first");
            tx.entity_mut(later)?
                .properties
                .set("targetname", "FIRST-AIM");
            Ok(())
        })
        .unwrap();
    session
        .select(&[ObjectId::Entity(first)], SelectMode::Replace)
        .unwrap();
    let original = session.document.vmf().clone();
    let history = session.history_snapshot();
    let bytes = std::fs::read(&child).unwrap();
    let request = prepare_selected(&mut session);
    let error = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(
        error.contains("collides with a visible root entity"),
        "{error}"
    );
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.render_document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(std::fs::read(child).unwrap(), bytes);
}
