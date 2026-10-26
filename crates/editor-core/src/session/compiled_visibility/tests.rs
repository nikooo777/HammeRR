use super::*;

fn leaf_target(session: &Session) -> Target {
    let metadata = session
        .compiled
        .as_ref()
        .unwrap()
        .visibility_inspection
        .as_ref()
        .unwrap();
    let leaf = metadata
        .leaves()
        .iter()
        .find(|leaf| leaf.cluster >= 0)
        .unwrap();
    Target::Leaf {
        ordinal: leaf.ordinal as usize,
    }
}

#[test]
fn owned_positions_and_camera_resolve_native_world_leaves_without_changing_drafts() {
    let (_directory, mut session, source) = test_support::compiled();
    let mut camera = session.camera();
    camera.projection = render::Projection::Perspective;
    camera.position = glam::Vec3::new(0.0, 0.0, 16.0);
    session.set_camera(camera);
    session
        .begin_property_edit(super::super::PropertyTarget::World)
        .unwrap();
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.selected_ids(),
        session.history_snapshot(),
        serde_json::to_value(session.property_draft()).unwrap(),
    );
    let token = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap()
        .build["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let query = Query {
        build_token: Some(token.clone()),
        target: Target::Position {
            source: PositionSource::Position {
                position: [0.0, 0.0, 16.0],
            },
        },
        ..Query::default()
    };
    let position = session.inspect_compiled_visibility(&query).unwrap();
    let point = position.position.as_ref().unwrap();
    assert_eq!(point.requested_position, [0.0, 0.0, 16.0]);
    assert_eq!(point.effective_position, [0.0, 0.0, 16.0]);
    assert!(point.retained_world_leaf && point.cluster >= 0);
    assert_eq!(
        position.target,
        Target::Leaf {
            ordinal: point.native_leaf
        }
    );
    let by_camera = Query {
        target: Target::Position {
            source: PositionSource::Camera {},
        },
        ..query.clone()
    };
    let sampled = session.inspect_compiled_visibility(&by_camera).unwrap();
    assert_eq!(sampled.target, position.target);
    assert_eq!(sampled.visibility.rows, position.visibility.rows);
    assert_eq!(
        serde_json::to_value(sampled.selection).unwrap(),
        serde_json::to_value(position.selection).unwrap()
    );
    assert_eq!(
        before,
        (
            session.document().vmf().clone(),
            session.camera(),
            session.selected_ids(),
            session.history_snapshot(),
            serde_json::to_value(session.property_draft()).unwrap()
        )
    );
    for position in [[128.0, 0.0, 16.0], [2048.0; 3]] {
        let report = session
            .inspect_compiled_visibility(&Query {
                target: Target::Position {
                    source: PositionSource::Position { position },
                },
                ..query.clone()
            })
            .unwrap();
        assert_eq!(
            report.position.as_ref().unwrap().classification,
            "solid_or_exterior"
        );
        assert_eq!(report.position.as_ref().unwrap().cluster, -1);
        assert!(report.visibility.total.is_none() && report.visibility.rows.is_empty());
    }
    let mut planar = camera;
    planar.projection = render::Projection::Top;
    session.set_camera(planar);
    let error = session.inspect_compiled_visibility(&by_camera).unwrap_err();
    assert!(error.contains("Perspective"));
    assert!(session.inspect_compiled_visibility(&query).is_ok());
    session.set_camera(camera);
    assert!(
        session
            .inspect_compiled_visibility(&Query {
                target: Target::Position {
                    source: PositionSource::Position {
                        position: [f64::MAX; 3]
                    }
                },
                ..query.clone()
            })
            .is_err()
    );
    session
        .create_block(
            Actor::User,
            DVec3::splat(-32.0),
            DVec3::splat(-16.0),
            "test/checker",
            false,
        )
        .unwrap();
    let historical = session.inspect_compiled_visibility(&query).unwrap();
    assert!(historical.historical && historical.position.is_some());
    assert!(historical.current_bounds().is_err());
    std::fs::write(source.with_extension("bsp"), b"changed output").unwrap();
    let unavailable = session.inspect_compiled_visibility(&query).unwrap();
    assert_eq!(unavailable.build["status"], "unavailable");
    assert!(unavailable.position.is_none() && unavailable.selection.is_none());
}

