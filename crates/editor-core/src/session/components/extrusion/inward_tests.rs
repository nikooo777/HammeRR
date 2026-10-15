use super::*;

#[test]
fn inward_shares_actors_cancel_saved_plans_apply_selection_and_exact_history() {
    for actor in [Actor::User, Actor::Agent] {
        for owned in [false, true] {
            for direction in 0..2 {
                let (mut session, id, flat) = super::tests::fixture(actor, owned, direction);
                let source = session.document().vmf().clone();
                let history = session.document().undo_labels().count();
                let token = session
                    .preview_partition_extrusion(flat, super::tests::spec(-16.125))
                    .unwrap();
                let report = session.component_report(&[id]).unwrap();
                assert_eq!(report["preview"]["extrusion"]["direction"], "inward");
                assert_eq!(report["texture_mode"], "floor_attached");
                assert!(report["preview"]["extrusion"]["pieces"][0]["solid"].is_null());
                let draft = session.render_document().vmf().clone();
                assert_eq!(session.document().vmf(), &source);
                assert_eq!(session.document().undo_labels().count(), history);
                assert!(
                    map_core::partition_metadata::Metadata::parse(&draft)
                        .unwrap()
                        .entries
                        .is_empty()
                );
                assert!(session.ensure_partitions_resolved().is_err());
                let fragments = &session
                    .geometry
                    .components
                    .preview
                    .as_ref()
                    .unwrap()
                    .extrusion
                    .as_ref()
                    .unwrap()
                    .plan
                    .fragments;
                let ids = fragments
                    .iter()
                    .map(|fragment| fragment.solid.id)
                    .collect::<BTreeSet<_>>();
                let faces = fragments
                    .iter()
                    .map(|fragment| fragment.solid.sides.len())
                    .sum::<usize>();
                let count = fragments.len();
                session
                    .frame_component_partitions(actor, token, 1.6)
                    .unwrap();
                session.cancel_component_move(token).unwrap();
                assert_eq!(session.render_document().vmf(), &source);
                assert_eq!(
                    session.saved_partition_report()["entries"][0]["valid"],
                    true
                );
                let side = session.component_selection().iter().next().unwrap().sides[0];
                let flat = session
                    .resume_component_partitions(&[(id, side)], &session.component_stamp())
                    .unwrap();
                let token = session
                    .preview_partition_extrusion(flat, super::tests::spec(-16.125))
                    .unwrap();
                assert_eq!(session.render_document().vmf(), &draft);
                assert_eq!(session.commit_component_edit(actor, token).unwrap(), count);
                assert_eq!(session.document().vmf(), &draft);
                assert_eq!(session.selected_faces().len(), faces);
                assert!(
                    session
                        .selected_faces()
                        .iter()
                        .all(|face| ids.contains(&face.solid))
                );
                assert_eq!(session.document().undo_labels().count(), history + 1);
                session.undo(actor).unwrap();
                assert_eq!(session.document().vmf(), &source);
                session.redo(actor).unwrap();
                assert_eq!(session.document().vmf(), &draft);
            }
        }
    }
}

#[test]
fn inward_rejected_updates_and_allocator_changes_preserve_the_reviewed_draft() {
    let (mut session, id, flat) = super::tests::fixture(Actor::Agent, false, 0);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let token = session
        .preview_partition_extrusion(flat, super::tests::spec(-16.0))
        .unwrap();
    let draft = session.render_document().vmf().clone();
    for height in [-128.0, -4097.0, -0.01, 0.0, f64::NAN] {
        assert!(
            session
                .preview_partition_extrusion(token, super::tests::spec(height))
                .is_err()
        );
        assert_eq!(session.partition_preview_token(), Some(token));
        assert_eq!(session.render_document().vmf(), &draft);
    }
    session
        .document
        .edit("reserve", |tx| tx.new_side_id())
        .unwrap();
    assert!(
        session
            .commit_component_edit(Actor::Agent, token)
            .unwrap_err()
            .contains("allocation")
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.render_document().vmf(), &draft);
    session
        .select_component_partitions(
            token,
            &[PartitionRef {
                solid: id,
                partition: 0,
            }],
            SelectMode::Replace,
        )
        .unwrap();
    assert_eq!(
        session.component_report(&[id]).unwrap()["preview"]["can_apply"],
        false
    );
    assert!(session.commit_component_edit(Actor::Agent, token).is_err());
    let updated = session
        .preview_partition_extrusion(token, super::tests::spec(-128.0))
        .unwrap();
    assert!(
        session
            .preview_partition_extrusion(token, super::tests::spec(-16.0))
            .is_err()
    );
    let flat = session.raise_component_partition(updated, 0.0).unwrap();
    assert_eq!(session.render_document().vmf(), &source);
    session.cancel_component_move(flat).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn inward_floor_picking_and_material_previews_follow_real_fragment_surfaces() {
    let (mut session, id, flat) = super::tests::fixture(Actor::Agent, false, 0);
    let reference = PartitionRef {
        solid: id,
        partition: 0,
    };
    session
        .select_component_partitions(flat, std::slice::from_ref(&reference), SelectMode::Replace)
        .unwrap();
    let flat = session
        .update_component_partition_material(
            flat,
            std::slice::from_ref(&reference),
            Some("floor/planned"),
        )
        .unwrap();
    let token = session
        .preview_partition_extrusion(flat, super::tests::spec(-16.0))
        .unwrap();
    assert!(session.partition_face_overlays().is_empty());
    let partition = session.checked_partition(token).unwrap().partitions[0].clone();
    let piece = &partition.loops[0];
    let center = piece
        .vertices
        .iter()
        .map(|index| partition.vertices[*index as usize].position)
        .sum::<DVec3>()
        / piece.vertices.len() as f64;
    let camera =
        render::Camera::looking_at((center + DVec3::Z * 200.0).as_vec3(), center.as_vec3());
    let picks = session
        .pick_component_partitions(
            token,
            camera,
            crate::session::Pointer {
                position: glam::DVec2::new(400.0, 300.0),
                size: (800, 600),
            },
        )
        .unwrap();
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0].partition, reference);
    assert!((picks[0].position.z - 48.0).abs() < 0.01);
    let report = session.component_report(&[id]).unwrap();
    assert!(
        report["preview"]["extrusion"]["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|fragment| fragment["faces"].as_array().unwrap())
            .any(|face| face["surface"] == "top" && face["material"] == "floor/planned")
    );
}

