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
        .set_compiled_visibility_overlay(Actor::Agent, Some(token.clone()), Some(target))
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
            .set_compiled_visibility_overlay(Actor::User, Some(token.clone()), Some(target))
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
        .set_compiled_visibility_overlay(Actor::User, None, None)
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
