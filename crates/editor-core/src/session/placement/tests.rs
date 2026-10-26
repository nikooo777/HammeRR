use super::*;

use super::fixtures::fixture;

fn options() -> Options {
    Options {
        align: false,
        offset: 0.0,
        texture_lock: true,
        source_corner: None,
    }
}

#[test]
fn independent_grid_snap_shared_support_budget_rejects_the_next_root() {
    const HELPER_CORNERS: usize = 8;
    let mut session = Session::new(Err("no assets needed".into()));
    let mut ids = Vec::new();
    for origin in [DVec3::splat(32.0), DVec3::splat(64.0)] {
        ids.push(
            session
                .create_entity(Actor::User, "info_target", Some(origin), BTreeMap::new())
                .unwrap(),
        );
    }
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let mut context = session.placement_support_context(session.document());
    context.points_used = MAX_POINTS - HELPER_CORNERS;
    session
        .object_translation_bounds_prepared(&ids[..1], DVec3::X, Some(&mut context))
        .unwrap();
    assert_eq!(context.points_used, MAX_POINTS);
    let error = session
        .object_translation_bounds_prepared(&ids[1..], DVec3::Y, Some(&mut context))
        .unwrap_err();
    assert!(error.contains("one million finite support points"));
    assert_eq!(context.points_used, MAX_POINTS);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

fn corner_options(session: &Session, solid: u32) -> Options {
    let (_, brush) = subobject::solid(session.document(), solid).unwrap();
    let shape = subobject::topology(brush).unwrap();
    Options {
        source_corner: Some(Component {
            solid,
            sides: shape.vertices[0].sides.clone(),
        }),
        ..options()
    }
}

#[test]
fn corner_snap_moves_real_oblique_corners_and_whole_owners_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for (owned, texture_lock) in [(false, false), (false, true), (true, false), (true, true)] {
            let (mut session, selected) = fixture();
            let ObjectId::Solid(solid) = selected else {
                unreachable!()
            };
            let mut moving = selected;
            if owned {
                let extra = session
                    .create_block(
                        actor,
                        DVec3::new(20.0, 20.0, 40.0),
                        DVec3::new(28.0, 28.0, 48.0),
                        "brick/test",
                        false,
                    )
                    .unwrap();
                let ObjectId::Solid(extra) = extra else {
                    unreachable!()
                };
                moving = session
                    .edit(actor, "tie source children", |tx| {
                        map_core::edit::tie_to_entity(tx, &[solid, extra], "func_detail", &[])
                    })
                    .unwrap();
            }
            session
                .transform_objects(
                    actor,
                    &[moving],
                    Transform {
                        rotation: DVec3::new(0.0, 0.0, 17.0),
                        translation: DVec3::new(-3.25, 2.75, 0.3),
                        ..Transform::default()
                    },
                    None,
                )
                .unwrap();
            session
                .select(&[moving], super::super::SelectMode::Replace)
                .unwrap();
            let mut options = corner_options(&session, solid);
            options.texture_lock = texture_lock;
            let component = options.source_corner.clone().unwrap();
            let (_, brush) = subobject::solid(session.document(), solid).unwrap();
            let shape = subobject::topology(brush).unwrap();
            let source_position =
                shape.vertices[shape.resolve(&component.sides).unwrap()[0]].position;
            let originals = session
                .document()
                .vmf()
                .solids()
                .filter(|brush| session.document().owner_of_solid(brush.id) == Some(moving))
                .map(|brush| (brush.id, subobject::topology(brush).unwrap()))
                .collect::<Vec<_>>();
            let source = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let ray = Ray {
                origin: DVec3::new(-200.0, -200.0, 100.0),
                direction: DVec3::NEG_Z,
            };
            let report = session
                .preview_surface_placement(
                    actor,
                    &[moving],
                    Method::BrushCorner,
                    Some(ray),
                    options.clone(),
                )
                .unwrap();
            assert_eq!(
                report["source_corner"],
                super::super::components::component_json(&component)
            );
            assert!(report["plan"]["contact_offset"].is_null());
            let target = DVec3::from_array(
                serde_json::from_value(report["plan"]["corner"]["position"].clone()).unwrap(),
            );
            assert_eq!(target, DVec3::new(-256.0, -256.0, 0.0));
            let delta = target - source_position;
            for (id, original) in originals {
                let (_, brush) = subobject::solid(session.render_document(), id).unwrap();
                let moved = subobject::topology(brush).unwrap();
                for vertex in original.vertices {
                    let point = moved.vertices[moved.resolve(&vertex.sides).unwrap()[0]].position;
                    assert!(point.abs_diff_eq(vertex.position + delta, 1e-8));
                }
            }
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.document().undo_labels().count(), history);
            assert_eq!(
                session.transform_preview().unwrap().transform.texture_lock,
                texture_lock
            );
            for old in source
                .solids()
                .filter(|brush| session.document().owner_of_solid(brush.id) == Some(moving))
            {
                let (_, new) = subobject::solid(session.render_document(), old.id).unwrap();
                for (old, new) in old.sides.iter().zip(&new.sides) {
                    if texture_lock {
                        assert!(
                            (map_core::texel_uv(old, old.plane.0[0])
                                - map_core::texel_uv(new, new.plane.0[0]))
                            .length()
                                < 1e-8
                        );
                    } else {
                        assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
                    }
                }
            }
            session
                .apply_surface_placement(actor, report["token"].as_u64().unwrap())
                .unwrap();
            assert_eq!(session.document().undo_labels().count(), history + 1);
            let again = session
                .preview_surface_placement(
                    actor,
                    &[moving],
                    Method::BrushCorner,
                    Some(ray),
                    options,
                )
                .unwrap();
            assert_eq!(again["plan"]["translation"], json!([0.0, 0.0, 0.0]));
            session
                .apply_surface_placement(actor, again["token"].as_u64().unwrap())
                .unwrap();
            assert_eq!(session.document().undo_labels().count(), history + 1);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &source);
        }
    }
}

