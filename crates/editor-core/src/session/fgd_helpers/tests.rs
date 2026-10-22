use super::*;
use crate::session::{Actor, SelectMode, working_region};
use glam::{DMat3, DVec3};
use source_formats::fgd::Catalog;

const DEFINITION: &str = "@BaseClass size(-24 -12 0, 40 20 72) color(12 34 56) = shared [] @PointClass base(shared) sphere(radius) = sample [ angles(angle) : \"Angles\" : \"0 90 0\" radius(float) : \"Radius\" : 64 ]";

#[test]
fn implicit_sphere_binding_shares_named_geometry_defaults_and_read_only_reload() {
    let definition = "@BaseClass sphere() sphere(radius) = shared [ radius(string) : \"Max Audible Distance\" : \"64\" ] @PointClass base(shared) = sample []";
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        session.set_definitions(actor, Catalog::parse(definition).unwrap());
        let ObjectId::Entity(id) = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap()
        else {
            panic!()
        };
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let scene = session.build_scene();
        let mesh = sphere_vertices(&scene);
        let report = session.entity_helper_report(id).unwrap();
        assert_eq!(report.spheres.len(), 1);
        assert!(report.spheres[0].implicit_property && report.spheres[0].displayed);
        assert_eq!(report.spheres[0].value_source, "authored");
        assert!(
            report
                .status
                .contains("sphere() uses the implicit radius binding")
        );
        session.set_definitions(
            actor,
            Catalog::parse(&definition.replace("sphere()", "sphere(radius)")).unwrap(),
        );
        let named = session.entity_helper_report(id).unwrap();
        assert_eq!(named.spheres.len(), 1);
        assert!(!named.spheres[0].implicit_property);
        assert_eq!(sphere_vertices(&session.build_scene()), mesh);
        session.set_definitions(actor, Catalog::parse(definition).unwrap());
        session
            .set_entity_properties(actor, id, &[("radius".into(), None)])
            .unwrap();
        let fallback = session.entity_helper_report(id).unwrap();
        assert!(fallback.spheres[0].implicit_property);
        assert_eq!(fallback.spheres[0].value_source, "fgd_default");
        assert_eq!(sphere_vertices(&session.build_scene()), mesh);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn fgd_size_drives_shared_scene_picks_bounds_regions_and_inspector_without_map_edits() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = (DVec3::new(-24.0, -12.0, 0.0), DVec3::new(40.0, 20.0, 72.0));
        assert_eq!(
            session.object_bounds_in(session.document(), &[id]),
            Some(bounds)
        );
        assert_eq!(
            session.object_bounds_in_basis(session.document(), &[id], DMat3::IDENTITY),
            Some(bounds)
        );
        let mesh = session.build_scene();
        assert_eq!(mesh.mins.as_dvec3(), bounds.0);
        assert_eq!(mesh.maxs.as_dvec3(), bounds.1);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.color[..3] == [12, 34, 56])
        );
        let ray = Ray {
            origin: DVec3::new(30.0, -100.0, 64.0),
            direction: DVec3::Y,
        };
        assert_eq!(session.pick(&ray).unwrap().object, id);
        assert_eq!(
            session.pick(&ray).unwrap().point,
            DVec3::new(30.0, -12.0, 64.0)
        );
        assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 1);
        let inspector = session.entity_inspector(entity).unwrap();
        let report = inspector.helpers.unwrap();
        session.select(&[id], SelectMode::Replace).unwrap();
        let selected = session.build_scene();
        assert!(!selected.selection_outline_ranges.is_empty());
        assert_eq!(selected.vertices.len(), mesh.vertices.len());
        session.select(&[], SelectMode::Replace).unwrap();
        assert_eq!(report.bounds_source, "fgd_size");
        assert!(report.unsupported.is_empty());
        assert_eq!(report.spheres[0].radius, Some(64.0));
        assert!(report.status.contains("world-aligned"));
        assert_eq!(
            session
                .select_region(
                    &map_core::region::Region::from_bounds(
                        DVec3::new(25.0, -20.0, 50.0),
                        DVec3::new(45.0, 25.0, 80.0)
                    )
                    .unwrap(),
                    map_core::region::Match::Intersecting,
                    false
                )
                .objects,
            [id]
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::new(25.0, -20.0, 50.0), DVec3::new(45.0, 25.0, 80.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(
            session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(100.0), DVec3::splat(200.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(session.pick(&ray).is_none());
        assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 0);
        assert_eq!(session.build_scene().stats.point_entities, 0);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn invalid_size_keeps_a_diagnostic_marker_visible_and_reload_updates_every_shared_bound() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(
            actor,
            Catalog::parse("@PointClass size(0 0 0) = sample []").unwrap(),
        );
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session.entity_helper_report(entity).unwrap();
        assert!(report.invalid_size);
        assert_eq!(report.bounds_source, "invalid_fgd_size");
        assert!(report.local_bounds.is_none());
        assert!(report.status.contains("generic editor marker"));
        assert!(!report.errors.is_empty());
        assert!(
            session
                .build_scene()
                .vertices
                .iter()
                .all(|vertex| vertex.color[..3] == [255, 176, 48])
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(100.0), DVec3::splat(200.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(
            session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        let records = session.get_working_region(0, 50).unwrap();
        assert_eq!(records["objects"][0]["bounds_source"], "invalid_fgd_size");
        assert!(!records["objects"][0]["bounds_error"].is_null());
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        assert!(
            !session
                .working_evaluation(session.document())
                .objects
                .contains(&id)
        );
        assert_eq!(
            session.entity_helper_report(entity).unwrap().bounds_source,
            "fgd_size"
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn point_lights_share_fgd_boxes_while_directional_lights_keep_their_helpers() {
    for actor in [Actor::User, Actor::Agent] {
        for (class, source, half, vertices) in [
            ("light", "fgd_size", 1.0, 24),
            ("light_spot", "specialized_light", 18.0, 0),
        ] {
            let mut session = Session::new(Err("no assets needed".into()));
            session.set_definitions(
                actor,
                Catalog::parse(&format!(
                    "@PointClass size(2 2 2) color(1 2 3) = {class} []"
                ))
                .unwrap(),
            );
            let id = session
                .create_entity(actor, class, Some(DVec3::ZERO), BTreeMap::new())
                .unwrap();
            let ObjectId::Entity(entity) = id else {
                unreachable!()
            };
            session.select(&[], SelectMode::Replace).unwrap();
            assert_eq!(
                session.entity_helper_report(entity).unwrap().bounds_source,
                source
            );
            assert_eq!(
                session.object_bounds_in(session.document(), &[id]),
                Some((DVec3::splat(-half), DVec3::splat(half)))
            );
            let scene = session.build_scene();
            if vertices > 0 {
                assert_eq!(scene.vertices.len(), vertices);
                assert!(
                    scene
                        .vertices
                        .iter()
                        .all(|vertex| vertex.color[..3] == [1, 2, 3])
                );
            } else {
                assert!(scene.vertices.len() > 24);
            }
        }
    }
}

#[test]
fn loaded_model_geometry_takes_precedence_over_a_conflicting_fgd_box() {
    let Ok(game) = crate::startup::mount_game(None) else {
        return;
    };
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        Actor::Agent,
        Catalog::parse("@PointClass size(2 2 2) sphere(radius) = prop_static [ radius(float) : \"Radius\" : 128 ]").unwrap(),
    );
    let id = session
        .create_entity(
            Actor::Agent,
            "prop_static",
            Some(DVec3::ZERO),
            BTreeMap::from([(
                "model".into(),
                "models/props_junk/wood_crate001a.mdl".into(),
            )]),
        )
        .unwrap();
    let ObjectId::Entity(entity) = id else {
        unreachable!()
    };
    let report = session.entity_helper_report(entity).unwrap();
    assert_eq!(report.bounds_source, "model");
    assert_eq!(report.spheres[0].suppression_reason, Some("loaded_model"));
    assert!(sphere_vertices(&session.build_scene()).is_empty());
    assert_eq!(
        serde_json::to_value(report.world_bounds).unwrap(),
        session.inspect_entity_model(entity).unwrap()["world_bounds"]
    );
    assert!(session.build_scene().vertices.len() > 24);
    assert!(
        !session
            .working_evaluation(session.document())
            .helpers
            .contains_key(&id)
    );
}

fn sphere_vertices(mesh: &render::SceneMesh) -> Vec<&render::scene::LineVertex> {
    mesh.lines
        .iter()
        .filter(|vertex| vertex.color == [80, 220, 255, 255])
        .collect()
}

#[test]
fn sphere_guides_share_selection_visibility_and_preview_centers_without_changing_bounds() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session.object_bounds_in(session.document(), &[id]);
        let selected = session.build_scene();
        assert_eq!(sphere_vertices(&selected).len(), 3 * 32 * 2);
        let selected_bounds = (selected.mins, selected.maxs);
        let report = session.entity_helper_report(entity).unwrap();
        assert!(report.spheres[0].displayed);
        assert_eq!(report.spheres[0].center, Some([0.0; 3]));
        assert_eq!(
            report.spheres[0].guide_bounds,
            Some([[-64.0; 3], [64.0; 3]])
        );
        assert!(
            session
                .pick(&Ray {
                    origin: DVec3::new(60.0, -100.0, 0.0),
                    direction: DVec3::Y
                })
                .is_none()
        );
        session.set_view(crate::session::ViewOptions {
            selection_outlines: false,
            ..session.view()
        });
        assert_eq!(sphere_vertices(&session.build_scene()).len(), 3 * 32 * 2);
        session.select(&[], SelectMode::Replace).unwrap();
        let plain = session.build_scene();
        assert!(sphere_vertices(&plain).is_empty());
        assert_eq!((plain.mins, plain.maxs), selected_bounds);
        assert_eq!(
            plain
                .vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>(),
            selected
                .vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>()
        );
        assert_eq!(plain.indices, selected.indices);
        let report = session.entity_helper_report(entity).unwrap();
        assert_eq!(report.spheres[0].state, "ready");
        assert!(!report.spheres[0].displayed);
        assert_eq!(report.spheres[0].suppression_reason, Some("not_selected"));
        session.select(&[id], SelectMode::Replace).unwrap();
        session
            .preview_transform(
                actor,
                &[id],
                map_core::edit::Transform {
                    translation: DVec3::new(100.0, 200.0, 300.0),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let report = session.entity_helper_report(entity).unwrap();
        assert!(report.spheres[0].displayed);
        assert_eq!(report.spheres[0].center, Some([100.0, 200.0, 300.0]));
        assert_eq!(
            report.world_bounds,
            Some([[-24.0, -12.0, 0.0], [40.0, 20.0, 72.0]])
        );
        let preview = session.build_scene();
        assert!(
            sphere_vertices(&preview)
                .iter()
                .any(|vertex| vertex.position == [164.0, 200.0, 300.0])
        );
        session.cancel_transform_preview();
        assert_eq!(
            session.entity_helper_report(entity).unwrap().spheres[0].center,
            Some([0.0; 3])
        );
        session
            .set_working_region(
                actor,
                working_region::Action::Set {
                    bounds: [DVec3::splat(50.0), DVec3::splat(60.0)],
                    enabled: true,
                },
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(sphere_vertices(&session.build_scene()).is_empty());
        assert_eq!(
            session.entity_helper_report(entity).unwrap().spheres[0].suppression_reason,
            Some("outside_working_region")
        );
        assert_eq!(session.object_bounds_in(session.document(), &[id]), bounds);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .set_working_region(
                actor,
                working_region::Action::Clear,
                &session.working_region_stamp(),
            )
            .unwrap();
        session
            .edit(actor, "import hidden entity", |tx| {
                tx.entity_mut(entity)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert!(sphere_vertices(&session.build_scene()).is_empty());
        assert_eq!(
            session.entity_helper_report(entity).unwrap().spheres[0].suppression_reason,
            Some("authored_hidden")
        );
    }
}

#[test]
fn sphere_property_edits_are_atomic_and_unavailable_states_do_not_invent_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(actor, Catalog::parse(DEFINITION).unwrap());
        let id = session
            .create_entity(actor, "sample", Some(DVec3::ZERO), BTreeMap::new())
            .unwrap();
        let ObjectId::Entity(entity) = id else {
            unreachable!()
        };
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        assert!(
            session
                .set_entity_properties(
                    actor,
                    entity,
                    &[
                        ("radius".into(), Some("invalid".into())),
                        ("targetname".into(), Some("must_not_apply".into()))
                    ]
                )
                .is_err()
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .set_entity_properties(actor, entity, &[("radius".into(), None)])
            .unwrap();
        let row = &session.entity_helper_report(entity).unwrap().spheres[0];
        assert_eq!(row.value_source, "fgd_default");
        assert_eq!(row.radius, Some(64.0));
        for raw in ["0", "-1", "65537"] {
            session
                .set_entity_properties(actor, entity, &[("radius".into(), Some(raw.into()))])
                .unwrap();
            let report = session.entity_helper_report(entity).unwrap();
            assert!(!report.spheres[0].displayed);
            assert_eq!(
                report.spheres[0].suppression_reason,
                Some("unavailable_radius")
            );
            assert_eq!(
                report.spheres[0].state,
                if raw == "65537" {
                    "invalid"
                } else {
                    "nonpositive"
                }
            );
            assert!(sphere_vertices(&session.build_scene()).is_empty());
        }
        session
            .edit(actor, "import malformed radius", |tx| {
                tx.entity_mut(entity)?.properties.set("radius", "broken");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            session.entity_helper_report(entity).unwrap().spheres[0].state,
            "invalid"
        );
        session.undo(actor).unwrap();
        session
            .set_entity_properties(actor, entity, &[("radius".into(), Some("32".into()))])
            .unwrap();
        assert!(session.entity_helper_report(entity).unwrap().spheres[0].displayed);
        assert_eq!(sphere_vertices(&session.build_scene()).len(), 3 * 32 * 2);
    }
}

#[test]
fn sphere_scene_budget_and_specialized_precedence_are_reported_deterministically() {
    let helpers = (0..16)
        .map(|i| format!("sphere(r{i})"))
        .collect::<Vec<_>>()
        .join(" ");
    let properties = (0..16)
        .map(|i| format!("r{i}(float) : \"Radius\" : {}", i + 1))
        .collect::<Vec<_>>()
        .join(" ");
    let mut session = Session::new(Err("no assets needed".into()));
    session.set_definitions(Actor::Agent, Catalog::parse(&format!("@PointClass {helpers} = sample [{properties}] @PointClass sphere(radius) = light_spot [ radius(float) : \"Radius\" : 128 ] @PointClass sphere(radius) = move_rope [ radius(float) : \"Radius\" : 128 ]")).unwrap());
    let mut objects = Vec::new();
    for i in 0..9 {
        objects.push(
            session
                .create_entity(
                    Actor::Agent,
                    "sample",
                    Some(DVec3::new(i as f64 * 100.0, 0.0, 0.0)),
                    BTreeMap::new(),
                )
                .unwrap(),
        );
    }
    objects.reverse();
    session.select(&objects, SelectMode::Replace).unwrap();
    assert_eq!(
        sphere_vertices(&session.build_scene()).len(),
        spheres::MAX_DISPLAYED_SPHERES * 3 * 32 * 2
    );
    let ObjectId::Entity(first) = *objects.last().unwrap() else {
        unreachable!()
    };
    let ObjectId::Entity(last) = objects[0] else {
        unreachable!()
    };
    assert!(
        session
            .entity_helper_report(first)
            .unwrap()
            .spheres
            .iter()
            .all(|row| row.displayed)
    );
    let report = session.entity_helper_report(last).unwrap();
    assert!(report.spheres.iter().all(|row| row.state == "ready"
        && !row.displayed
        && row.suppression_reason == Some("scene_guide_limit")));
    assert!(report.status.contains("16 suppressed"));
    session
        .edit(Actor::Agent, "import unavailable radii", |tx| {
            let entity = tx.entity_mut(first)?;
            entity.properties.set("r0", "0");
            entity.properties.set("r1", "broken");
            Ok(())
        })
        .unwrap();
    let report = session.entity_helper_report(first).unwrap();
    assert_eq!(report.spheres[0].state, "nonpositive");
    assert_eq!(report.spheres[1].state, "invalid");
    let report = session.entity_helper_report(last).unwrap();
    assert!(report.spheres[..2].iter().all(|row| row.displayed));
    assert!(
        report.spheres[2..]
            .iter()
            .all(|row| row.suppression_reason == Some("scene_guide_limit"))
    );
    assert_eq!(
        sphere_vertices(&session.build_scene()).len(),
        spheres::MAX_DISPLAYED_SPHERES * 3 * 32 * 2
    );
    for (class, reason) in [
        ("light_spot", "specialized_light"),
        ("move_rope", "specialized_rope"),
    ] {
        let ObjectId::Entity(id) = session
            .create_entity(Actor::Agent, class, Some(DVec3::ZERO), BTreeMap::new())
            .unwrap()
        else {
            unreachable!()
        };
        let report = session.entity_helper_report(id).unwrap();
        assert_eq!(report.spheres[0].state, "ready");
        assert_eq!(report.spheres[0].suppression_reason, Some(reason));
        assert!(sphere_vertices(&session.build_scene()).is_empty());
    }
}

#[test]
fn sphere_model_helpers_remain_suppressed_when_assets_are_missing() {
    for (helper, declaration, authored) in [
        ("studio()", "", false),
        ("studioprop(\"models/missing.mdl\")", "", false),
        ("model()", "", false),
        (
            "",
            "model(studio) : \"Model\" : \"models/missing.mdl\"",
            false,
        ),
        ("", "", true),
    ] {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(Actor::Agent, Catalog::parse(&format!("@PointClass {helper} sphere(radius) = sample [ radius(float) : \"Radius\" : 128 screenspacefade(integer) : \"Screen fade\" : 1 {declaration} ]")).unwrap());
        let properties = if authored {
            BTreeMap::from([("model".into(), "models/missing.mdl".into())])
        } else {
            BTreeMap::new()
        };
        let ObjectId::Entity(id) = session
            .create_entity(Actor::Agent, "sample", Some(DVec3::ZERO), properties)
            .unwrap()
        else {
            unreachable!()
        };
        let report = session.entity_helper_report(id).unwrap();
        assert_eq!(report.spheres[0].state, "ready");
        assert_eq!(report.spheres[0].suppression_reason, Some("model_helper"));
        assert!(report.sphere_limits.contains("screen pixels"));
        assert!(sphere_vertices(&session.build_scene()).is_empty());
    }
}