#[test]
fn inward_picking_uses_real_surviving_materials_and_guides_include_visible_floor_fragments() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsnodraw.vmt"),
        "UnlitGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let gameinfo = directory.path().join("gameinfo.txt");
    std::fs::write(
        &gameinfo,
        "GameInfo { FileSystem { SearchPaths { Game |gameinfo_path|. } } }",
    )
    .unwrap();
    let game =
        std::sync::Arc::new(source_formats::gamefs::GameFs::from_gameinfo(gameinfo).unwrap());
    for tool_source in [false, true] {
        let (mut session, id, flat) = super::tests::fixture(Actor::Agent, false, 0);
        session.game = game.clone();
        session.view.tool_textures = false;
        session.cancel_component_move(flat).unwrap();
        let side = session.component_selection().iter().next().unwrap().sides[0];
        if tool_source {
            session
                .edit(Actor::Agent, "tool source", |tx| {
                    map_core::edit::set_material(
                        tx,
                        &[map_core::edit::FaceTarget::Object(ObjectId::Solid(id))],
                        "tools/toolsnodraw",
                    )
                })
                .unwrap();
        }
        let flat = session
            .resume_component_partitions(&[(id, side)], &session.component_stamp())
            .unwrap();
        let flat = session
            .update_component_partition_material(
                flat,
                &[PartitionRef {
                    solid: id,
                    partition: 0,
                }],
                Some("brick/planned"),
            )
            .unwrap();
        let flat = session
            .update_component_partition_material(
                flat,
                &[PartitionRef {
                    solid: id,
                    partition: 1,
                }],
                Some(if tool_source {
                    "brick/planned"
                } else {
                    "tools/toolsnodraw"
                }),
            )
            .unwrap();
        session
            .select_component_partitions(
                flat,
                &[PartitionRef {
                    solid: id,
                    partition: 0,
                }],
                SelectMode::Replace,
            )
            .unwrap();
        let token = session
            .preview_partition_extrusion(
                flat,
                ExtrusionSpec {
                    height: -16.0,
                    wall_material: None,
                    texture_lock: false,
                },
            )
            .unwrap();
        let draft = session.checked_partition(token).unwrap().clone();
        let mut mesh = session.build_scene();
        mesh.lines.clear();
        session.append_partition_guides(&mut mesh);
        assert!(
            mesh.lines
                .iter()
                .any(|vertex| (vertex.position[2] - 48.0).abs() < 0.01)
        );
        for piece in &draft.partitions[0].loops {
            let center = piece
                .vertices
                .iter()
                .map(|index| draft.partitions[0].vertices[*index as usize].position)
                .sum::<DVec3>()
                / piece.vertices.len() as f64;
            let camera =
                render::Camera::looking_at((center + DVec3::Z * 200.0).as_vec3(), center.as_vec3());
            let picks = session
                .pick_component_partitions(
                    token,
                    camera,
                    crate::session::Pointer {
                        position: glam::DVec2::new(400.0, 300.0),
                        size: (800, 600),
                    },
                )
                .unwrap();
            assert_eq!(picks.len(), usize::from(piece.id == 0 || !tool_source));
            if let Some(hit) = picks.first() {
                assert_eq!(hit.partition.partition, piece.id);
                assert!((hit.position.z - if piece.id == 0 { 48.0 } else { 64.0 }).abs() < 0.01);
            }
        }
    }
}