#[test]
fn corner_snap_rejections_preserve_owned_plan_and_pick_state() {
    let (mut session, selected) = fixture();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    let options = corner_options(&session, solid);
    let ray = Ray {
        origin: DVec3::new(-200.0, -200.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    let report = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(ray),
            options.clone(),
        )
        .unwrap();
    let mut edge = options.clone();
    edge.source_corner.as_mut().unwrap().sides.truncate(2);
    let mut repeated = options.clone();
    let source = repeated.source_corner.as_mut().unwrap();
    source.sides.push(source.sides[0]);
    let mut wrong = options.clone();
    wrong.source_corner =
        corner_options(&session, session.document().vmf().world.solids[0].id).source_corner;
    for invalid in [
        Options {
            align: true,
            ..options.clone()
        },
        Options {
            offset: 1.0,
            ..options.clone()
        },
        edge,
        repeated,
        wrong,
    ] {
        assert!(
            session
                .preview_surface_placement(
                    Actor::Agent,
                    &[selected],
                    Method::BrushCorner,
                    Some(ray),
                    invalid
                )
                .is_err()
        );
        assert_eq!(session.surface_placement(), report);
    }
    assert!(
        session
            .preview_surface_placement(
                Actor::Agent,
                &[selected],
                Method::BrushCorner,
                None,
                options.clone()
            )
            .is_err()
    );
    assert_eq!(session.surface_placement(), report);
    let pick = session
        .begin_surface_placement_pick(&[selected], options)
        .unwrap();
    assert_eq!(pick["method"], "brush_corner");
    let token = pick["token"].as_u64().unwrap();
    assert!(
        session
            .pick_surface_placement(
                Actor::Agent,
                token,
                Ray {
                    origin: DVec3::splat(1000.0),
                    direction: DVec3::Z
                }
            )
            .is_err()
    );
    assert!(session.surface_placement()["picking"].as_bool().unwrap());
    session
        .pick_surface_placement(Actor::User, token, ray)
        .unwrap();
    assert_eq!(
        session.surface_placement()["plan"]["method"],
        "brush_corner"
    );
}

#[test]
fn corner_snap_owner_overflow_rejects_before_replacing_a_valid_plan() {
    let (mut session, selected) = fixture();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    session
        .create_block(
            Actor::User,
            DVec3::new(65520.0, -8.0, -8.0),
            DVec3::new(65536.0, 8.0, 8.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .select(&[selected], super::super::SelectMode::Replace)
        .unwrap();
    let (_, brush) = subobject::solid(session.document(), solid).unwrap();
    let shape = subobject::topology(brush).unwrap();
    let vertex = shape
        .vertices
        .iter()
        .min_by(|a, b| a.position.x.total_cmp(&b.position.x))
        .unwrap();
    let options = Options {
        source_corner: Some(Component {
            solid,
            sides: vertex.sides.clone(),
        }),
        ..options()
    };
    let good = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(Ray {
                origin: DVec3::new(-200.0, -200.0, 100.0),
                direction: DVec3::NEG_Z,
            }),
            options.clone(),
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let error = session
        .preview_surface_placement(
            Actor::Agent,
            &[selected],
            Method::BrushCorner,
            Some(Ray {
                origin: DVec3::new(65540.0, 0.0, 0.0),
                direction: DVec3::NEG_X,
            }),
            options,
        )
        .unwrap_err();
    assert!(error.contains("outside ±65536"));
    assert_eq!(session.surface_placement(), good);
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn corner_snap_rejects_partial_owned_terrain_bounds_without_replacing_a_valid_plan() {
    for nonfinite in [false, true] {
        let (mut session, selected) = fixture();
        let ObjectId::Solid(solid) = selected else {
            unreachable!()
        };
        let (owner, source_solid, terrain_solid) = session
            .edit(Actor::User, "mixed owner terrain", |tx| {
                let source = map_core::edit::block_solid(
                    tx,
                    DVec3::new(32.0, 32.0, 32.0),
                    DVec3::new(48.0, 48.0, 48.0),
                    "brick/test",
                )?;
                let mut terrain = map_core::edit::block_solid(
                    tx,
                    DVec3::new(64.0, 32.0, 32.0),
                    DVec3::new(80.0, 48.0, 48.0),
                    "brick/test",
                )?;
                let geometry = BrushGeometry::from_solid(&terrain).unwrap();
                for (index, face) in geometry.faces.iter().take(2).enumerate() {
                    let mut info = source_formats::vmf::DispInfo {
                        power: 2,
                        start_position: face.vertices[0],
                        flags: None,
                        elevation: 0.0,
                        subdivided: false,
                        normals: None,
                        distances: None,
                        offsets: None,
                        offset_normals: None,
                        alphas: None,
                        triangle_tags: None,
                        allowed_verts: None,
                        extra_properties: Default::default(),
                        extra: Vec::new(),
                    };
                    if index == 1 {
                        if nonfinite {
                            info.elevation = f64::INFINITY;
                        } else {
                            info.offsets = Some(vec![vec![DVec3::ZERO; 4]; 5]);
                        }
                    }
                    terrain.sides[face.side].displacement = Some(info);
                }
                terrain.hidden = true;
                let ids = (source.id, terrain.id);
                let owner = map_core::edit::create_entity(
                    tx,
                    "func_detail",
                    None,
                    &[],
                    vec![source, terrain],
                )?;
                Ok((owner, ids.0, ids.1))
            })
            .unwrap();
        let (_, terrain) = subobject::solid(session.document(), terrain_solid).unwrap();
        assert!(query::solid_bounds(terrain).is_some());
        let invalid_options = corner_options(&session, source_solid);
        let valid_options = corner_options(&session, solid);
        let ray = Ray {
            origin: DVec3::new(-200.0, -200.0, 100.0),
            direction: DVec3::NEG_Z,
        };
        let good = session
            .preview_surface_placement(
                Actor::User,
                &[selected],
                Method::BrushCorner,
                Some(ray),
                valid_options,
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let preview = session.render_document().vmf().clone();
        let history = session.document().undo_labels().count();
        let error = session
            .preview_surface_placement(
                Actor::Agent,
                &[owner],
                Method::BrushCorner,
                Some(ray),
                invalid_options,
            )
            .unwrap_err();
        assert!(error.contains("terrain face"), "{error}");
        assert_eq!(session.surface_placement(), good);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.selected_ids(), [selected]);
    }
}

#[test]
fn corner_snap_keeps_tiny_real_axis_moves_independent_of_other_axis_magnitude() {
    let mut session = Session::new(Err("No assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(0.0, -16.0, -16.0),
            DVec3::new(16.0, 16.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    let selected = session
        .create_block(
            Actor::User,
            DVec3::new(1e-12, 65500.0, 32.0),
            DVec3::new(16.0 + 1e-12, 65516.0, 48.0),
            "brick/test",
            false,
        )
        .unwrap();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    session
        .select(&[selected], super::super::SelectMode::Replace)
        .unwrap();
    let shape = session.component_topology(&[solid]).unwrap().remove(0);
    let vertex = shape
        .vertices
        .iter()
        .min_by(|a, b| a.position.x.total_cmp(&b.position.x))
        .unwrap();
    let options = Options {
        source_corner: Some(Component {
            solid,
            sides: vertex.sides.clone(),
        }),
        ..options()
    };
    let report = session
        .preview_surface_placement(
            Actor::User,
            &[selected],
            Method::BrushCorner,
            Some(Ray {
                origin: DVec3::new(0.5, -15.5, 100.0),
                direction: DVec3::NEG_Z,
            }),
            options,
        )
        .unwrap();
    let delta = report["plan"]["translation"][0].as_f64().unwrap();
    assert!(delta < -1e-14 && delta > -1e-11, "{delta}");
}

#[test]
fn corner_snap_target_queries_share_visibility_and_reject_ambiguous_hits() {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsnodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let game = std::sync::Arc::new(game);
    for mode in [
        "owned",
        "owner_hidden",
        "child_hidden",
        "tools",
        "outside",
        "terrain",
        "ambiguous",
    ] {
        let (mut session, selected) = fixture();
        session.game = game.clone();
        let ObjectId::Solid(source_solid) = selected else {
            unreachable!()
        };
        let owner = session
            .edit(Actor::User, "corner target", |tx| {
                let ObjectId::Solid(id) = map_core::edit::create_block(
                    tx,
                    DVec3::new(-200.0, -200.0, 16.0),
                    DVec3::new(-160.0, -160.0, 24.0),
                    if mode == "tools" {
                        "tools/toolsnodraw"
                    } else {
                        "brick/test"
                    },
                )?
                else {
                    unreachable!()
                };
                if mode == "terrain" {
                    let side = tx.document().solid(id).unwrap().sides[0].id;
                    map_core::displacement::edit::create(
                        tx,
                        &[map_core::surface::FaceId { solid: id, side }],
                        2,
                        0,
                    )?;
                }
                let owner = map_core::edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                Ok(owner)
            })
            .unwrap();
        let ObjectId::Entity(owner_id) = owner else {
            unreachable!()
        };
        if matches!(mode, "owner_hidden" | "child_hidden" | "ambiguous") {
            session
                .edit(Actor::User, "target visibility or identity", |tx| {
                    let entity = tx.entity_mut(owner_id)?;
                    match mode {
                        "owner_hidden" => entity.hidden = true,
                        "child_hidden" => entity.solids[0].hidden = true,
                        "ambiguous" => entity.solids[0].sides[1].id = entity.solids[0].sides[0].id,
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
        }
        if mode == "tools" {
            let mut view = session.view;
            view.tool_textures = false;
            session.set_view(view);
        }
        if mode == "outside" {
            let stamp = session.working_region_stamp();
            session
                .set_working_region(
                    Actor::User,
                    super::super::working_region::Action::Set {
                        bounds: [DVec3::splat(-32.0), DVec3::splat(64.0)],
                        enabled: true,
                    },
                    &stamp,
                )
                .unwrap();
        }
        let options = corner_options(&session, source_solid);
        let good = session
            .preview_surface_placement(
                Actor::User,
                &[selected],
                Method::BrushCorner,
                Some(Ray {
                    origin: DVec3::new(-240.0, -240.0, 100.0),
                    direction: DVec3::NEG_Z,
                }),
                options.clone(),
            )
            .unwrap();
        let result = session.preview_surface_placement(
            Actor::Agent,
            &[selected],
            Method::BrushCorner,
            Some(Ray {
                origin: DVec3::new(-180.0, -180.0, 100.0),
                direction: DVec3::NEG_Z,
            }),
            options,
        );
        if mode == "ambiguous" {
            assert!(result.is_err(), "{mode}");
            assert_eq!(session.surface_placement(), good);
        } else {
            let report = result.unwrap();
            if matches!(mode, "owned" | "terrain") {
                assert_eq!(report["plan"]["target"], object_id(owner));
                assert_eq!(
                    report["plan"]["corner"]["target"]["kind"],
                    if mode == "terrain" {
                        "terrain_vertex"
                    } else {
                        "brush_corner"
                    }
                );
            } else {
                assert_ne!(report["plan"]["target"], object_id(owner), "{mode}");
            }
        }
    }
}

#[test]
fn terrain_corner_snap_uses_the_prepared_mixed_power_vertex_and_atomic_history() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, selected, face, ray) = super::fixtures::terrain_target();
        let ObjectId::Solid(solid) = selected else {
            unreachable!()
        };
        let ids = super::fixtures::moving_roots(&mut session, selected);
        let options = corner_options(&session, solid);
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let prepared = map_core::displacement::surface::prepare(&source);
        let mesh = prepared.get(&face).unwrap();
        let index = mesh
            .positions
            .iter()
            .position(|point| *point == DVec3::new(128.0, 16.0, 17.0))
            .unwrap();
        let raw = session.terrain_grids(&[face], false).unwrap().remove(0);
        assert_eq!(raw.mesh.positions[index], DVec3::new(128.0, 16.0, 16.5));
        let hit = query::pick_surface(session.document(), &ray, |_| true).unwrap();
        let vertex = hit.terrain_vertex.unwrap();
        assert_eq!(
            (vertex.face, vertex.index, vertex.position),
            (face, index, mesh.positions[index])
        );
        assert!(
            map_core::displacement::triangles::indices(3).unwrap()[vertex.triangle]
                .contains(&index)
        );
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        let token = report["token"].as_u64().unwrap();
        assert_eq!(
            report["plan"]["target"],
            object_id(ObjectId::Solid(face.solid))
        );
        assert_eq!(
            report["plan"]["corner"]["position"],
            json!([128.0, 16.0, 17.0])
        );
        assert_eq!(
            report["plan"]["corner"]["target"],
            json!({"kind":"terrain_vertex", "solid":face.solid, "side":face.side, "triangle":vertex.triangle, "index":index, "grid_size":9, "row":index / 9, "column":index % 9, "provenance":"prepared_surface"})
        );
        let moved = subobject::topology(session.render_document().solid(solid).unwrap()).unwrap();
        let component = options.source_corner.as_ref().unwrap();
        assert!(
            moved.vertices[moved.resolve(&component.sides).unwrap()[0]]
                .position
                .abs_diff_eq(vertex.position, 1e-8)
        );
        assert_eq!(
            session.render_document().solid(face.solid),
            session.document().solid(face.solid)
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert!(
            session
                .preview_surface_placement(
                    actor,
                    &ids,
                    Method::BrushCorner,
                    Some(Ray {
                        origin: DVec3::splat(1000.0),
                        direction: DVec3::Z
                    }),
                    options.clone()
                )
                .is_err()
        );
        assert_eq!(session.surface_placement(), report);
        session.cancel_surface_placement(token).unwrap();
        assert_eq!(session.document().vmf(), &source);
        let report = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options.clone())
            .unwrap();
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        for terrain in source.world.solids.iter().filter(|brush| brush.id != solid) {
            assert_eq!(session.document().solid(terrain.id), Some(terrain));
        }
        let again = session
            .preview_surface_placement(actor, &ids, Method::BrushCorner, Some(ray), options)
            .unwrap();
        assert_eq!(again["plan"]["translation"], json!([0.0, 0.0, 0.0]));
        session
            .apply_surface_placement(actor, again["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}

#[test]
fn terrain_corner_snap_stale_source_and_asset_reload_reject_apply_but_allow_cancel() {
    let (mut session, selected, face, ray) = super::fixtures::terrain_target();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    for change_source in [true, false] {
        let options = corner_options(&session, solid);
        let report = session
            .preview_surface_placement(
                Actor::User,
                &[selected],
                Method::BrushCorner,
                Some(ray),
                options,
            )
            .unwrap();
        let token = report["token"].as_u64().unwrap();
        if change_source {
            session
                .edit(Actor::Agent, "change terrain target", |tx| {
                    tx.solid_mut(face.solid)?
                        .sides
                        .iter_mut()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .displacement
                        .as_mut()
                        .unwrap()
                        .distances
                        .as_mut()
                        .unwrap()[0][0] += 1.0;
                    Ok(())
                })
                .unwrap();
        } else {
            session.refresh_model_assets(Actor::Agent);
        }
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        assert!(session.apply_surface_placement(Actor::User, token).is_err());
        assert_eq!(session.surface_placement()["valid"], false);
        session.cancel_surface_placement(token).unwrap();
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }
}

#[test]
fn owned_brush_placement_uses_the_nearest_child_surface_and_keeps_atomic_history() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("Brush placement needs no assets".into()));
        session
            .create_block(
                actor,
                DVec3::new(-32.0, -32.0, -16.0),
                DVec3::new(32.0, 32.0, 0.0),
                "brick/test",
                false,
            )
            .unwrap();
        let (owner, solid) = session
            .edit(actor, "owned placement target", |tx| {
                let solid = map_core::edit::block_solid(
                    tx,
                    DVec3::new(-32.0, -32.0, 16.0),
                    DVec3::new(32.0, 32.0, 24.0),
                    "brick/test",
                )?;
                let id = solid.id;
                let owner =
                    map_core::edit::create_entity(tx, "func_detail", None, &[], vec![solid])?;
                Ok((owner, id))
            })
            .unwrap();
        let selected = session
            .create_block(
                actor,
                DVec3::new(-4.0, -4.0, 48.0),
                DVec3::new(4.0, 4.0, 56.0),
                "brick/test",
                false,
            )
            .unwrap();
        session
            .select(&[selected], super::super::SelectMode::Replace)
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let report = session
            .preview_surface_placement(actor, &[selected], Method::Floor, None, options())
            .unwrap();
        assert_eq!(report["plan"]["target"], object_id(owner));
        assert_eq!(report["plan"]["solid"], solid);
        assert_eq!(report["plan"]["point"], json!([0.0, 0.0, 24.0]));
        assert_eq!(report["plan"]["normal"], json!([0.0, 0.0, 1.0]));
        assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, -24.0]));
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .apply_surface_placement(actor, report["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(
            query::bounds(session.document(), selected).unwrap().0.z,
            24.0
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &source);
    }
}

#[test]
fn surface_placement_floor_ceiling_alignment_and_undo_share_contact_math() {
    let (mut session, id) = fixture();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let camera = session.camera();
    let report = session
        .preview_surface_placement(Actor::Agent, &[id], Method::Floor, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, -32.0]));
    assert_eq!(report["plan"]["normal"], json!([0.0, 0.0, 1.0]));
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.camera(), camera);
    let token = report["token"].as_u64().unwrap();
    session.apply_surface_placement(Actor::User, token).unwrap();
    assert_eq!(session.document().undo_labels().count(), history + 1);
    assert!(query::bounds(session.document(), id).unwrap().0.z.abs() < 1e-8);
    let history = session.document().undo_labels().count();
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Floor, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, 0.0]));
    session
        .apply_surface_placement(Actor::User, report["token"].as_u64().unwrap())
        .unwrap();
    assert_eq!(session.document().undo_labels().count(), history);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Ceiling, None, options())
        .unwrap();
    assert_eq!(report["plan"]["translation"], json!([0.0, 0.0, 80.0]));
    session
        .cancel_surface_placement(report["token"].as_u64().unwrap())
        .unwrap();
    let report = session
        .preview_surface_placement(
            Actor::Agent,
            &[id],
            Method::Ray,
            Some(Ray {
                origin: DVec3::new(0.0, 0.0, 64.0),
                direction: DVec3::Y,
            }),
            Options {
                align: true,
                offset: 4.0,
                ..options()
            },
        )
        .unwrap();
    assert!((report["plan"]["contact_offset"].as_f64().unwrap() - 4.0).abs() < 1e-8);
    let (_, high) = query::bounds(session.render_document(), id).unwrap();
    assert!((high.y - 60.0).abs() < 1e-8);
    session
        .apply_surface_placement(Actor::Agent, report["token"].as_u64().unwrap())
        .unwrap();
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn surface_placement_misses_and_foreign_transforms_preserve_owned_state() {
    let (mut session, id) = fixture();
    let source = session.document().vmf().clone();
    let report = session
        .preview_surface_placement(Actor::User, &[id], Method::Floor, None, options())
        .unwrap();
    let token = report["token"].as_u64().unwrap();
    let miss = Ray {
        origin: DVec3::splat(1000.0),
        direction: DVec3::Z,
    };
    assert!(
        session
            .preview_surface_placement(Actor::Agent, &[id], Method::Ray, Some(miss), options())
            .is_err()
    );
    assert_eq!(session.surface_placement()["token"], token);
    let transform = session.transform_preview().unwrap().transform;
    session
        .preview_transform(Actor::Agent, &[id], transform, Some(transform.pivot))
        .unwrap();
    assert!(session.surface_transform_owner().is_none());
    let foreign = session.render_document().vmf().clone();
    assert!(
        session
            .apply_surface_placement(Actor::Agent, token)
            .is_err()
    );
    session.cancel_surface_placement(token).unwrap();
    assert_eq!(session.render_document().vmf(), &foreign);
    session.cancel_transform_preview();
    let pick = session
        .begin_surface_placement_pick(&[id], options())
        .unwrap();
    let token = pick["token"].as_u64().unwrap();
    assert!(
        session
            .pick_surface_placement(Actor::User, token, miss)
            .is_err()
    );
    assert_eq!(session.surface_placement()["picking"], true);
    assert!(session.cancel_surface_placement(token + 1).is_err());
    session
        .pick_surface_placement(
            Actor::Agent,
            token,
            Ray {
                origin: DVec3::new(0.0, 0.0, 100.0),
                direction: DVec3::NEG_Z,
            },
        )
        .unwrap();
    assert_eq!(session.surface_placement()["picking"], false);
    session
        .create_entity(
            Actor::Agent,
            "info_target",
            Some(DVec3::new(100.0, 100.0, 16.0)),
            BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(session.surface_placement()["valid"], false);
    assert!(
        session
            .apply_surface_placement(Actor::Agent, token)
            .is_err()
    );
    session.cancel_surface_placement(token).unwrap();
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &source);
}

