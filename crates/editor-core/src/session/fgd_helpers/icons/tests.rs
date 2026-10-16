use super::*;
use crate::session::{Actor, SelectMode};
use glam::DVec3;
use map_core::query::Ray;
use source_formats::fgd::Catalog;

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    std::fs::write(directory.path().join("materials/test/icon.vmt"), "Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" $no_fullbright 1 }").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        actor,
        Catalog::parse("@PointClass size(4 4 4) iconsprite(\"test/icon\") = sample []").unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    (directory, session, id)
}

#[test]
fn entity_images_off_defers_admission_and_retains_cached_failures_until_reload() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, id) = fixture(actor);
        let path = directory.path().join("materials/test/icon.vmt");
        let healthy = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"invalid icon before admission").unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let serial = session.scene_serial();
        session.set_view(render::ViewOptions {
            entity_images: false,
            ..session.view()
        });
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .icon_sprite
            .unwrap();
        assert!(!report.displayed);
        assert_eq!(report.suppression_reason, Some("view_disabled"));
        assert!(report.source.is_none());
        let ordinary = session.build_scene();
        assert!(ordinary.icons.is_empty());
        assert_eq!(ordinary.vertices.len(), 24);
        std::fs::write(&path, &healthy).unwrap();
        session.set_view(render::ViewOptions {
            entity_images: true,
            ..session.view()
        });
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .icon_sprite
                .unwrap()
                .displayed
        );
        let shared = session.view();
        let temporary = session.build_scene_for_view(render::ViewOptions {
            entity_images: false,
            ..shared
        });
        assert!(temporary.icons.is_empty());
        assert_eq!(session.view(), shared);
        assert_eq!(session.scene_serial(), serial);
        std::fs::write(&path, b"invalid icon after reload").unwrap();
        session.reload_material_assets(actor);
        assert_eq!(
            session
                .entity_helper_report(id)
                .unwrap()
                .icon_sprite
                .unwrap()
                .status,
            "unavailable"
        );
        std::fs::write(&path, healthy).unwrap();
        for enabled in [false, false, true] {
            session.set_view(render::ViewOptions {
                entity_images: enabled,
                ..session.view()
            });
        }
        assert_eq!(
            session
                .entity_helper_report(id)
                .unwrap()
                .icon_sprite
                .unwrap()
                .status,
            "unavailable"
        );
        session.reload_material_assets(actor);
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .icon_sprite
                .unwrap()
                .displayed
        );
        session.set_view(render::ViewOptions {
            entity_images: false,
            ..session.view()
        });
        session.set_definitions(
            actor,
            Catalog::parse("@PointClass iconsprite() = sample []").unwrap(),
        );
        let unsupported = session
            .entity_helper_report(id)
            .unwrap()
            .icon_sprite
            .unwrap();
        assert_eq!(unsupported.status, "unsupported");
        assert!(unsupported.error.is_some() && !unsupported.displayed);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}
#[test]
fn ready_icon_replaces_only_box_fill_and_keeps_shared_bounds_picks_and_history() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, id) = fixture(actor);
        let object = ObjectId::Entity(id);
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let camera = session.camera;
        let report = session.entity_helper_report(id).unwrap();
        let icon = report.icon_sprite.unwrap();
        assert_eq!(icon.status, "ready");
        assert!(icon.displayed);
        assert_eq!(icon.world_size, [32.0, 16.0]);
        assert!(
            !report
                .unsupported
                .iter()
                .any(|helper| helper == "iconsprite")
        );
        let bounds = (DVec3::splat(-2.0), DVec3::splat(2.0));
        assert_eq!(
            session.object_bounds_in(session.document(), &[object]),
            Some(bounds)
        );
        let ray = Ray {
            origin: DVec3::new(0.0, -100.0, 0.0),
            direction: DVec3::Y,
        };
        assert_eq!(session.pick(&ray).unwrap().object, object);
        let mesh = session.build_scene();
        assert_eq!(mesh.icons.len(), 1);
        assert!(mesh.vertices.is_empty());
        assert_eq!((mesh.mins.as_dvec3(), mesh.maxs.as_dvec3()), bounds);
        assert!(!mesh.selection_outline_ranges.is_empty());
        session.select(&[], SelectMode::Replace).unwrap();
        let unselected = session.build_scene();
        assert_eq!(unselected.icons.len(), 1);
        assert!(unselected.selection_outline_ranges.is_empty());
        assert_eq!((unselected.mins, unselected.maxs), (mesh.mins, mesh.maxs));
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.camera, camera);
    }
}
#[test]
fn admission_failure_is_visible_and_cached_until_shared_material_reload() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, id) = fixture(actor);
        let path = directory.path().join("materials/test/icon.vmt");
        std::fs::write(&path, "UnlitGeneric { $basetexture test/checker }").unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session.entity_helper_report(id).unwrap();
        assert!(report.status.contains("Only a direct Sprite"));
        assert_eq!(report.icon_sprite.unwrap().status, "unavailable");
        let generic = session.build_scene();
        assert!(generic.icons.is_empty());
        assert_eq!(generic.vertices.len(), 24);
        std::fs::write(&path, "Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" }").unwrap();
        assert!(session.build_scene().icons.is_empty());
        session.reload_material_assets(actor);
        assert_eq!(session.build_scene().icons.len(), 1);
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .icon_sprite
                .unwrap()
                .displayed
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}
#[test]
fn model_sprite_and_malformed_declarations_never_trigger_icon_admission() {
    let (_directory, mut session, id) = fixture(Actor::Agent);
    for (helper, expected) in [
        (
            "iconsprite(\"test/icon\") studio(\"missing.mdl\")",
            "model_or_sprite_helper",
        ),
        (
            "iconsprite(\"test/icon\") sprite()",
            "model_or_sprite_helper",
        ),
        ("iconsprite()", "unavailable_icon"),
        ("iconsprite(\"test/icon\", 2)", "unavailable_icon"),
    ] {
        session.set_definitions(
            Actor::Agent,
            Catalog::parse(&format!("@PointClass size(4 4 4) {helper} = sample []")).unwrap(),
        );
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .icon_sprite
            .unwrap();
        assert!(!report.displayed);
        assert_eq!(report.suppression_reason, Some(expected));
        assert!(report.source.is_none());
        assert!(session.build_scene().icons.is_empty());
    }
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(4 4 4) iconsprite(\"test/icon\") = sample []").unwrap(),
    );
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("model".into(), Some("missing.mdl".into()))],
        )
        .unwrap();
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .icon_sprite
            .unwrap()
            .suppression_reason,
        Some("model_or_sprite_helper")
    );
}
#[test]
fn scene_icon_limit_and_inspection_share_deterministic_entity_order() {
    let (_directory, mut session, first) = fixture(Actor::Agent);
    let mut last = first;
    for _ in 0..render::icons::MAX_INSTANCES {
        let ObjectId::Entity(id) = session
            .create_entity(Actor::Agent, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap()
        else {
            panic!()
        };
        last = id;
    }
    let source = session.document().vmf().clone();
    assert_eq!(
        session.build_scene().icons.len(),
        render::icons::MAX_INSTANCES
    );
    assert!(
        session
            .entity_helper_report(first)
            .unwrap()
            .icon_sprite
            .unwrap()
            .displayed
    );
    assert_eq!(
        session
            .entity_helper_report(last)
            .unwrap()
            .icon_sprite
            .unwrap()
            .suppression_reason,
        Some("scene_icon_limit")
    );
    assert_eq!(session.document().vmf(), &source);
}
