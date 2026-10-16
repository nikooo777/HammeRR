use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::DVec3;
use map_core::{query::Ray, transform::Transform};
use source_formats::fgd::Catalog;

fn entity(session: &mut Session, actor: Actor, classname: &str) -> u32 {
    let ObjectId::Entity(id) = session
        .create_entity(actor, classname, Some(DVec3::ZERO), BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    id
}

#[test]
fn literal_declarations_report_failures_and_keep_other_forms_and_precedence_explicit() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        for (helper, error) in [
            ("studio()", "one literal"),
            ("studioprop(model)", "models/*.mdl"),
            ("studio(\"models/a.mdl\", 2)", "one literal"),
            (
                "studio(\"models/a.mdl\") studioprop(\"models/a.mdl\")",
                "ambiguous",
            ),
            ("studio(\"../outside.mdl\")", "relative"),
        ] {
            session.set_definitions(
                actor,
                Catalog::parse(&format!("@PointClass {helper} = sample []")).unwrap(),
            );
            let id = entity(&mut session, actor, "sample");
            let before = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let report = session
                .entity_helper_report(id)
                .unwrap()
                .model_guide
                .unwrap();
            assert_eq!(report.state, "unsupported");
            assert!(report.error.unwrap().contains(error), "{helper}");
            assert!(!report.displayed);
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.document().undo_labels().count(), history);
        }
        for (classname, extra, reason) in [
            ("light", "", "specialized_light"),
            ("move_rope", "", "specialized_rope"),
            ("sample", "decal()", "specialized_helper"),
        ] {
            session.set_definitions(
                actor,
                Catalog::parse(&format!(
                    "@PointClass studio(\"models/missing.mdl\") {extra} = {classname} []"
                ))
                .unwrap(),
            );
            let id = entity(&mut session, actor, classname);
            let report = session
                .entity_helper_report(id)
                .unwrap()
                .model_guide
                .unwrap();
            assert_eq!(report.suppression_reason, Some(reason));
            assert_eq!(report.state, "not_loaded");
        }
        session.set_definitions(
            actor,
            Catalog::parse("@PointClass studio(\"models/missing.mdl\") = sample []").unwrap(),
        );
        let id = entity(&mut session, actor, "sample");
        session
            .set_entity_properties(
                actor,
                id,
                &[("model".into(), Some("models/authored_missing.mdl".into()))],
            )
            .unwrap();
        let report = session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap();
        assert_eq!(report.suppression_reason, Some("authored_model"));
        assert!(!report.displayed);
    }
}

#[test]
fn literal_model_lookup_work_is_capped_even_when_every_asset_is_missing() {
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass studio(\"models/missing.mdl\") = sample []").unwrap(),
    );
    let ids = (0..65)
        .map(|_| entity(&mut session, Actor::Agent, "sample"))
        .collect::<Vec<_>>();
    let first = session
        .entity_helper_report(ids[0])
        .unwrap()
        .model_guide
        .unwrap();
    assert_eq!(first.state, "unavailable");
    let last = session
        .entity_helper_report(ids[64])
        .unwrap()
        .model_guide
        .unwrap();
    assert_eq!(last.suppression_reason, Some("scene_model_limit"));
    assert_eq!(last.state, "not_loaded");
    assert_eq!(session.build_scene().stats.fgd_model_helpers, 0);
}