#[test]
fn surface_placement_keeps_point_helpers_axis_aligned_on_slopes() {
    let (mut session, _) = fixture();
    let floor = ObjectId::Solid(session.document().vmf().world.solids[0].id);
    session
        .transform_objects(
            Actor::Agent,
            &[floor],
            Transform {
                rotation: DVec3::new(0.0, 30.0, 0.0),
                ..Transform::default()
            },
            Some(DVec3::ZERO),
        )
        .unwrap();
    let helper = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::new(0.0, -128.0, 64.0)),
            BTreeMap::new(),
        )
        .unwrap();
    let report = session
        .preview_surface_placement(
            Actor::Agent,
            &[helper],
            Method::Floor,
            None,
            Options {
                align: true,
                offset: 3.0,
                ..options()
            },
        )
        .unwrap();
    let plan = report["plan"].as_object().unwrap();
    let vector = |key| DVec3::from_array(serde_json::from_value(plan[key].clone()).unwrap());
    let normal = vector("normal");
    let point = vector("point");
    let (low, high) = query::bounds(session.render_document(), helper).unwrap();
    let mut minimum = f64::INFINITY;
    for x in [low.x, high.x] {
        for y in [low.y, high.y] {
            for z in [low.z, high.z] {
                minimum = minimum.min(normal.dot(DVec3::new(x, y, z) - point));
            }
        }
    }
    assert!((minimum - 3.0).abs() < 1e-7);
}