#[test]
fn exact_world_leaf_portal_pages_and_separate_native_visibility_rows() {
    let (_directory, session, source) = test_support::compiled();
    let report = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap();
    let token = report.build["token"].as_str().unwrap().to_owned();
    let bsp = Bsp::read(source.with_extension("bsp")).unwrap();
    let native = bsp.records::<Leaf>().unwrap();
    let mut offset = 0;
    let mut seen = Vec::new();
    loop {
        let page = session
            .inspect_compiled_visibility(&Query {
                build_token: Some(token.clone()),
                offset,
                limit: 2,
                ..Query::default()
            })
            .unwrap();
        for row in page.page.rows {
            let Row::Leaf(row) = row else {
                panic!("Summary must page world leaves");
            };
            assert_eq!(row.cluster, native[row.ordinal].cluster);
            assert_eq!(row.contents, native[row.ordinal].contents);
            assert_eq!(
                row.bounds,
                [
                    native[row.ordinal].mins.map(f64::from),
                    native[row.ordinal].maxs.map(f64::from)
                ]
            );
            seen.push(row.ordinal);
        }
        let Some(next) = page.page.next_offset else {
            break;
        };
        offset = next;
    }
    let metadata = session
        .compiled
        .as_ref()
        .unwrap()
        .visibility_inspection
        .as_ref()
        .unwrap();
    assert_eq!(
        seen,
        metadata
            .leaves()
            .iter()
            .map(|leaf| leaf.ordinal as usize)
            .collect::<Vec<_>>()
    );
    assert!(!seen.contains(&0));
    for kind in [Kind::Pvs, Kind::Pas] {
        let report = session
            .inspect_compiled_visibility(&Query {
                build_token: Some(token.clone()),
                target: leaf_target(&session),
                kind,
                ..Query::default()
            })
            .unwrap();
        let Some(Row::Leaf(leaf)) = report.selection else {
            panic!();
        };
        let visibility = bsp.visibility().unwrap().unwrap();
        let row = visibility
            .row(
                leaf.cluster as usize,
                if kind == Kind::Pvs {
                    VisibilityKind::Visible
                } else {
                    VisibilityKind::Audible
                },
            )
            .unwrap();
        let expected = (0..visibility.clusters())
            .filter(|&cluster| row[cluster / 8] & (1 << (cluster % 8)) != 0)
            .collect::<Vec<_>>();
        assert_eq!(report.visibility.rows, expected);
    }
    let portal = &metadata.portals()[0];
    let report = session
        .inspect_compiled_visibility(&Query {
            build_token: Some(token),
            target: Target::Portal {
                ordinal: portal.ordinal as usize,
            },
            ..Query::default()
        })
        .unwrap();
    assert_eq!(
        report.selected_vertices.unwrap(),
        portal
            .points()
            .iter()
            .map(|point| point.to_array())
            .collect::<Vec<_>>()
    );
    assert_eq!(report.page.rows.len(), 2);
    assert!(
        session
            .inspect_compiled_visibility(&Query {
                target: leaf_target(&session),
                ..Query::default()
            })
            .is_err()
    );
    assert!(
        session
            .inspect_compiled_visibility(&Query {
                build_token: Some("999999".into()),
                ..Query::default()
            })
            .is_err()
    );
}

