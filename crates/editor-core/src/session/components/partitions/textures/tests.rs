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

fn placement_fixture(
    actor: Actor,
    corners: u32,
    owned: bool,
) -> (tempfile::TempDir, Session, u32, u32, Vec<CreaseEdges>) {
    let (directory, game) = crate::test_assets::game();
    let folder = directory.path().join("materials/test");
    let mut image = std::fs::read(folder.join("checker.vtf")).unwrap();
    image[16..18].copy_from_slice(&128u16.to_le_bytes());
    image[18..20].copy_from_slice(&16u16.to_le_bytes());
    std::fs::write(folder.join("wide.vtf"), image).unwrap();
    std::fs::write(
        folder.join("wide.vmt"),
        "LightmappedGeneric { $basetexture test/wide %compilenodraw 1 }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let objects = session
        .create_primitive(
            actor,
            map_core::primitive::Spec {
                shape: map_core::primitive::Shape::Cylinder,
                segments: corners,
                mins: DVec3::new(-23.125, 5.25, 16.5),
                maxs: DVec3::new(81.875, 68.125, 80.25),
                material: "test/checker".into(),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let ObjectId::Solid(solid) = objects[0] else {
        unreachable!()
    };
    if owned {
        session
            .tie_to_entity(actor, &objects, "func_detail", Default::default())
            .unwrap();
    }
    let side = session
        .document
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    session
        .edit_faces(
            actor,
            &[map_core::surface::FaceId { solid, side }],
            map_core::surface::SurfaceEdit {
                lightmap_scale: Some(24),
                smoothing_groups: Some(5),
                ..Default::default()
            },
            false,
            false,
        )
        .unwrap();
    session
        .select_components(
            &[Component {
                solid,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let choices = session.component_partition_choices().unwrap();
    let choice = if corners == 4 { 2 } else { 0 };
    let pairs = vec![CreaseEdges {
        solid,
        edges: choices[0].1[choice].edges,
    }];
    (directory, session, solid, side, pairs)
}

fn projection_uv(projection: &map_core::surface::Projection, point: DVec3) -> glam::DVec2 {
    glam::DVec2::new(
        point.dot(projection.u.axis) / projection.u.scale + projection.u.offset,
        point.dot(projection.v.axis) / projection.v.scale + projection.v.offset,
    )
}

fn planned_bounds(
    session: &Session,
    draft: &Draft,
    target: &PartitionRef,
) -> (glam::DVec2, glam::DVec2) {
    let part = draft
        .partitions
        .iter()
        .find(|part| part.source.solid == target.solid)
        .unwrap();
    let piece = part
        .loops
        .iter()
        .find(|piece| piece.id == target.partition)
        .unwrap();
    let source = map_core::surface::side(
        &session.document,
        map_core::surface::FaceId {
            solid: target.solid,
            side: part.source.side,
        },
    )
    .unwrap();
    let projection = draft
        .effective_texture(&session.document, part, piece.id)
        .unwrap()
        .project(source)
        .unwrap();
    piece
        .vertices
        .iter()
        .map(|index| projection_uv(&projection, part.vertices[*index as usize].position))
        .fold(
            (
                glam::DVec2::splat(f64::INFINITY),
                glam::DVec2::splat(f64::NEG_INFINITY),
            ),
            |(lo, hi), uv| (lo.min(uv), hi.max(uv)),
        )
}

#[test]
fn piece_fit_and_justify_use_actual_loops_dimensions_and_preserve_exact_replay() {
    use map_core::surface::Justify;
    for actor in [Actor::User, Actor::Agent] {
        for corners in [3, 4, 5, 8] {
            for owned in [false, true] {
                let (_directory, mut session, solid, side, pairs) =
                    placement_fixture(actor, corners, owned);
                assert!(session.material_preview("test/wide").unwrap().tool);
                let original = session.document.vmf().clone();
                let history = session.document.undo_labels().count();
                let refs = [
                    PartitionRef {
                        solid,
                        partition: 0,
                    },
                    PartitionRef {
                        solid,
                        partition: 1,
                    },
                ];
                let token = session
                    .preview_component_partition(&pairs, &session.component_stamp())
                    .unwrap();
                let token = session
                    .update_component_partition_cut(token, [0.2, 0.8])
                    .unwrap();
                let token = session
                    .update_component_partition_material(token, &refs[1..], Some("test/wide"))
                    .unwrap();
                let token = session
                    .update_component_partition_texture(token, &refs, Some(texture()))
                    .unwrap();
                let before = session.checked_partition(token).unwrap().clone();
                let mut reversed = refs.clone();
                reversed.reverse();
                let token = session
                    .place_component_partition_texture(token, &reversed, true, [None, None])
                    .unwrap();
                let fitted = session.checked_partition(token).unwrap().clone();
                assert_eq!(fitted.partitions, before.partitions);
                assert_eq!(fitted.materials, before.materials);
                assert_eq!(fitted.selection, before.selection);
                for (target, size) in refs.iter().zip([[64.0, 32.0], [128.0, 16.0]]) {
                    let planned = fitted.textures[target];
                    assert!(planned.scale[0] < 0.0 && planned.scale[1] > 0.0);
                    assert_eq!(planned.rotation, texture().rotation);
                    let (lo, hi) = planned_bounds(&session, &fitted, target);
                    assert!(lo.abs_diff_eq(glam::DVec2::ZERO, 1e-7));
                    assert!(hi.abs_diff_eq(glam::DVec2::from_array(size), 1e-7));
                }
                let token = session
                    .update_component_partition_texture(token, &refs, Some(texture()))
                    .unwrap();
                let token = session
                    .place_component_partition_texture(
                        token,
                        &refs[..1],
                        false,
                        [Some(Justify::End), None],
                    )
                    .unwrap();
                let justified = session.checked_partition(token).unwrap();
                let planned = justified.textures[&refs[0]];
                assert_eq!(planned.scale, texture().scale);
                assert_eq!(planned.rotation, texture().rotation);
                assert_eq!(planned.shift[1], texture().shift[1]);
                assert_eq!(justified.textures[&refs[1]], texture());
                assert!((planned_bounds(&session, justified, &refs[0]).1.x - 64.0).abs() < 1e-7);
                for placement in [Justify::Start, Justify::Center, Justify::End] {
                    let token = session.partition_preview_token().unwrap();
                    session
                        .place_component_partition_texture(
                            token,
                            &refs,
                            false,
                            [Some(placement); 2],
                        )
                        .unwrap();
                    let draft = session
                        .checked_partition(session.partition_preview_token().unwrap())
                        .unwrap();
                    for (target, size) in refs.iter().zip([[64.0, 32.0], [128.0, 16.0]]) {
                        let (lo, hi) = planned_bounds(&session, draft, target);
                        let size = glam::DVec2::from_array(size);
                        match placement {
                            Justify::Start => assert!(lo.abs_diff_eq(glam::DVec2::ZERO, 1e-7)),
                            Justify::Center => {
                                assert!(((lo + hi) * 0.5).abs_diff_eq(size * 0.5, 1e-7))
                            }
                            Justify::End => assert!(hi.abs_diff_eq(size, 1e-7)),
                        }
                    }
                }
                let token = session.partition_preview_token().unwrap();
                let token = session
                    .place_component_partition_texture(token, &refs, true, [None, None])
                    .unwrap();
                assert_eq!(
                    session.checked_partition(token).unwrap().textures,
                    fitted.textures
                );
                assert_eq!(session.document.vmf(), &original);
                assert_eq!(session.document.undo_labels().count(), history);
                session.store_component_partition(actor, token).unwrap();
                let kept = session.document.vmf().clone();
                let token = session
                    .resume_component_partitions(&[(solid, side)], &session.component_stamp())
                    .unwrap();
                let draft = session.checked_partition(token).unwrap().clone();
                assert_eq!(draft.textures, fitted.textures);
                let token = session
                    .preview_partition_extrusion(
                        token,
                        subobject::ExtrusionSpec {
                            height: 32.125,
                            wall_material: Some("tools/toolsnodraw".into()),
                            texture_lock: true,
                        },
                    )
                    .unwrap();
                let plan = session
                    .checked_component_preview(token)
                    .unwrap()
                    .extrusion
                    .as_ref()
                    .unwrap();
                let source = map_core::surface::side(
                    &session.document,
                    map_core::surface::FaceId { solid, side },
                )
                .unwrap();
                for result in &plan.plan.pieces {
                    let piece = &draft.partitions[0].loops[result.partition as usize];
                    let top = &result.solid.sides[0];
                    let projection = fitted.textures[&refs[result.partition as usize]]
                        .project(source)
                        .unwrap();
                    for vertex in &piece.vertices {
                        let before = draft.partitions[0].vertices[*vertex as usize].position;
                        assert!(
                            map_core::texel_uv(top, before + DVec3::Z * 32.125)
                                .abs_diff_eq(projection_uv(&projection, before), 1e-7)
                        );
                    }
                    assert_eq!(
                        top.material,
                        if result.partition == 0 {
                            "TEST/CHECKER"
                        } else {
                            "TEST/WIDE"
                        }
                    );
                    assert_eq!(top.lightmap_scale, 24);
                    assert_eq!(top.smoothing_groups, Some(5));
                    assert_eq!(result.solid.sides[1].material, source.material);
                    assert!(
                        result.solid.sides[2..]
                            .iter()
                            .all(|side| side.material == "TOOLS/TOOLSNODRAW")
                    );
                }
                let expected = session.render_document().vmf().clone();
                session.commit_component_edit(actor, token).unwrap();
                assert_eq!(session.document.vmf(), &expected);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &kept);
                session.redo(actor).unwrap();
                assert_eq!(session.document.vmf(), &expected);
                session.undo(actor).unwrap();
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &original);
            }
        }
    }
}

#[test]
fn placement_rejects_missing_dimensions_invalid_scopes_and_stale_tokens_atomically() {
    let (_directory, mut session, solid, _, pairs) = placement_fixture(Actor::Agent, 4, false);
    let refs = [
        PartitionRef {
            solid,
            partition: 0,
        },
        PartitionRef {
            solid,
            partition: 1,
        },
    ];
    let token = session
        .preview_component_partition(&pairs, &session.component_stamp())
        .unwrap();
    let token = session
        .update_component_partition_texture(token, &refs, Some(texture()))
        .unwrap();
    let before = session.component_report(&[solid]).unwrap();
    for invalid in [
        vec![],
        vec![refs[0].clone(); 2],
        vec![PartitionRef {
            solid,
            partition: 9,
        }],
        vec![PartitionRef {
            solid: 999,
            partition: 0,
        }],
        vec![refs[0].clone(); 33],
    ] {
        assert!(
            session
                .place_component_partition_texture(token, &invalid, true, [None, None])
                .is_err()
        );
        assert_eq!(session.component_report(&[solid]).unwrap(), before);
    }
    assert!(
        session
            .place_component_partition_texture(token, &refs, false, [None, None])
            .is_err()
    );
    assert_eq!(session.component_report(&[solid]).unwrap(), before);
    let stale = token;
    let token = session
        .update_component_partition_material(token, &refs[1..], Some("missing/no_dimensions"))
        .unwrap();
    let before = session.component_report(&[solid]).unwrap();
    assert!(
        session
            .place_component_partition_texture(token, &refs, true, [None, None])
            .unwrap_err()
            .contains("dimensions unavailable")
    );
    assert_eq!(session.component_report(&[solid]).unwrap(), before);
    assert!(
        session
            .place_component_partition_texture(stale, &refs, true, [None, None])
            .is_err()
    );
    let token = session
        .update_component_partition_material(token, &refs[1..], None)
        .unwrap();
    let token = session
        .preview_partition_extrusion(
            token,
            subobject::ExtrusionSpec {
                height: 16.0,
                wall_material: None,
                texture_lock: true,
            },
        )
        .unwrap();
    let before = session.component_report(&[solid]).unwrap();
    assert!(
        session
            .place_component_partition_texture(token, &refs, true, [None, None])
            .is_err()
    );
    assert_eq!(session.component_report(&[solid]).unwrap(), before);
    session.cancel_component_move(token).unwrap();
    let (mut session, solid, pairs) = fixture(Actor::User, 0);
    let token = session
        .preview_component_partition(&pairs, &session.component_stamp())
        .unwrap();
    let refs = [PartitionRef {
        solid,
        partition: 0,
    }];
    assert!(
        session
            .place_component_partition_texture(token, &refs, true, [None, None])
            .is_err()
    );
    let token = session.raise_component_partition(token, 16.0).unwrap();
    let before = session.component_report(&[solid]).unwrap();
    assert!(
        session
            .place_component_partition_texture(
                token,
                &refs,
                false,
                [Some(map_core::surface::Justify::Center); 2]
            )
            .is_err()
    );
    assert_eq!(session.component_report(&[solid]).unwrap(), before);
}

#[test]
fn multi_source_fitting_is_order_independent_and_roofs_preserve_saved_metadata() {
    let (_directory, mut session, first, first_side, _) = placement_fixture(Actor::Agent, 4, true);
    let ObjectId::Solid(second) = session
        .create_block(
            Actor::Agent,
            DVec3::new(192.125, -17.25, 0.5),
            DVec3::new(288.875, 80.125, 64.25),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let second_side = session
        .document
        .solid(second)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    session
        .select_components(
            &[
                Component {
                    solid: first,
                    sides: vec![first_side],
                },
                Component {
                    solid: second,
                    sides: vec![second_side],
                },
            ],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let pairs = session
        .component_partition_choices()
        .unwrap()
        .into_iter()
        .map(|(component, choices)| CreaseEdges {
            solid: component.solid,
            edges: choices[0].edges,
        })
        .collect::<Vec<_>>();
    let token = session
        .preview_component_partition(&pairs, &session.component_stamp())
        .unwrap();
    let refs = [first, second]
        .into_iter()
        .flat_map(|solid| {
            [
                PartitionRef {
                    solid,
                    partition: 0,
                },
                PartitionRef {
                    solid,
                    partition: 1,
                },
            ]
        })
        .collect::<Vec<_>>();
    let token = session
        .update_component_partition_texture(token, &refs, Some(texture()))
        .unwrap();
    let mut reversed = refs.clone();
    reversed.reverse();
    let token = session
        .place_component_partition_texture(token, &reversed, true, [None, None])
        .unwrap();
    let fitted = session.checked_partition(token).unwrap().textures.clone();
    let token = session
        .place_component_partition_texture(token, &refs, true, [None, None])
        .unwrap();
    assert_eq!(session.checked_partition(token).unwrap().textures, fitted);
    session
        .store_component_partition(Actor::Agent, token)
        .unwrap();
    let mut metadata =
        map_core::partition_metadata::Metadata::parse(session.document.vmf()).unwrap();
    for intent in metadata.entries.values_mut() {
        intent.extra.push(source_formats::keyvalues::Node::string(
            "future_surface",
            "preserve",
        ));
    }
    session
        .edit(Actor::Agent, "future partition field", |tx| {
            metadata.write(tx)
        })
        .unwrap();
    let saved = session.document.vmf().clone();
    let token = session
        .resume_component_partitions(
            &[(first, first_side), (second, second_side)],
            &session.component_stamp(),
        )
        .unwrap();
    let token = session
        .place_component_partition_texture(
            token,
            &refs,
            false,
            [Some(map_core::surface::Justify::End), None],
        )
        .unwrap();
    session
        .store_component_partition(Actor::Agent, token)
        .unwrap();
    let metadata = map_core::partition_metadata::Metadata::parse(session.document.vmf()).unwrap();
    for intent in metadata.entries.values() {
        assert!(
            intent
                .extra
                .contains(&source_formats::keyvalues::Node::string(
                    "future_surface",
                    "preserve"
                ))
        );
    }
    let kept = session.document.vmf().clone();
    let token = session
        .resume_component_partitions(
            &[(first, first_side), (second, second_side)],
            &session.component_stamp(),
        )
        .unwrap();
    let draft = session.checked_partition(token).unwrap().clone();
    let token = session
        .raise_component_partition_with_texture(token, 16.125, Some(true))
        .unwrap();
    for part in &draft.partitions {
        let source = map_core::surface::side(
            &session.document,
            map_core::surface::FaceId {
                solid: part.source.solid,
                side: part.source.side,
            },
        )
        .unwrap();
        let positions = part.positions(16.125);
        let raised = session.render_document().solid(part.source.solid).unwrap();
        for piece in &part.loops {
            let roof = raised
                .sides
                .iter()
                .find(|side| {
                    let plane = map_core::Plane::from_points(side.plane.0).unwrap();
                    piece
                        .vertices
                        .iter()
                        .all(|vertex| plane.distance_to(positions[*vertex as usize]).abs() < 1e-6)
                })
                .unwrap();
            let projection = draft
                .effective_texture(&session.document, part, piece.id)
                .unwrap()
                .project(source)
                .unwrap();
            for vertex in &piece.vertices {
                assert!(
                    map_core::texel_uv(roof, positions[*vertex as usize]).abs_diff_eq(
                        projection_uv(&projection, part.vertices[*vertex as usize].position),
                        1e-7
                    )
                );
            }
            assert_eq!(roof.material, source.material);
            assert_eq!(roof.lightmap_scale, source.lightmap_scale);
            assert_eq!(roof.smoothing_groups, source.smoothing_groups);
        }
    }
    let expected = session.render_document().vmf().clone();
    session.commit_component_edit(Actor::Agent, token).unwrap();
    assert_eq!(session.document.vmf(), &expected);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &kept);
    let token = session
        .resume_component_partitions(
            &[(first, first_side), (second, second_side)],
            &session.component_stamp(),
        )
        .unwrap();
    let token = session
        .update_component_partition_texture(token, &refs[..1], Some(texture()))
        .unwrap();
    session.cancel_component_move(token).unwrap();
    assert_eq!(session.document.vmf(), &kept);
    assert_eq!(session.document.vmf().world.solids, saved.world.solids);
    assert_eq!(session.document.vmf().entities, saved.entities);
}
