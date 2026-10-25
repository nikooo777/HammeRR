use std::collections::BTreeMap;

use glam::DVec3;
use map_core::{ObjectId, query::Ray};

use crate::session::{Actor, SelectMode, Session, working_region};

fn node(session: &mut Session, class: &str, origin: DVec3, values: &[(&str, &str)]) -> ObjectId {
    session
        .create_entity(
            Actor::Agent,
            class,
            Some(origin),
            values
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
}

fn fixture() -> (Session, ObjectId, ObjectId) {
    let mut session = Session::new(Err("no mounted assets".into()));
    let target = node(
        &mut session,
        "keyframe_rope",
        DVec3::new(256.0, 0.0, 128.0),
        &[("targetname", "end")],
    );
    let source = node(
        &mut session,
        "move_rope",
        DVec3::new(0.0, 0.0, 128.0),
        &[
            ("targetname", "start"),
            ("NextKey", "end"),
            ("Slack", "64"),
            ("Width", "12"),
        ],
    );
    session.select(&[], SelectMode::Replace).unwrap();
    (session, source, target)
}

#[test]
fn rope_preview_settings_and_picking_share_geometry_without_map_or_history_changes() {
    let (mut session, source, _) = fixture();
    let before = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let geometry = session.rope_preview_geometry();
    let link = &geometry.links[0];
    let point = link.points[link.points.len() / 2];
    let ray = Ray {
        origin: point - DVec3::Y * 100.0,
        direction: DVec3::Y,
    };
    assert_eq!(session.rope_preview_state()["enabled"], false);
    assert!(session.pick(&ray).is_none());
    assert_eq!(
        session
            .pick_with_rope_preview(&ray, Some(true))
            .unwrap()
            .object,
        source
    );
    assert!(!session.view().rope_preview);
    let serial = session.scene_serial();
    session.set_view(render::ViewOptions {
        rope_preview: true,
        tool_textures: false,
        ..session.view()
    });
    assert_eq!(session.scene_serial(), serial);
    assert_eq!(session.pick(&ray).unwrap().object, source);
    assert!(session.pick_with_rope_preview(&ray, Some(false)).is_none());
    assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 1);
    session
        .set_selection_filters(
            Actor::Agent,
            crate::session::selection_filters::SelectionFilters {
                point_entities: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(session.pick_for_selection(&ray).is_none());
    assert_eq!(session.object_hit_candidates(&ray).unwrap()["total"], 0);
    assert_eq!(session.pick(&ray).unwrap().object, source);
    session
        .set_selection_filters(Actor::User, Default::default())
        .unwrap();
    session.select(&[source], SelectMode::Replace).unwrap();
    let mesh = session.build_scene();
    assert!(
        mesh.batches
            .iter()
            .any(|batch| batch.rope && batch.index_count > 0)
    );
    assert!(!mesh.rope_lines.is_empty());
    assert_eq!(mesh.stats.point_entities, 2);
    let endpoint = Ray {
        origin: DVec3::new(0.0, -100.0, 128.0),
        direction: DVec3::Y,
    };
    assert_eq!(session.pick(&endpoint).unwrap().object, source);
    session.set_view(render::ViewOptions {
        rope_preview: false,
        ..session.view()
    });
    assert!(session.pick(&ray).is_none());
    assert_eq!(session.pick(&endpoint).unwrap().object, source);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.selected_ids(), [source]);
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    session.select_faces(&[], SelectMode::Replace).unwrap();
    assert!(session.pick(&ray).is_none());
    session.select(&[source], SelectMode::Replace).unwrap();
    assert_eq!(session.pick(&ray).unwrap().object, source);
}

#[test]
fn rope_preview_diagnostics_follow_authored_edits_visibility_working_regions_and_undo() {
    let (mut session, _, target) = fixture();
    let ObjectId::Entity(target_id) = target else {
        unreachable!()
    };
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    session
        .set_entity_properties(
            Actor::Agent,
            target_id,
            &[("targetname".into(), Some("renamed".into()))],
        )
        .unwrap();
    let report = session.rope_preview_state();
    assert!(report["links"].as_array().unwrap().is_empty());
    assert!(
        report["summary"]
            .as_str()
            .unwrap()
            .contains("no authored target")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session.undo(Actor::Agent).unwrap();
    assert_eq!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    session
        .set_visibility(Actor::Agent, "hide", &[target])
        .unwrap();
    assert!(
        session.rope_preview_state()["summary"]
            .as_str()
            .unwrap()
            .contains("hidden")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session.undo(Actor::Agent).unwrap();
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [
                    DVec3::new(-16.0, -16.0, 100.0),
                    DVec3::new(100.0, 16.0, 160.0),
                ],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        session.rope_preview_state()["summary"]
            .as_str()
            .unwrap()
            .contains("working region")
    );
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        session.rope_preview_state()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rope_preview_reports_remain_compact_for_oversized_imported_identifiers_even_when_disabled() {
    let (mut session, source, target) = fixture();
    let (ObjectId::Entity(source), ObjectId::Entity(target)) = (source, target) else {
        unreachable!()
    };
    let oversized = "é".repeat(render::ropes::MAX_IDENTIFIER_BYTES * 32);
    session
        .set_entity_properties(
            Actor::Agent,
            source,
            &[("NextKey".into(), Some(oversized.clone()))],
        )
        .unwrap();
    session
        .set_entity_properties(
            Actor::Agent,
            target,
            &[("targetname".into(), Some(oversized.clone()))],
        )
        .unwrap();
    for enabled in [false, true] {
        let report = session.rope_preview_report(enabled);
        assert!(report["links"].as_array().unwrap().is_empty());
        assert_eq!(report["issues"].as_array().unwrap().len(), 2);
        assert!(report.to_string().len() < 4096);
        assert!(report["summary"].as_str().unwrap().len() < 512);
        assert!(report["issues"].as_array().unwrap().iter().all(|issue| {
            issue["message"]
                .as_str()
                .unwrap()
                .contains("identifier exceeds")
        }));
    }
    assert!(!session.view().rope_preview);
    assert_eq!(
        session
            .document()
            .entity(source)
            .unwrap()
            .properties
            .get("NextKey"),
        Some(oversized.as_str())
    );
    assert_eq!(
        session
            .document()
            .entity(target)
            .unwrap()
            .properties
            .get("targetname"),
        Some(oversized.as_str())
    );
}

#[test]
fn temporary_editor_visibility_removes_rope_spans_and_keeps_authored_nodes_available() {
    let (mut session, source, target) = fixture();
    session.set_view(render::ViewOptions {
        rope_preview: true,
        ..session.view()
    });
    let geometry = session.rope_preview_geometry();
    let point = geometry.links[0].points[geometry.links[0].points.len() / 2];
    let ray = Ray {
        origin: point - DVec3::Y * 100.0,
        direction: DVec3::Y,
    };
    assert_eq!(session.pick(&ray).unwrap().object, source);
    let before = session.document().vmf().clone();
    session
        .set_editor_visibility(
            Actor::Agent,
            crate::session::editor_visibility::Action::Hide(vec![target]),
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(session.rope_preview_geometry().links.is_empty());
    assert!(session.pick(&ray).is_none());
    assert!(!session.build_scene().batches.iter().any(|batch| batch.rope));
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::ShowAll,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(session.pick(&ray).unwrap().object, source);
    assert_eq!(session.document().vmf(), &before);
}

#[test]
fn rope_presentation_bounds_frame_sag_and_width_without_changing_authored_spatial_operations() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, source, target) = fixture();
        let ObjectId::Entity(source_id) = source else {
            unreachable!()
        };
        let authored = session
            .object_bounds_in(session.document(), &[source])
            .unwrap();
        session
            .set_entity_properties(
                actor,
                source_id,
                &[
                    ("Slack".into(), Some("0".into())),
                    ("Width".into(), Some("32".into())),
                ],
            )
            .unwrap();
        let document = session.document().vmf().clone();
        let history = session.history_snapshot();
        session.select(&[source], SelectMode::Replace).unwrap();
        assert_eq!(
            session.presentation_object_bounds_in(session.document(), &[source], None),
            Some(authored)
        );
        session.set_view(render::ViewOptions {
            rope_preview: true,
            ..session.view()
        });
        let straight = (
            DVec3::new(-16.0, -16.0, 112.0),
            DVec3::new(272.0, 16.0, 144.0),
        );
        assert_eq!(
            session.presentation_object_bounds_in(session.document(), &[source], None),
            Some(straight)
        );
        assert_eq!(
            session.framing_bounds_in(session.document(), &[source], None),
            Some(straight)
        );
        assert_eq!(session.volume_selection_bounds(), [straight.0, straight.1]);
        assert_eq!(
            session.presentation_object_bounds_in(session.document(), &[target], None),
            session.object_bounds_in(session.document(), &[target])
        );
        assert_eq!(
            session.object_bounds_in(session.document(), &[source]),
            Some(authored)
        );
        assert_eq!(
            session.object_bounds_in_basis(session.document(), &[source], glam::DMat3::IDENTITY),
            Some(authored)
        );
        assert_eq!(
            session.selection_pivot(&[source]).unwrap(),
            DVec3::new(0.0, 0.0, 128.0)
        );
        let snap = session.preview_snap_to_grid(actor, &[source]).unwrap();
        assert_eq!(snap.bounds_minimum, authored.0.to_array());
        session.cancel_transform_preview();
        for projection in [
            render::Projection::Perspective,
            render::Projection::Top,
            render::Projection::Front,
            render::Projection::Side,
        ] {
            session.set_projection(actor, projection);
            let expected =
                session
                    .camera()
                    .frame_bounds(straight.0.as_vec3(), straight.1.as_vec3(), 0.5);
            session.frame_selection(actor, 0.5).unwrap();
            assert_eq!(session.camera(), expected);
        }
        assert_eq!(session.document().vmf(), &document);
        assert_eq!(session.history_snapshot(), history);
        session
            .set_entity_properties(
                actor,
                source_id,
                &[
                    ("Slack".into(), Some("64".into())),
                    ("Width".into(), Some("1".into())),
                ],
            )
            .unwrap();
        let sag = session
            .presentation_object_bounds_in(session.document(), &[source], None)
            .unwrap();
        assert_eq!(sag.0.x, authored.0.x);
        assert_eq!(sag.1.z, authored.1.z);
        assert!(sag.0.z < authored.0.z - 32.0);
        let preview = session.rope_preview_geometry();
        assert!(
            preview.links[0]
                .triangles()
                .flatten()
                .all(|point| point.cmpge(sag.0).all() && point.cmple(sag.1).all())
        );
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        session
            .preview_transform(
                actor,
                &[source],
                map_core::edit::Transform {
                    translation: DVec3::Z * 128.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let presented = session
            .framing_bounds_in(session.render_document(), &[source], None)
            .unwrap();
        assert!(presented.1.z >= 264.0);
        assert_ne!(presented, sag);
        assert_eq!(
            session.presentation_object_bounds_in(session.document(), &[source], None),
            Some(sag)
        );
        session.cancel_transform_preview();
        assert_eq!(
            session.framing_bounds_in(session.document(), &[source], None),
            Some(sag)
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn rope_presentation_area_and_volume_use_source_owned_bounds_and_shared_selection_filters() {
    use crate::session::{Pointer, volume};
    use glam::DVec2;
    use map_core::region::{Match, Region};

    for actor in [Actor::User, Actor::Agent] {
        let (mut session, source, _) = fixture();
        session.set_view(render::ViewOptions {
            rope_preview: true,
            ..session.view()
        });
        let preview = session.rope_preview_geometry();
        let middle = preview.links[0].points[preview.links[0].points.len() / 2];
        let mins = middle - DVec3::splat(2.0);
        let maxs = middle + DVec3::splat(2.0);
        let region = Region::from_bounds(mins, maxs).unwrap();
        assert_eq!(
            session
                .select_region(&region, Match::Intersecting, false)
                .objects,
            [source]
        );
        assert!(
            session
                .select_region(&region, Match::Contained, false)
                .objects
                .is_empty()
        );
        let all = session
            .presentation_object_bounds_in(session.document(), &[source], None)
            .unwrap();
        let all = Region::from_bounds(all.0, all.1).unwrap();
        assert_eq!(
            session.select_region(&all, Match::Contained, false).objects,
            [source]
        );
        assert!(
            session
                .select_region(&region, Match::Intersecting, true)
                .faces
                .is_empty()
        );
        session
            .set_selection_filters(
                actor,
                crate::session::selection_filters::SelectionFilters {
                    point_entities: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            session
                .select_region(&region, Match::Intersecting, false)
                .objects
                .is_empty()
        );
        session
            .set_selection_filters(actor, Default::default())
            .unwrap();
        let original = session.document().vmf().clone();
        let history = session.history_snapshot();
        let token = session
            .preview_volume_selection(volume::Spec {
                mins,
                maxs,
                rule: Match::Intersecting,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [source]);
        assert_eq!(
            session
                .commit_volume_selection(actor, token)
                .unwrap()
                .objects,
            [source]
        );
        let camera = render::Camera::planar(render::Projection::Front, middle.as_vec3(), 64.0);
        let matches = session
            .preview_area_selection(
                actor,
                camera,
                Pointer {
                    position: DVec2::splat(40.0),
                    size: (100, 100),
                },
                DVec2::splat(60.0),
                Match::Intersecting,
                SelectMode::Replace,
            )
            .unwrap();
        assert_eq!(matches.objects, [source]);
        assert_eq!(session.commit_area_selection().unwrap().objects, [source]);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn rope_presentation_disabled_invalid_hidden_and_region_excluded_spans_keep_endpoint_bounds() {
    for excluded in [
        "disabled",
        "invalid",
        "cycle",
        "hidden",
        "editor_hidden",
        "region",
    ] {
        let (mut session, source, target) = fixture();
        let (ObjectId::Entity(source_id), ObjectId::Entity(target_id)) = (source, target) else {
            unreachable!()
        };
        let authored = session.object_bounds_in(session.document(), &[source]);
        session.set_view(render::ViewOptions {
            rope_preview: true,
            ..session.view()
        });
        let preview = session.rope_preview_geometry();
        let middle = preview.links[0].points[preview.links[0].points.len() / 2];
        match excluded {
            "disabled" => session.set_view(render::ViewOptions {
                rope_preview: false,
                ..session.view()
            }),
            "invalid" => session
                .set_entity_properties(
                    Actor::Agent,
                    source_id,
                    &[("NextKey".into(), Some("missing".into()))],
                )
                .unwrap(),
            "cycle" => session
                .set_entity_properties(
                    Actor::Agent,
                    target_id,
                    &[("NextKey".into(), Some("start".into()))],
                )
                .unwrap(),
            "hidden" => session
                .set_visibility(Actor::Agent, "hide", &[target])
                .unwrap(),
            "editor_hidden" => {
                session
                    .set_editor_visibility(
                        Actor::Agent,
                        crate::session::editor_visibility::Action::Hide(vec![target]),
                        &session.working_region_stamp(),
                    )
                    .unwrap();
            }
            "region" => {
                session
                    .set_working_region(
                        Actor::Agent,
                        working_region::Action::Set {
                            bounds: [middle - DVec3::splat(4.0), middle + DVec3::splat(4.0)],
                            enabled: true,
                        },
                        &session.working_region_stamp(),
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            session.presentation_object_bounds_in(session.document(), &[source], None),
            authored,
            "{excluded}"
        );
        assert_eq!(
            session.framing_bounds_in(session.document(), &[source], None),
            authored,
            "{excluded}"
        );
        let region = map_core::region::Region::from_bounds(
            middle - DVec3::splat(2.0),
            middle + DVec3::splat(2.0),
        )
        .unwrap();
        assert!(
            session
                .select_region(&region, map_core::region::Match::Intersecting, false)
                .objects
                .is_empty(),
            "{excluded}"
        );
    }
}

#[test]
fn rope_presentation_toggle_retires_area_and_volume_reviews_while_preserving_geometry_drafts() {
    use crate::session::{Pointer, volume};
    use glam::DVec2;
    use map_core::region::Match;

    for enabled in [false, true] {
        let (mut session, source, _) = fixture();
        session.set_view(render::ViewOptions {
            rope_preview: enabled,
            ..session.view()
        });
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = render::Camera::planar(
            render::Projection::Front,
            glam::Vec3::new(128.0, 0.0, 96.0),
            512.0,
        );
        session
            .preview_area_selection(
                Actor::User,
                camera,
                Pointer {
                    position: DVec2::splat(10.0),
                    size: (100, 100),
                },
                DVec2::splat(90.0),
                Match::Intersecting,
                SelectMode::Replace,
            )
            .unwrap();
        session.set_view(session.view());
        assert!(session.area_preview().is_some());
        session.set_view(render::ViewOptions {
            rope_preview: !enabled,
            ..session.view()
        });
        assert!(session.commit_area_selection().is_err());
        let token = session
            .preview_volume_selection(volume::Spec {
                mins: DVec3::splat(-512.0),
                maxs: DVec3::splat(512.0),
                rule: Match::Intersecting,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        session.set_view(session.view());
        assert_eq!(session.volume_preview().unwrap().token, token);
        session.set_view(render::ViewOptions {
            rope_preview: enabled,
            ..session.view()
        });
        assert!(
            session
                .commit_volume_selection(Actor::Agent, token)
                .is_err()
        );
        assert!(session.selected_ids().is_empty());
        session
            .preview_transform(
                Actor::User,
                &[source],
                map_core::edit::Transform {
                    translation: DVec3::X * 10.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let draft = session.render_document().vmf().clone();
        let selected = session.selected_ids();
        assert_eq!(selected, [source]);
        session.set_view(render::ViewOptions {
            rope_preview: !enabled,
            ..session.view()
        });
        assert_eq!(session.render_document().vmf(), &draft);
        assert!(session.transform_preview().is_some());
        assert_eq!(session.selected_ids(), selected);
        session.cancel_transform_preview();
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), selected);
    }
}