#[test]
fn current_framing_and_single_guide_preserve_drafts_and_suppress_changed_output() {
    let (_directory, mut session, source) = test_support::compiled();
    session
        .begin_property_edit(super::super::PropertyTarget::World)
        .unwrap();
    let draft = session.property_draft().unwrap().token;
    session
        .append_property_change(
            draft,
            super::super::PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_visibility".into(),
            },
        )
        .unwrap();
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        serde_json::to_value(session.property_draft()).unwrap(),
    );
    let target = leaf_target(&session);
    let token = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap()
        .build["token"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .frame_compiled_visibility(Actor::Agent, token.clone(), target, 1.6)
        .unwrap();
    session
        .set_compiled_visibility_overlay(
            Actor::Agent,
            Some(token.clone()),
            Some(target),
            GuideMode::default(),
        )
        .unwrap();
    let mesh = session.build_scene();
    assert_eq!(
        mesh.lines
            .iter()
            .filter(|line| line.color == [70, 220, 255, 255])
            .count(),
        24
    );
    let portal = session
        .compiled
        .as_ref()
        .unwrap()
        .visibility_inspection
        .as_ref()
        .unwrap()
        .portals()[0]
        .ordinal as usize;
    session
        .set_compiled_visibility_overlay(
            Actor::Agent,
            Some(token.clone()),
            Some(Target::Portal { ordinal: portal }),
            GuideMode::default(),
        )
        .unwrap();
    let points = session
        .compiled
        .as_ref()
        .unwrap()
        .visibility_inspection
        .as_ref()
        .unwrap()
        .portal(portal)
        .unwrap()
        .points()
        .len();
    let mesh = session.build_scene();
    assert_eq!(
        mesh.lines
            .iter()
            .filter(|line| line.color == [70, 220, 255, 255])
            .count(),
        points * 2
    );
    assert_eq!(
        before,
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.selected_ids(),
            serde_json::to_value(session.property_draft()).unwrap()
        )
    );
    let camera = session.camera();
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let preview = session.render_document().vmf().clone();
    assert!(
        session
            .frame_compiled_visibility(Actor::User, token.clone(), target, 1.0)
            .is_err()
    );
    assert!(
        session
            .set_compiled_visibility_overlay(
                Actor::User,
                Some(token.clone()),
                Some(target),
                GuideMode::default()
            )
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &preview);
    session.cancel_block_preview();
    let serial = session.scene_serial();
    std::fs::write(source.with_extension("bsp"), b"changed output").unwrap();
    session.preflight_compiled_visibility();
    assert!(session.scene_serial() > serial);
    assert_eq!(session.compiled_visibility_overlay()["displayed"], false);
    assert_eq!(
        session
            .build_scene()
            .lines
            .iter()
            .filter(|line| line.color == [70, 220, 255, 255])
            .count(),
        0
    );
    let report = session
        .inspect_compiled_visibility(&Query {
            build_token: Some(token.clone()),
            target,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(report.build["status"], "unavailable");
    assert!(report.summary.is_none() && report.page.total.is_none() && report.page.rows.is_empty());
    assert!(
        session
            .frame_compiled_visibility(Actor::User, token, target, 1.0)
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    session
        .set_compiled_visibility_overlay(Actor::User, None, None, GuideMode::default())
        .unwrap();
    assert_eq!(session.compiled_visibility_overlay()["requested"], false);
}

#[test]
fn historical_inspection_cancellation_and_failed_rebuild_keep_owned_metadata() {
    let (_directory, mut session, source) = test_support::compiled();
    let token = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap()
        .build["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let metadata = Arc::clone(
        session
            .compiled
            .as_ref()
            .unwrap()
            .visibility_inspection
            .as_ref()
            .unwrap(),
    );
    let bytes = std::fs::read(source.with_extension("bsp")).unwrap();
    session
        .create_block(
            Actor::User,
            DVec3::splat(-32.0),
            DVec3::splat(-16.0),
            "test/checker",
            false,
        )
        .unwrap();
    let report = session
        .inspect_compiled_visibility(&Query {
            build_token: Some(token),
            ..Query::default()
        })
        .unwrap();
    assert!(report.historical && report.summary.is_some() && !report.page.rows.is_empty());
    let request = session.begin_compile(Actor::User, None).unwrap();
    let build_token = session.compile_token().unwrap();
    session.cancel_compile(Actor::User, build_token).unwrap();
    let result = request.run();
    session.finish_compile(Actor::User, &result);
    assert!(result.is_err());
    assert_eq!(session.compile_status()["job"]["status"], "cancelled");
    assert!(Arc::ptr_eq(
        &metadata,
        session
            .compiled
            .as_ref()
            .unwrap()
            .visibility_inspection
            .as_ref()
            .unwrap()
    ));
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), bytes);
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        session
            .inspect_compiled_visibility_controlled(&Query::default(), &control)
            .is_err()
    );
    assert!(
        Query {
            limit: MAX_PAGE_ROWS + 1,
            ..Query::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Query {
            offset: usize::MAX,
            ..Query::default()
        }
        .validate()
        .is_err()
    );
    session.compiled.as_mut().unwrap().visibility_inspection =
        Err("Visibility metadata exceeded its retained storage limit".into());
    let report = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap();
    assert_eq!(report.build["status"], "unavailable");
    assert!(report.summary.is_none() && report.page.total.is_none() && report.page.rows.is_empty());
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), bytes);
}

