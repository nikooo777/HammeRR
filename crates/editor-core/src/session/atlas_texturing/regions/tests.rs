use super::*;
use source_formats::vmf::Vmf;

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, FaceId) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let map_core::ObjectId::Solid(solid) = session
        .create_block(
            actor,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let face = FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session
        .select_faces(&[face], crate::session::SelectMode::Replace)
        .unwrap();
    (directory, session, face)
}

fn inspected(session: &mut Session) -> Value {
    session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report
}

fn fit_region() -> FitRegion {
    FitRegion {
        rectangle: Rectangle {
            x: 16,
            y: 8,
            width: 32,
            height: 16,
        },
        padding: 2,
    }
}

fn save(session: &mut Session, actor: Actor, name: &str) -> Value {
    let atlas = inspected(session);
    let catalog = session.atlas_regions_snapshot();
    session
        .save_atlas_region(
            actor,
            name,
            "Materials/Test/Checker.vmt",
            fit_region(),
            atlas["snapshot"].as_str().unwrap(),
            &catalog,
        )
        .unwrap()
}

#[test]
fn named_atlas_regions_share_actors_persist_and_reuse_owned_fit_with_independent_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, face) = fixture(actor);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let selection = session.selected_faces();
        let saved = save(&mut session, actor, " Red \"été\" ");
        assert_eq!(saved["regions"][0]["name"], "Red \"été\"");
        assert_eq!(saved["regions"][0]["material"], "test/checker");
        assert_eq!(saved["regions"][0]["dimensions"], json!([64, 32]));
        assert_eq!(session.document().undo_labels().count(), history + 1);
        let catalog_vmf = session.document().vmf().clone();
        assert_eq!(catalog_vmf.world, before.world);
        assert_eq!(catalog_vmf.entities, before.entities);
        let text = catalog_vmf.write(&Default::default());
        session.replace_document(map_core::Document::new(Vmf::parse(&text).unwrap()), None);
        session
            .select_faces(&selection, crate::session::SelectMode::Replace)
            .unwrap();
        let camera = session.camera();
        let listed = session.atlas_regions_state().unwrap();
        let atlas = inspected(&mut session);
        let id = saved["saved_id"].as_u64().unwrap() as u32;
        let chosen = session
            .choose_atlas_region(
                id,
                "test/checker",
                atlas["snapshot"].as_str().unwrap(),
                listed["snapshot"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(chosen["rectangle"], saved["regions"][0]["rectangle"]);
        assert_eq!(chosen["padding"], 2);
        assert_eq!(session.document().undo_labels().count(), 0);
        session
            .preview_atlas_fit_region(
                actor,
                &[face],
                "test/checker",
                fit_region(),
                chosen["snapshot"].as_str().unwrap(),
                None,
            )
            .unwrap();
        let token = session.face_preview_token();
        let preview = session.render_document().vmf().clone();
        let snapshot = session.atlas_regions_snapshot();
        assert!(
            session
                .rename_atlas_region(actor, id, "Blocked", &snapshot)
                .is_err()
        );
        assert!(session.delete_atlas_region(actor, id, &snapshot).is_err());
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.face_preview_token(), token);
        session.commit_face_preview(actor).unwrap();
        assert_eq!(session.document().vmf(), &preview);
        assert_eq!(session.document().undo_labels().count(), 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &catalog_vmf);
        let snapshot = session.atlas_regions_snapshot();
        let renamed = session
            .rename_atlas_region(actor, id, "Blue", &snapshot)
            .unwrap();
        assert_eq!(renamed["regions"][0]["name"], "Blue");
        assert!(session.delete_atlas_region(actor, id, &snapshot).is_err());
        let deleted = session
            .delete_atlas_region(actor, id, renamed["snapshot"].as_str().unwrap())
            .unwrap();
        assert_eq!(deleted["regions"], json!([]));
        session.undo(actor).unwrap();
        assert_eq!(
            session.atlas_regions_state().unwrap()["regions"][0]["name"],
            "Blue"
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &catalog_vmf);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_faces(), selection);
    }
}

#[test]
fn named_atlas_regions_reject_duplicate_invalid_stale_and_changed_same_size_image_atomically() {
    let (directory, mut session, _) = fixture(Actor::Agent);
    let saved = save(&mut session, Actor::Agent, "Panel");
    let atlas = inspected(&mut session);
    let snapshot = session.atlas_regions_snapshot();
    let before = session.document().vmf().clone();
    let history = session.history_snapshot();
    for (name, region, source, catalog) in [
        (
            " panel ",
            fit_region(),
            atlas["snapshot"].as_str().unwrap(),
            snapshot.as_str(),
        ),
        (
            "Invalid",
            FitRegion {
                padding: 8,
                ..fit_region()
            },
            atlas["snapshot"].as_str().unwrap(),
            snapshot.as_str(),
        ),
        ("Stale", fit_region(), "stale", snapshot.as_str()),
        (
            "Stale",
            fit_region(),
            atlas["snapshot"].as_str().unwrap(),
            "stale",
        ),
    ] {
        assert!(
            session
                .save_atlas_region(Actor::Agent, name, "test/checker", region, source, catalog)
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
    }
    let id = saved["saved_id"].as_u64().unwrap() as u32;
    let path = directory.path().join("materials/test/checker.vtf");
    let mut pixels = std::fs::read(&path).unwrap();
    pixels[80] ^= 127;
    std::fs::write(&path, pixels).unwrap();
    session.reload_material_assets(Actor::Agent);
    let changed = inspected(&mut session);
    assert_eq!(changed["width"], atlas["width"]);
    assert_eq!(changed["height"], atlas["height"]);
    assert_ne!(changed["image_identity"], atlas["image_identity"]);
    assert!(
        session
            .choose_atlas_region(
                id,
                "test/checker",
                changed["snapshot"].as_str().unwrap(),
                &snapshot
            )
            .unwrap_err()
            .contains("image changed")
    );
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.history_snapshot(), history);
    session
        .delete_atlas_region(Actor::Agent, id, &snapshot)
        .unwrap();
}