#[test]
fn installed_literal_meshes_share_cache_and_preview_but_never_spatial_model_targets() {
    for actor in [Actor::User, Actor::Agent] {
        let Ok(game) = crate::startup::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let definition = "@BaseClass size(4 4 4) studio(\"models/props_junk/wood_crate001a.mdl\") = helper [] @PointClass base(helper) = sample []";
        session.set_definitions(actor, Catalog::parse(definition).unwrap());
        let id = entity(&mut session, actor, "sample");
        let object = ObjectId::Entity(id);
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session
            .object_bounds_in(session.document(), &[object])
            .unwrap();
        let support = session
            .placement_points_in(session.document(), &[object])
            .unwrap();
        let guide = session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap();
        assert!(guide.displayed);
        assert_eq!(guide.source, "fgd_literal");
        assert!(guide.visual_bounds.unwrap()[1][0] > bounds.1.x);
        assert!(
            session
                .model_instances(session.document())
                .instances
                .is_empty()
        );
        assert!(session.inspect_entity_model(id).is_err());
        let scene = session.build_scene();
        assert_eq!(scene.stats.fgd_model_helpers, 1);
        assert_eq!(scene.stats.model_entities, 0);
        assert_eq!((scene.mins.as_dvec3(), scene.maxs.as_dvec3()), bounds);
        assert_eq!(
            session
                .placement_points_in(session.document(), &[object])
                .unwrap()
                .len(),
            support.len()
        );
        assert_eq!(support.len(), 8);
        let outside_box = Ray {
            origin: DVec3::new(10.0, -128.0, 0.0),
            direction: DVec3::Y,
        };
        assert!(session.pick(&outside_box).is_none());
        let through_box = Ray {
            origin: DVec3::new(0.0, -128.0, 0.0),
            direction: DVec3::Y,
        };
        assert_eq!(session.pick(&through_box).unwrap().object, object);
        session.select(&[object], SelectMode::Replace).unwrap();
        assert_eq!(session.selection_bounds().unwrap(), bounds);
        session
            .preview_transform(
                actor,
                &[object],
                Transform {
                    translation: DVec3::X * 64.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let moved = session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap()
            .visual_bounds
            .unwrap();
        assert!((moved[0][0] - guide.visual_bounds.unwrap()[0][0] - 64.0).abs() < 1e-6);
        session.cancel_transform_preview();
        let asset = session
            .models
            .borrow_mut()
            .get(&session.game, "models/props_junk/wood_crate001a.mdl")
            .unwrap();
        session.set_definitions(
            actor,
            Catalog::parse(&definition.replace("studio(", "studioprop(")).unwrap(),
        );
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .model_guide
                .unwrap()
                .displayed
        );
        assert!(std::sync::Arc::ptr_eq(
            &asset,
            &session
                .models
                .borrow_mut()
                .get(&session.game, "models/props_junk/wood_crate001a.mdl")
                .unwrap()
        ));
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    enabled: true,
                    bounds: [DVec3::splat(10.0), DVec3::splat(20.0)],
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert_eq!(session.build_scene().stats.fgd_model_helpers, 0);
        assert_eq!(
            session
                .entity_helper_report(id)
                .unwrap()
                .model_guide
                .unwrap()
                .suppression_reason,
            Some("outside_working_region")
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Clear,
                &session.working_region_stamp(),
            )
            .unwrap();
        session.refresh_model_assets(actor);
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .model_guide
                .unwrap()
                .displayed
        );
        assert!(!std::sync::Arc::ptr_eq(
            &asset,
            &session
                .models
                .borrow_mut()
                .get(&session.game, "models/props_junk/wood_crate001a.mdl")
                .unwrap()
        ));
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn source_failures_stay_cached_until_reload_and_never_seed_texture_prefetch() {
    let Ok(installed) = crate::startup::mount_game(None) else {
        return;
    };
    let (directory, game) = crate::test_assets::game();
    let path = "models/props_junk/wood_crate001a.mdl";
    let stem = path.strip_suffix(".mdl").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&format!(
            "@PointClass size(4 4 4) studio(\"{path}\") = sample []"
        ))
        .unwrap(),
    );
    let id = entity(&mut session, Actor::Agent, "sample");
    let original = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap()
            .state,
        "unavailable"
    );
    for suffix in [".mdl", ".vvd", ".dx90.vtx"] {
        let relative = format!("{stem}{suffix}");
        let target = directory.path().join(&relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, installed.read(&relative).unwrap().unwrap()).unwrap();
    }
    assert_eq!(
        session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap()
            .state,
        "unavailable"
    );
    session.refresh_model_assets(Actor::Agent);
    let guide = session
        .entity_helper_report(id)
        .unwrap()
        .model_guide
        .unwrap();
    assert_eq!(guide.state, "ready");
    assert_eq!(guide.material_policy, "flat_editor");
    let material_count = session.materials.len();
    assert_eq!(session.build_scene().stats.fgd_model_helpers, 1);
    assert_eq!(session.materials.len(), material_count);
    let reload = session.reload_material_assets(Actor::Agent);
    assert_eq!(reload.models_refreshed, 0);
    assert_eq!(session.materials.len(), material_count);
    std::fs::write(directory.path().join(path), b"invalid replacement").unwrap();
    assert!(
        session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap()
            .displayed
    );
    session.refresh_model_assets(Actor::Agent);
    let failed = session
        .entity_helper_report(id)
        .unwrap()
        .model_guide
        .unwrap();
    assert_eq!(failed.state, "unavailable");
    assert!(failed.error.is_some());
    assert_eq!(session.build_scene().stats.fgd_model_helpers, 0);
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.document().undo_labels().count(), history);
}

#[test]
fn entity_model_guides_off_defers_geometry_reads_and_preserves_authored_models() {
    let Ok(installed) = crate::startup::mount_game(None) else {
        return;
    };
    let (directory, game) = crate::test_assets::game();
    let path = "models/props_junk/wood_crate001a.mdl";
    let stem = path.strip_suffix(".mdl").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&format!(
            "@PointClass size(4 4 4) studio(\"{path}\") = sample []"
        ))
        .unwrap(),
    );
    let id = entity(&mut session, Actor::Agent, "sample");
    session.set_view(render::ViewOptions {
        entity_model_guides: false,
        ..session.view()
    });
    let source = session.document().vmf().clone();
    let serial = session.scene_serial();
    let report = session
        .entity_helper_report(id)
        .unwrap()
        .model_guide
        .unwrap();
    assert_eq!(report.suppression_reason, Some("view_disabled"));
    assert_eq!(report.state, "not_loaded");
    assert!(report.error.is_none() && report.visual_bounds.is_none());
    assert_eq!(session.build_scene().stats.fgd_model_helpers, 0);
    for suffix in [".mdl", ".vvd", ".dx90.vtx"] {
        let relative = format!("{stem}{suffix}");
        let target = directory.path().join(&relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, installed.read(&relative).unwrap().unwrap()).unwrap();
    }
    session.set_view(render::ViewOptions {
        entity_model_guides: true,
        ..session.view()
    });
    assert!(
        session
            .entity_helper_report(id)
            .unwrap()
            .model_guide
            .unwrap()
            .displayed
    );
    let shared = session.view();
    assert_eq!(
        session
            .build_scene_for_view(render::ViewOptions {
                entity_model_guides: false,
                ..shared
            })
            .stats
            .fgd_model_helpers,
        0
    );
    assert_eq!(session.view(), shared);
    assert_eq!(session.scene_serial(), serial);
    assert_eq!(session.document().vmf(), &source);
    session
        .create_entity(
            Actor::Agent,
            "prop_dynamic",
            Some(DVec3::X * 128.0),
            BTreeMap::from([("model".into(), path.into())]),
        )
        .unwrap();
    session.set_view(render::ViewOptions {
        entity_model_guides: false,
        ..session.view()
    });
    let scene = session.build_scene();
    assert_eq!(scene.stats.fgd_model_helpers, 0);
    assert_eq!(scene.stats.model_entities, 1);
}
