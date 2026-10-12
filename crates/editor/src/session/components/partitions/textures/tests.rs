use super::*;
use crate::session::components::partitions::tests::fixture;

fn texture() -> PartitionTexture {
    PartitionTexture {
        shift: [13.125, -27.5],
        scale: [-0.375, 0.625],
        rotation: 37.0,
    }
}

#[test]
fn piece_texture_drafts_share_scene_storage_cancel_replay_and_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for direction in 0..2 {
            let (mut session, solid, pairs) = fixture(actor, direction);
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let piece = PartitionRef {
                solid,
                partition: 0,
            };
            let token = session
                .update_component_partition_texture(
                    token,
                    std::slice::from_ref(&piece),
                    Some(texture()),
                )
                .unwrap();
            let report = session.component_report(&[solid]).unwrap();
            let loops = &report["preview"]["partition"]["partitions"][0]["loops"];
            assert_eq!(loops[0]["texture_plan"], texture_json(texture()));
            assert_eq!(loops[0]["texture_inherited"], false);
            assert_eq!(loops[1]["texture_inherited"], true);
            let draft = session.checked_partition(token).unwrap().clone();
            let part = &draft.partitions[0];
            let source = map_core::surface::side(
                &session.document,
                map_core::surface::FaceId {
                    solid,
                    side: part.source.side,
                },
            )
            .unwrap()
            .clone();
            let projection = texture().project(&source).unwrap();
            let mut planned = source.clone();
            planned.u_axis = projection.u;
            planned.v_axis = projection.v;
            let mesh = session.build_scene();
            let tops = mesh
                .vertices
                .iter()
                .filter(|vertex| vertex.normal[2] > 0.99)
                .collect::<Vec<_>>();
            assert_eq!(tops.len(), 8);
            for (index, vertices) in tops.as_chunks::<4>().0.iter().enumerate() {
                for vertex in vertices {
                    let point = glam::Vec3::from_array(vertex.position).as_dvec3();
                    let expected =
                        map_core::texel_uv(if index == 0 { &planned } else { &source }, point)
                            / 128.0;
                    assert!(
                        glam::Vec2::from_array(vertex.uv)
                            .as_dvec2()
                            .abs_diff_eq(expected, 1e-6)
                    );
                }
            }
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            session.cancel_component_move(token).unwrap();
            assert_eq!(session.document.vmf(), &before);
            let token = session
                .preview_component_partition(&pairs, &session.component_stamp())
                .unwrap();
            let token = session
                .update_component_partition_texture(token, &[piece], Some(texture()))
                .unwrap();
            let side = part.source.side;
            session.store_component_partition(actor, token).unwrap();
            let kept = session.document.vmf().clone();
            assert_eq!(
                session.saved_partition_report()["entries"][0]["top_textures"][0],
                texture_json(texture())
            );
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            assert_eq!(session.checked_partition(token).unwrap().textures.len(), 1);
            let token = session
                .raise_component_partition_with_texture(token, 16.125, Some(true))
                .unwrap();
            let raised = session.render_document().vmf().clone();
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &raised);
            assert!(
                session.saved_partition_report()["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &kept);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &raised);
            session.undo(actor).unwrap();
            let token = session
                .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                .unwrap();
            let token = session
                .preview_partition_extrusion(
                    token,
                    subobject::ExtrusionSpec {
                        height: 32.125,
                        wall_material: None,
                        texture_lock: true,
                    },
                )
                .unwrap();
            let expected = session.render_document().vmf().clone();
            session.commit_component_edit(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &expected);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &kept);
        }
    }
}

#[test]
fn texture_updates_reject_invalid_refs_values_stale_and_raised_drafts_atomically() {
    let (mut session, solid, pairs) = fixture(Actor::Agent, 0);
    let before = session.document.vmf().clone();
    let stale = session
        .preview_component_partition(&pairs, &session.component_stamp())
        .unwrap();
    let piece = PartitionRef {
        solid,
        partition: 0,
    };
    let token = session
        .update_component_partition_texture(stale, std::slice::from_ref(&piece), Some(texture()))
        .unwrap();
    let report = session.component_report(&[solid]).unwrap();
    for refs in [
        vec![],
        vec![piece.clone(), piece.clone()],
        vec![PartitionRef {
            solid,
            partition: 9,
        }],
        vec![PartitionRef {
            solid: 999,
            partition: 0,
        }],
        vec![piece.clone(); 33],
    ] {
        assert!(
            session
                .update_component_partition_texture(token, &refs, Some(texture()))
                .is_err()
        );
        assert_eq!(session.component_report(&[solid]).unwrap(), report);
    }
    for invalid in [
        PartitionTexture {
            scale: [0.0, 1.0],
            ..texture()
        },
        PartitionTexture {
            rotation: f64::NAN,
            ..texture()
        },
        PartitionTexture {
            shift: [f64::INFINITY, 0.0],
            ..texture()
        },
        PartitionTexture {
            scale: [1e-9, 1.0],
            ..texture()
        },
    ] {
        assert!(
            session
                .update_component_partition_texture(
                    token,
                    std::slice::from_ref(&piece),
                    Some(invalid)
                )
                .is_err()
        );
        assert_eq!(session.component_report(&[solid]).unwrap(), report);
    }
    assert!(
        session
            .update_component_partition_texture(stale, std::slice::from_ref(&piece), None)
            .is_err()
    );
    let token = session.raise_component_partition(token, 16.0).unwrap();
    assert!(
        session
            .update_component_partition_texture(token, std::slice::from_ref(&piece), None)
            .is_err()
    );
    let token = session.raise_component_partition(token, 0.0).unwrap();
    let token = session
        .update_component_partition_texture(token, &[piece], None)
        .unwrap();
    assert!(
        session
            .checked_partition(token)
            .unwrap()
            .textures
            .is_empty()
    );
    session.cancel_component_move(token).unwrap();
    assert_eq!(session.document.vmf(), &before);
}