#[test]
fn destination_guides_join_complete_native_rows_independently_of_pages_and_preserve_ownership() {
    let (_directory, mut session, source) = test_support::compiled();
    session
        .begin_property_edit(super::super::PropertyTarget::World)
        .unwrap();
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.selected_ids(),
        session.history_snapshot(),
        serde_json::to_value(session.property_draft()).unwrap(),
    );
    let summary = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap();
    let token = summary.build["token"].as_str().unwrap().to_owned();
    let target = leaf_target(&session);
    let Target::Leaf { ordinal } = target else {
        panic!()
    };
    let bsp = Bsp::read(source.with_extension("bsp")).unwrap();
    let native = bsp.records::<Leaf>().unwrap();
    let cluster = native[ordinal].cluster as usize;
    let metadata = Arc::clone(
        session
            .compiled
            .as_ref()
            .unwrap()
            .visibility_inspection
            .as_ref()
            .unwrap(),
    );
    let visibility = bsp.visibility().unwrap().unwrap();
    for kind in [Kind::Pvs, Kind::Pas] {
        let row = visibility
            .row(
                cluster,
                if kind == Kind::Pvs {
                    VisibilityKind::Visible
                } else {
                    VisibilityKind::Audible
                },
            )
            .unwrap();
        let expected = metadata
            .leaves()
            .iter()
            .filter_map(|leaf| {
                let cluster = native[leaf.ordinal as usize].cluster;
                (cluster >= 0 && row[cluster as usize / 8] & (1 << (cluster as usize % 8)) != 0)
                    .then_some(leaf.ordinal as usize)
            })
            .collect::<Vec<_>>();
        let clusters = (0..visibility.clusters())
            .filter(|&cluster| row[cluster / 8] & (1 << (cluster % 8)) != 0)
            .count();
        let (page, prepared) = session
            .inspect_visibility_prepared(
                &Query {
                    build_token: Some(token.clone()),
                    target,
                    kind,
                    limit: 1,
                    visibility_offset: usize::from(clusters > 1),
                    ..Query::default()
                },
                &Control::default(),
                true,
            )
            .unwrap();
        assert_eq!(page.visibility.rows.len(), 1);
        assert!(expected.len() > page.visibility.rows.len());
        assert_eq!(page.visibility.total, Some(clusters));
        let prepared = prepared.unwrap();
        assert_eq!(
            prepared
                .boxes
                .iter()
                .map(|leaf| leaf.ordinal)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(prepared.clusters, clusters);
        for leaf in &prepared.boxes {
            assert_eq!(
                leaf.bounds,
                [
                    native[leaf.ordinal].mins.map(f64::from),
                    native[leaf.ordinal].maxs.map(f64::from)
                ]
            );
        }
        for source in [target, Target::Cluster { ordinal: cluster }] {
            let guide = session
                .set_compiled_visibility_overlay(
                    Actor::Agent,
                    Some(token.clone()),
                    Some(source),
                    GuideMode::Destinations { visibility: kind },
                )
                .unwrap();
            assert_eq!(guide["native_leaves"], json!(expected));
            assert_eq!(guide["destination_clusters"], clusters);
            assert_eq!(guide["leaf_count"], expected.len());
            assert_eq!(
                guide["line_vertices"],
                expected.len() * LINE_VERTICES_PER_BOX
            );
            assert_eq!(
                guide["mode"],
                json!(GuideMode::Destinations { visibility: kind })
            );
        }
        assert_eq!(
            session
                .build_scene()
                .lines
                .iter()
                .filter(|line| line.color == [70, 220, 255, 255])
                .count(),
            expected.len() * LINE_VERTICES_PER_BOX
        );
    }
    assert_eq!(
        before,
        (
            session.document().vmf().clone(),
            session.camera(),
            session.selected_ids(),
            session.history_snapshot(),
            serde_json::to_value(session.property_draft()).unwrap(),
        )
    );
    let previous = session.compiled_visibility_overlay();
    for (build_token, source) in [
        (None, None),
        (Some(token.clone()), None),
        (None, Some(target)),
        (Some("999999".into()), Some(target)),
        (Some(token.clone()), Some(Target::Summary {})),
        (
            Some(token.clone()),
            Some(Target::Portal {
                ordinal: metadata.portals()[0].ordinal as usize,
            }),
        ),
    ] {
        assert!(
            session
                .set_compiled_visibility_overlay(
                    Actor::Agent,
                    build_token,
                    source,
                    GuideMode::Destinations {
                        visibility: Kind::Pvs
                    }
                )
                .is_err()
        );
        assert_eq!(session.compiled_visibility_overlay(), previous);
    }
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    assert!(
        session
            .set_compiled_visibility_overlay_controlled(
                Actor::Agent,
                Some(token.clone()),
                Some(target),
                GuideMode::Destinations {
                    visibility: Kind::Pvs
                },
                &cancelled
            )
            .is_err()
    );
    assert_eq!(session.compiled_visibility_overlay(), previous);
    let solid = metadata
        .leaves()
        .iter()
        .find(|leaf| leaf.cluster < 0)
        .unwrap()
        .ordinal as usize;
    assert!(
        session
            .set_compiled_visibility_overlay(
                Actor::Agent,
                Some(token.clone()),
                Some(Target::Leaf { ordinal: solid }),
                GuideMode::Destinations {
                    visibility: Kind::Pvs
                }
            )
            .unwrap_err()
            .contains("no PVS/PAS source cluster")
    );
    assert_eq!(session.compiled_visibility_overlay(), previous);
    session
        .create_block(
            Actor::User,
            DVec3::splat(-32.0),
            DVec3::splat(-16.0),
            "test/checker",
            false,
        )
        .unwrap();
    session.preflight_compiled_visibility();
    let stale = session.compiled_visibility_overlay();
    assert_eq!(stale["displayed"], false);
    assert_eq!(stale["native_leaves"], previous["native_leaves"]);
    assert_eq!(
        session
            .build_scene()
            .lines
            .iter()
            .filter(|line| line.color == [70, 220, 255, 255])
            .count(),
        0
    );
    assert!(
        session
            .set_compiled_visibility_overlay(
                Actor::Agent,
                Some(token.clone()),
                Some(target),
                GuideMode::Destinations {
                    visibility: Kind::Pvs
                }
            )
            .is_err()
    );
    assert_eq!(session.compiled_visibility_overlay(), stale);
    let historical = session
        .inspect_compiled_visibility(&Query {
            build_token: Some(token.clone()),
            target,
            ..Query::default()
        })
        .unwrap();
    assert!(historical.historical && historical.visibility.total.is_some());
    std::fs::write(source.with_extension("bsp"), b"changed output").unwrap();
    let unavailable = session
        .inspect_compiled_visibility(&Query {
            build_token: Some(token),
            target,
            ..Query::default()
        })
        .unwrap();
    assert!(unavailable.page.total.is_none() && unavailable.visibility.total.is_none());
    session
        .set_compiled_visibility_overlay(Actor::Agent, None, None, GuideMode::default())
        .unwrap();
    assert_eq!(session.compiled_visibility_overlay()["requested"], false);
}

#[test]
fn complete_destination_bounds_refuse_over_cap_without_truncation_and_check_cancellation() {
    let world = (1..=MAX_GUIDE_LEAVES + 1)
        .map(|ordinal| compiler::visibility_inspection::WorldLeaf {
            ordinal: ordinal as u32,
            prepared_node: ordinal as u32,
            cluster: 0,
        })
        .collect::<Vec<_>>();
    let mut native = vec![
        Leaf {
            cluster: 0,
            mins: [-1; 3],
            maxs: [1; 3],
            ..Leaf::default()
        };
        MAX_GUIDE_LEAVES + 2
    ];
    native[0].cluster = -1;
    native[0].contents = source_formats::bsp::contents::SOLID;
    let error = destination_bounds(&[1], &world, &native, 0, 1, &Control::default()).unwrap_err();
    assert!(
        error.contains("513 native world leaves")
            && error.contains("cap is 512")
            && error.contains("previous guide is unchanged")
    );
    let guide = destination_bounds(
        &[1],
        &world[..MAX_GUIDE_LEAVES],
        &native,
        0,
        1,
        &Control::default(),
    )
    .unwrap();
    assert_eq!(guide.boxes.len(), MAX_GUIDE_LEAVES);
    assert_eq!(guide.boxes.last().unwrap().ordinal, MAX_GUIDE_LEAVES);
    assert_eq!(
        guide.retained_bytes,
        size_of::<DestinationGuide>() + guide.boxes.capacity() * size_of::<GuideBox>()
    );
    assert!(destination_storage(usize::MAX).is_err());
    assert!(destination_bounds(&[], &world, &native, 0, 1, &Control::default()).is_err());
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        destination_bounds(&[1], &world, &native, 0, 1, &control)
            .unwrap_err()
            .contains("cancelled")
    );
    let solid = [compiler::visibility_inspection::WorldLeaf {
        ordinal: 0,
        prepared_node: 0,
        cluster: -1,
    }];
    assert!(
        destination_bounds(&[1], &solid, &native, 0, 1, &Control::default())
            .unwrap()
            .boxes
            .is_empty()
    );
}
