use super::super::{Actor, ViewUpdate};
use super::*;
use glam::DVec3;
use map_core::{Document, edit};

fn game() -> (tempfile::TempDir, source_formats::gamefs::GameFs) {
    let (directory, game) = crate::test_assets::game();
    let path = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes.truncate(80);
    bytes.extend([255; 4]);
    std::fs::write(path, bytes).unwrap();
    (directory, game)
}

fn fixture() -> (tempfile::TempDir, Session, ObjectId) {
    let (directory, game) = game();
    let mut session = Session::new(Ok(game));
    let floor = session
        .create_block(
            Actor::User,
            DVec3::new(-32.0, -32.0, -8.0),
            DVec3::new(32.0, 32.0, 0.0),
            "test/checker",
            false,
        )
        .unwrap();
    (directory, session, floor)
}

fn add(
    session: &mut Session,
    class: &str,
    origin: Option<DVec3>,
    fields: &[(&str, &str)],
) -> ObjectId {
    let fields = fields
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    session
        .edit(Actor::User, "light fixture", |tx| {
            edit::create_entity(tx, class, origin, &fields, Vec::new())
        })
        .unwrap()
}

fn enable(session: &mut Session) {
    let view = session
        .resolve_view_update(ViewUpdate {
            lighting_preview: Some(true),
            ..Default::default()
        })
        .unwrap();
    session.set_view(view);
}

#[test]
fn lighting_channel_selects_point_spot_sun_and_ambient_without_changing_shared_temporary_state() {
    let (_directory, mut session, _) = fixture();
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[
            ("_light", "255 0 0 20"),
            ("_lightHDR", "0 0 255 400"),
            ("_lightscaleHDR", "2"),
        ],
    );
    add(
        &mut session,
        "light_spot",
        Some(DVec3::Z * 48.0),
        &[
            ("_light", "255 0 0 30 0 255 0 300"),
            ("_lightHDR", "-1 -1 -1 1"),
            ("_lightscaleHDR", "2"),
            ("pitch", "-90"),
        ],
    );
    add(
        &mut session,
        "light_environment",
        None,
        &[
            ("_light", "255 0 0 40"),
            ("_lightHDR", "0 0 255 500"),
            ("_lightscaleHDR", "2"),
            ("pitch", "-90"),
            ("_ambient", "255 0 0 10"),
            ("_ambientHDR", "0 255 0 60"),
            ("_AmbientScaleHDR", "3"),
        ],
    );
    enable(&mut session);
    let ldr = session.build_scene();
    assert_eq!(ldr.lighting.report.channel, render::LightingChannel::Ldr);
    for (light, red) in ldr.lighting.lights.iter().zip([20.0, 30.0, 40.0]) {
        assert_eq!(light.intensity_cap[..3], [red, 0.0, 0.0]);
    }
    let accepted = session.view();
    let view = session
        .resolve_view_update(ViewUpdate {
            lighting_channel: Some("hdr"),
            ..Default::default()
        })
        .unwrap();
    assert_ne!(
        session.render_scene_key(view),
        session.render_scene_key(accepted)
    );
    let view = ViewOptions {
        lighting_sky_ambient: true,
        ..view
    };
    let hdr = session.build_scene_for_view(view);
    assert_eq!(hdr.lighting.report.channel, render::LightingChannel::Hdr);
    assert_eq!(hdr.lighting.report.admitted_lights, 3);
    assert_eq!(
        hdr.lighting.report.admitted_receivers,
        ldr.lighting.report.admitted_receivers
    );
    assert_eq!(hdr.lighting.lights[0].intensity_cap[..3], [0.0, 0.0, 800.0]);
    assert_eq!(hdr.lighting.lights[1].intensity_cap[..3], [0.0, 600.0, 0.0]);
    assert_eq!(
        hdr.lighting.lights[2].intensity_cap[..3],
        [0.0, 0.0, 1000.0]
    );
    assert_eq!(hdr.lighting.lights[2].origin_radius, [0.0, 180.0, 0.0, 0.0]);
    assert_eq!(session.view(), accepted);
    session.set_view(view);
    let state = session.lighting_preview_state();
    assert_eq!(state["channel"], "hdr");
    assert_eq!(state["report"]["channel"], "hdr");
    let disabled = session
        .resolve_view_update(ViewUpdate {
            lighting_preview: Some(false),
            ..Default::default()
        })
        .unwrap();
    session.set_view(disabled);
    let state = session.lighting_preview_state();
    assert_eq!(state["channel"], "hdr");
    assert_eq!(state["report"]["channel"], "hdr");
    assert!(state["summary"].as_str().unwrap().contains("HDR"));
}

#[test]
fn malformed_selected_hdr_excludes_sources_without_substituting_valid_ldr() {
    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    for class in ["light", "light_spot", "light_environment"] {
        add(
            &mut session,
            class,
            Some(DVec3::Z * 32.0),
            &[
                ("_light", "255 0 0 20"),
                ("_lightHDR", "malformed"),
                ("pitch", "-90"),
            ],
        );
    }
    assert_eq!(session.build_scene().lighting.report.admitted_lights, 3);
    let hdr = session
        .resolve_view_update(ViewUpdate {
            lighting_channel: Some("hdr"),
            ..Default::default()
        })
        .unwrap();
    let invalid = session.build_scene_for_view(hdr);
    assert!(invalid.lighting.lights.is_empty());
    assert_eq!(invalid.lighting.report.admitted_lights, 0);
    assert_eq!(invalid.lighting.report.excluded_lights, 3);
    assert!(
        invalid
            .lighting
            .report
            .diagnostics
            .iter()
            .all(|row| row.entity.is_some() && row.reason.contains("_lightHDR"))
    );
    let sun = session.document().vmf().entities.last().unwrap().id;
    session
        .set_entity_properties(
            Actor::User,
            sun,
            &[
                ("_lightHDR".into(), Some("0 0 255 600".into())),
                ("_ambientHDR".into(), Some("malformed".into())),
            ],
        )
        .unwrap();
    assert_eq!(
        session
            .build_scene_for_view(hdr)
            .lighting
            .report
            .admitted_suns,
        1
    );
    let ambient = session.build_scene_for_view(ViewOptions {
        lighting_sky_ambient: true,
        ..hdr
    });
    assert_eq!(ambient.lighting.report.admitted_suns, 0);
    assert!(
        ambient
            .lighting
            .report
            .diagnostics
            .iter()
            .any(|row| row.entity == Some(sun) && row.reason.contains("_ambientHDR"))
    );
}

#[test]
fn unresolved_template_lights_have_explicit_diagnostics_without_hiding_valid_lamps() {
    let (_directory, mut session, _) = fixture();
    let valid = add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 128 32 10")],
    );
    let unresolved = [
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 48.0),
            &[("_light", "$Lamp")],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_lightHDR", "$HDR"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[("_light", "255 255 255 10"), ("_lightscaleHDR", "$Gain")],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_fifty_percent_distance", "$Half"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_zero_percent_distance", "$End"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_inner_cone", "$Inner"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_cone", "$Outer"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_exponent", "$Focus"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[("_light", "255 255 255 10"), ("pitch", "$Tilt")],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_constant_attn", "$Constant"),
            ],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_linear_attn", "$Linear"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_quadratic_attn", "$Quadratic"),
                ("_fifty_percent_distance", "64"),
            ],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[("_light", "255 255 255 10"), ("_distance", "$Radius")],
        ),
        add(
            &mut session,
            "light_spot",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_distance", "$UnusedRadius"),
                ("_fifty_percent_distance", "64"),
                ("pitch", "-90"),
            ],
        ),
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_hardfalloff", "$Hard"),
                ("_fifty_percent_distance", "64"),
            ],
        ),
    ];
    enable(&mut session);
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let report = session.build_scene().lighting.report;
    assert!(report.active);
    assert_eq!(report.admitted_lights, 1);
    assert_eq!(report.excluded_lights, unresolved.len());
    let ObjectId::Entity(valid) = valid else {
        unreachable!()
    };
    assert_eq!(report.admitted_entities, [valid]);
    for id in unresolved {
        let ObjectId::Entity(id) = id else {
            unreachable!()
        };
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.entity == Some(id)
                && diagnostic
                    .reason
                    .contains("Unresolved template light value")
        }));
    }
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn unresolved_environment_values_exclude_the_sun_with_sky_ambient_enabled_or_disabled() {
    for key in [
        "_light",
        "_lightHDR",
        "_ambient",
        "_ambientHDR",
        "_lightscaleHDR",
        "_AmbientScaleHDR",
        "SunSpreadAngle",
    ] {
        let (_directory, mut session, _) = fixture();
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 32.0),
            &[("_light", "255 128 32 10")],
        );
        let sun = add(
            &mut session,
            "light_environment",
            Some(DVec3::Z * 64.0),
            &[
                ("_light", "255 255 255 10"),
                ("_lightHDR", "-1 -1 -1 1"),
                ("_ambient", "255 255 255 5"),
                ("_ambientHDR", "-1 -1 -1 1"),
                ("pitch", "-90"),
            ],
        );
        let ObjectId::Entity(sun) = sun else {
            unreachable!()
        };
        session
            .set_entity_properties(Actor::User, sun, &[(key.into(), Some("$Color".into()))])
            .unwrap();
        enable(&mut session);
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        for sky_ambient in [false, true] {
            let view = session
                .resolve_view_update(ViewUpdate {
                    lighting_sky_ambient: Some(sky_ambient),
                    ..Default::default()
                })
                .unwrap();
            session.set_view(view);
            let report = session.build_scene().lighting.report;
            assert!(report.active);
            assert_eq!(report.admitted_lights, 1);
            assert_eq!(report.excluded_lights, 1);
            assert_eq!(report.admitted_suns, 0);
            assert_eq!(report.excluded_suns, 1);
            assert!(report.diagnostics.iter().any(|diagnostic| {
                diagnostic.entity == Some(sun)
                    && diagnostic
                        .reason
                        .contains(&format!("Unresolved template light value {key}=$Color"))
            }));
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
        }
    }
}

#[test]
fn authored_lighting_tracks_owned_transform_cancel_apply_and_undo_without_view_edits() {
    let (_directory, mut session, _) = fixture();
    let light = add(
        &mut session,
        "light",
        Some(DVec3::new(20.0, 0.0, 32.0)),
        &[("_light", "255 128 32 100")],
    );
    let off = session.build_scene();
    assert!(off.lighting.lights.is_empty());
    assert!(off.vertices.iter().all(|v| v.lighting_receiver == 0));
    enable(&mut session);
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let camera = session.camera();
    session
        .preview_transform(
            Actor::User,
            &[light],
            edit::Transform {
                translation: DVec3::X * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let preview = session.render_document().vmf().clone();
    let serial = session.scene_serial();
    let mesh = session.build_scene();
    assert_eq!(mesh.lighting.report.admitted_lights, 1);
    assert_eq!(mesh.lighting.report.admitted_receivers, 6);
    assert_eq!(
        mesh.lighting.lights[0].origin_radius[..3],
        [36.0, 0.0, 32.0]
    );
    for (quality, channel) in [("fast", "hdr"), ("detail", "ldr")] {
        let view = session
            .resolve_view_update(ViewUpdate {
                lighting_channel: Some(channel),
                lighting_quality: Some(quality),
                lighting_sky_ambient: Some(quality == "fast"),
                lighting_exposure_ev: Some(2.0),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        assert_eq!(session.scene_serial(), serial);
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(session.camera(), camera);
    }
    session.cancel_transform_preview();
    assert_eq!(
        session.build_scene().lighting.lights[0].origin_radius[0],
        20.0
    );
    session
        .preview_transform(
            Actor::User,
            &[light],
            edit::Transform {
                translation: DVec3::X * 16.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    session.commit_transform_preview(Actor::User).unwrap();
    assert_eq!(
        session.build_scene().lighting.lights[0].origin_radius[0],
        36.0
    );
    assert_eq!(session.document.undo_labels().count(), history + 1);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(
        session.build_scene().lighting.lights[0].origin_radius[0],
        20.0
    );
}

#[test]
fn authored_lighting_has_no_prefix_on_budget_failure_and_explains_zero_and_unsupported_inputs() {
    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    let empty = session.build_scene();
    assert!(empty.lighting.report.active);
    assert!(empty.lighting.report.summary().contains("black"));
    for _ in 0..=MAX_LIGHTS {
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 32.0),
            &[("_light", "255 255 255 10")],
        );
    }
    let overflow = session.build_scene();
    assert!(!overflow.lighting.report.active);
    assert!(overflow.lighting.lights.is_empty());
    assert!(overflow.lighting.report.admitted_entities.is_empty());
    assert_eq!(overflow.lighting.report.excluded_lights, MAX_LIGHTS + 1);
    assert_eq!(overflow.lighting.report.admitted_receivers, 0);
    assert!(overflow.vertices.iter().all(|v| v.lighting_receiver == 0));
    assert!(
        overflow
            .lighting
            .report
            .summary()
            .contains("33 supported lights")
    );
    session.undo(Actor::User).unwrap();
    assert_eq!(
        session.build_scene().lighting.report.admitted_lights,
        MAX_LIGHTS
    );
    for _ in 0..MAX_LIGHTS {
        session.undo(Actor::User).unwrap();
    }
    for (class, fields) in [
        ("light_environment", vec![]),
        ("light_dynamic", vec![]),
        ("light", vec![("_light", "NaN 0 0 10")]),
        (
            "light",
            vec![("_light", "255 0 0 10"), ("targetname", "switch")],
        ),
        ("light", vec![("_light", "255 0 0 10"), ("style", "1")]),
        ("light", vec![("_light", "255 0 0 10"), ("spawnflags", "1")]),
        ("light", vec![("_light", "255 0 0 10"), ("pattern", "abc")]),
        (
            "light_spot",
            vec![("_light", "255 0 0 10"), ("target", "missing")],
        ),
    ] {
        add(&mut session, class, Some(DVec3::Z * 32.0), &fields);
    }
    add(&mut session, "light", None, &[("_light", "255 0 0 10")]);
    let invalid = session.build_scene();
    assert_eq!(invalid.lighting.report.admitted_lights, 0);
    assert_eq!(invalid.lighting.report.excluded_lights, 9);
    assert_eq!(invalid.lighting.report.diagnostics.len(), 9);
    assert!(invalid.lighting.report.summary().contains("Partial"));
}

#[test]
fn authored_lighting_targets_visibility_and_summed_headroom_are_checked() {
    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    add(
        &mut session,
        "light_spot",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 10"), ("target", "aim")],
    );
    add(
        &mut session,
        "info_target",
        Some(DVec3::ZERO),
        &[("targetname", "aim")],
    );
    assert_eq!(session.build_scene().lighting.report.admitted_lights, 1);
    add(
        &mut session,
        "info_target",
        Some(DVec3::X),
        &[("targetname", "aim")],
    );
    assert_eq!(session.build_scene().lighting.report.excluded_lights, 1);
    session.undo(Actor::User).unwrap();
    session.undo(Actor::User).unwrap();
    session.undo(Actor::User).unwrap();
    for _ in 0..2 {
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 32.0),
            &[("_light", "255 255 255 600000000000")],
        );
    }
    let mesh = session.build_scene();
    assert!(!mesh.lighting.report.active);
    assert!(mesh.lighting.report.unavailable.unwrap().contains("Summed"));
    session.undo(Actor::User).unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_lights, 1);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            super::super::working_region::Action::Set {
                bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
                enabled: true,
            },
            &stamp,
        )
        .unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_lights, 0);
}

#[test]
fn optional_sky_ambient_uses_one_sun_slot_and_excludes_malformed_or_over_budget_sources() {
    let (_directory, mut session, _) = fixture();
    assert!(!session.view().lighting_sky_ambient);
    enable(&mut session);
    let ObjectId::Entity(sun) = add(
        &mut session,
        "light_environment",
        None,
        &[
            ("_light", "255 0 0 200"),
            ("pitch", "-90"),
            ("_ambient", "0 0 255 80"),
        ],
    ) else {
        panic!()
    };
    let direct = session.build_scene();
    assert_eq!(direct.lighting.lights[0].origin_radius, [0.0; 4]);
    let view = session
        .resolve_view_update(ViewUpdate {
            lighting_sky_ambient: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_ne!(
        session.render_scene_key(view),
        session.render_scene_key(session.view())
    );
    session.set_view(view);
    let ambient = session.build_scene();
    assert!(ambient.lighting.report.sky_ambient);
    assert_eq!(ambient.lighting.report.admitted_lights, 1);
    assert_eq!(
        ambient.lighting.lights[0].origin_radius,
        [0.0, 0.0, 80.0, 0.0]
    );
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 10")],
    );
    session
        .edit(Actor::User, "malformed ambient", |tx| {
            edit::set_properties(tx, sun, &[("_ambient".into(), Some("malformed".into()))])
        })
        .unwrap();
    let malformed = session.build_scene();
    assert!(malformed.lighting.report.active);
    assert_eq!(malformed.lighting.report.admitted_lights, 1);
    assert_eq!(malformed.lighting.report.admitted_suns, 0);
    assert_eq!(malformed.lighting.report.excluded_suns, 1);
    assert!(
        malformed
            .lighting
            .report
            .diagnostics
            .iter()
            .any(|row| row.reason.contains("LDR sky ambient"))
    );
    let mut direct_view = session.view();
    direct_view.lighting_sky_ambient = false;
    assert_eq!(
        session
            .build_scene_for_view(direct_view)
            .lighting
            .report
            .admitted_suns,
        1
    );
    session
        .edit(Actor::User, "separate channels", |tx| {
            edit::set_properties(
                tx,
                sun,
                &[
                    ("_light".into(), Some("255 0 0 600000000000".into())),
                    ("_ambient".into(), Some("0 0 255 600000000000".into())),
                ],
            )
        })
        .unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_suns, 1);
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 500000000000")],
    );
    let overflow = session.build_scene();
    assert!(!overflow.lighting.report.active);
    assert!(
        overflow
            .lighting
            .report
            .unavailable
            .unwrap()
            .contains("Summed")
    );
    assert!(overflow.lighting.lights.is_empty());
    assert_eq!(overflow.lighting.report.admitted_suns, 0);
}

#[test]
fn direct_sun_duplicates_visibility_and_shared_budgets_never_choose_a_prefix() {
    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    let sun = add(
        &mut session,
        "light_environment",
        Some(DVec3::Z * 32.0),
        &[
            ("_light", "255 0 0 600000000000"),
            ("pitch", "-90"),
            ("_ambient", "malformed ignored ambient"),
            ("_lightHDR", "malformed ignored HDR"),
        ],
    );
    let scene = session.build_scene();
    assert_eq!(scene.lighting.report.admitted_suns, 1);
    assert_eq!(
        scene.lighting.lights[0].coefficients_spot[3],
        render::lighting::SUN_KIND
    );
    assert!(
        glam::Vec3::from_slice(&scene.lighting.lights[0].direction_inner[..3])
            .distance(glam::Vec3::Z)
            < 1e-6
    );
    let ObjectId::Entity(sun_id) = sun else {
        panic!()
    };
    for raw in ["", "0 0", "invalid 0 0", "NaN 0 0", "inf 0 0", "20000 0 0"] {
        session
            .edit(Actor::User, "invalid sun origin", |tx| {
                edit::set_properties(tx, sun_id, &[("origin".into(), Some(raw.into()))])
            })
            .unwrap();
        let report = session.build_scene().lighting.report;
        assert_eq!(report.admitted_suns, 0, "{raw:?}");
        assert_eq!(report.excluded_suns, 1, "{raw:?}");
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.entity == Some(sun_id) && diagnostic.reason.contains("Sun origin")
        }));
        session.undo(Actor::User).unwrap();
        assert_eq!(session.build_scene().lighting.report.admitted_suns, 1);
    }
    let other = add(
        &mut session,
        "light_environment",
        Some(DVec3::ZERO),
        &[("_light", "255 0 0 10")],
    );
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 10")],
    );
    let duplicate = session.build_scene();
    assert_eq!(duplicate.lighting.report.admitted_lights, 1);
    assert_eq!(duplicate.lighting.report.excluded_suns, 2);
    for id in [sun, other] {
        let ObjectId::Entity(id) = id else { panic!() };
        assert!(
            duplicate
                .lighting
                .report
                .diagnostics
                .iter()
                .any(|row| row.entity == Some(id) && row.reason.contains("Conflicting"))
        );
    }
    let stamp = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            super::super::editor_visibility::Action::Hide(vec![other]),
            &stamp,
        )
        .unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_suns, 1);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            super::super::working_region::Action::Set {
                bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
                enabled: true,
            },
            &stamp,
        )
        .unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_suns, 0);

    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    add(
        &mut session,
        "light_environment",
        None,
        &[("_light", "255 255 255 600000000000"), ("pitch", "-90")],
    );
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 600000000000")],
    );
    let overflow = session.build_scene();
    assert!(
        overflow
            .lighting
            .report
            .unavailable
            .as_ref()
            .unwrap()
            .contains("Summed")
    );
    assert_eq!(overflow.lighting.report.admitted_suns, 0);
    assert_eq!(overflow.lighting.report.excluded_suns, 1);
    assert!(overflow.lighting.lights.is_empty());
    session.undo(Actor::User).unwrap();
    for _ in 0..MAX_LIGHTS - 1 {
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 32.0),
            &[("_light", "255 255 255 1")],
        );
    }
    assert_eq!(
        session.build_scene().lighting.report.admitted_lights,
        MAX_LIGHTS
    );
    assert_eq!(session.build_scene().lighting.report.admitted_suns, 1);
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 1")],
    );
    let overflow = session.build_scene();
    assert!(!overflow.lighting.report.active);
    assert!(overflow.lighting.lights.is_empty());
    assert!(overflow.lighting.report.admitted_entities.is_empty());
    assert_eq!(overflow.lighting.report.excluded_lights, MAX_LIGHTS + 1);
    assert_eq!(overflow.lighting.report.excluded_suns, 1);
}

#[test]
fn authored_lighting_receiver_policy_uses_the_same_cached_vmt_as_base_until_reload() {
    let (directory, mut session, _) = fixture();
    enable(&mut session);
    assert_eq!(session.build_scene().lighting.report.admitted_receivers, 6);
    let vmt = directory.path().join("materials/test/checker.vmt");
    std::fs::write(&vmt, "UnlitGeneric { $basetexture test/checker }").unwrap();
    assert_eq!(session.build_scene().lighting.report.admitted_receivers, 6);
    session.reload_material_assets(Actor::User);
    let mesh = session.build_scene();
    assert_eq!(mesh.lighting.report.admitted_receivers, 0);
    assert_eq!(mesh.lighting.report.excluded_receivers, 6);
    for extra in [
        "$translucent 1",
        "$alphatest 1",
        "$selfillum 1",
        "%tooltexture test/checker",
        "Proxies { AnimatedTexture { } }",
    ] {
        std::fs::write(
            &vmt,
            format!("LightmappedGeneric {{ $basetexture test/checker {extra} }}"),
        )
        .unwrap();
        session.reload_material_assets(Actor::User);
        assert_eq!(session.build_scene().lighting.report.excluded_receivers, 6);
    }
    let off = session.view();
    for surface in [
        SurfaceView::Flat,
        SurfaceView::Wireframe,
        SurfaceView::LightmapDensity,
        SurfaceView::SmoothingGroups,
    ] {
        let mesh = session.build_scene_for_view(ViewOptions { surface, ..off });
        assert!(!mesh.lighting.report.active);
        assert!(mesh.lighting.lights.is_empty());
        assert!(mesh.lighting.report.diagnostics.is_empty());
        assert!(mesh.vertices.iter().all(|v| v.lighting_receiver == 0));
    }
}

fn rendered(
    context: &render::GpuContext,
    renderer: &mut render::Renderer,
    mesh: &render::SceneMesh,
    materials: &render::MaterialLibrary,
    view: ViewOptions,
) -> Vec<u8> {
    let scene = renderer.upload(context, mesh, materials);
    renderer.render_to_rgba(
        context,
        &scene,
        &render::Camera::looking_at(glam::Vec3::new(0.0, 0.0, 96.0), glam::Vec3::ZERO),
        &view,
        65,
        65,
    )
}

fn displayed(contribution: DVec3, exposure: f64) -> [u8; 3] {
    (contribution * (2.0f64.powf(exposure) / 255.0))
        .to_array()
        .map(|linear| {
            let mapped = linear / (1.0 + linear);
            let srgb = if mapped <= 0.0031308 {
                mapped * 12.92
            } else {
                1.055 * mapped.powf(1.0 / 2.4) - 0.055
            };
            (srgb * 255.0).round() as u8
        })
}

fn oracle(origin: DVec3, fields: &[(&str, &str)], spot: bool) -> PointLight {
    let source = compiler::map::MapEntity {
        id: 10,
        classname: if spot { "light_spot" } else { "light" }.into(),
        origin: Some(origin),
        brushes: Vec::new(),
        properties: fields
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    };
    let light = PointLight::from_entity(&source, -1).unwrap();
    if spot {
        light.with_spotlight(&source, &Default::default()).unwrap()
    } else {
        light
    }
}

fn sun_source(origin: Option<DVec3>, fields: &[(&str, &str)]) -> compiler::map::MapEntity {
    compiler::map::MapEntity {
        id: 10,
        classname: "light_environment".into(),
        origin,
        brushes: Vec::new(),
        properties: fields
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

fn sun_oracle(origin: Option<DVec3>, fields: &[(&str, &str)]) -> Sun {
    Sun::from_entity(&sun_source(origin, fields), &Default::default()).unwrap()
}

fn floor(
    document: &Document,
    game: &source_formats::gamefs::GameFs,
    materials: &mut render::MaterialLibrary,
    normal_maps: bool,
    lighting: bool,
) -> render::SceneMesh {
    render::SceneMesh::build_with_previews_filtered(
        document.vmf(),
        game,
        materials,
        (&Default::default(), &Default::default()),
        &[],
        (&[], &Default::default()),
        (&Default::default(), &[]),
        None,
        true,
        normal_maps,
        lighting,
        |_| true,
    )
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn authored_lighting_gpu_matches_compiler_oracle_and_preserves_off_and_diagnostic_pixels() {
    let (directory, game) = game();
    let mut document = Document::empty();
    document
        .edit("floor", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, -8.0),
                DVec3::new(32.0, 32.0, 0.0),
                "test/checker",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.u_axis.axis = DVec3::X;
                side.v_axis.axis = DVec3::Y;
                side.u_axis.scale = 1.0;
                side.v_axis.scale = 1.0;
            }
            Ok(())
        })
        .unwrap();
    let context = render::GpuContext::new().unwrap();
    let mut renderer = render::Renderer::new(&context);
    let mut materials = render::MaterialLibrary::default();
    let off = ViewOptions {
        grid_visible: false,
        ..Default::default()
    };
    let on = ViewOptions {
        lighting_preview: true,
        ..off
    };
    let baseline = floor(&document, &game, &mut materials, false, false);
    let old_pixels = rendered(&context, &mut renderer, &baseline, &materials, off);
    let center = (32 * 65 + 32) * 4;
    let cases = [
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![("_light", "255 128 32 100")],
            false,
        ),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![
                ("_light", "255 128 32 10"),
                ("_linear_attn", "1"),
                ("_quadratic_attn", "1"),
            ],
            false,
        ),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![("_light", "255 128 32 100"), ("_distance", "20")],
            false,
        ),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![
                ("_light", "255 128 32 1000"),
                ("_fifty_percent_distance", "40"),
                ("_zero_percent_distance", "80"),
                ("_hardfalloff", "1"),
            ],
            false,
        ),
        (
            DVec3::new(40.0, 0.0, 1000.0),
            vec![
                ("_light", "255 128 32 10000"),
                ("_fifty_percent_distance", "1"),
                ("_zero_percent_distance", "1000"),
            ],
            false,
        ),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![
                ("_light", "255 128 32 100"),
                ("pitch", "-90"),
                ("_inner_cone", "10"),
                ("_cone", "60"),
                ("_exponent", "2"),
            ],
            true,
        ),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![
                ("_light", "255 128 32 100"),
                ("pitch", "-90"),
                ("_inner_cone", "10"),
                ("_cone", "20"),
            ],
            true,
        ),
        (DVec3::ZERO, vec![("_light", "255 128 32 100")], false),
        (
            DVec3::new(40.0, 0.0, 64.0),
            vec![("_light", "255 128 32 100000000000")],
            false,
        ),
    ];
    for (origin, fields, spot) in cases {
        let light = oracle(origin, &fields, spot);
        let mut mesh = floor(&document, &game, &mut materials, false, true);
        mesh.lighting
            .lights
            .push(pack(light.preview_parameters().unwrap()));
        for exposure in [-8.0, 0.0, 8.0] {
            let pixels = rendered(
                &context,
                &mut renderer,
                &mesh,
                &materials,
                ViewOptions {
                    lighting_exposure_ev: exposure as f32,
                    ..on
                },
            );
            let expected = displayed(light.contribution(DVec3::ZERO, DVec3::Z).unwrap(), exposure);
            for (channel, expected_channel) in expected.iter().enumerate() {
                assert!(
                    pixels[center + channel].abs_diff(*expected_channel) <= 2,
                    "{fields:?} EV{exposure}: {:?} vs {expected:?}",
                    &pixels[center..center + 3]
                );
            }
            assert_eq!(pixels[center + 3], 255);
        }
        assert_eq!(
            old_pixels,
            rendered(&context, &mut renderer, &mesh, &materials, off)
        );
        for surface in [
            SurfaceView::Flat,
            SurfaceView::Wireframe,
            SurfaceView::LightmapDensity,
            SurfaceView::SmoothingGroups,
        ] {
            assert_eq!(
                rendered(
                    &context,
                    &mut renderer,
                    &baseline,
                    &materials,
                    ViewOptions { surface, ..off }
                ),
                rendered(
                    &context,
                    &mut renderer,
                    &mesh,
                    &materials,
                    ViewOptions { surface, ..on }
                )
            );
        }
    }
    for pitch in ["-90", "-30", "90"] {
        let mut previous = None;
        for origin in [
            None,
            Some(DVec3::ZERO),
            Some(DVec3::new(100.0, -200.0, 300.0)),
        ] {
            let sun = sun_oracle(
                origin,
                &[
                    ("_light", "255 0 0 200"),
                    ("pitch", pitch),
                    ("_ambient", "0 0 255 1000"),
                ],
            );
            let mut mesh = floor(&document, &game, &mut materials, false, true);
            mesh.lighting
                .lights
                .push(pack_sun(sun.preview_parameters().unwrap()));
            for quality in [
                render::lighting::Quality::Fast,
                render::lighting::Quality::Detail,
            ] {
                for exposure in [-8.0, 0.0, 8.0] {
                    let pixels = rendered(
                        &context,
                        &mut renderer,
                        &mesh,
                        &materials,
                        ViewOptions {
                            lighting_quality: quality,
                            lighting_exposure_ev: exposure as f32,
                            ..on
                        },
                    );
                    let expected_radiance = DVec3::X
                        * match pitch {
                            "-90" => 200.0,
                            "-30" => 100.0,
                            "90" => 0.0,
                            _ => unreachable!(),
                        };
                    assert!(
                        sun.contribution(DVec3::Z)
                            .unwrap()
                            .distance(expected_radiance)
                            < 1e-10
                    );
                    let expected = displayed(expected_radiance, exposure);
                    for probe in [(32 * 65 + 32) * 4, (24 * 65 + 32) * 4, (32 * 65 + 24) * 4] {
                        for channel in 0..3 {
                            assert!(
                                pixels[probe + channel].abs_diff(expected[channel]) <= 2,
                                "sun {pitch} {quality:?} EV{exposure}"
                            );
                        }
                    }
                    if quality == render::lighting::Quality::Detail && exposure == 0.0 {
                        if let Some(previous) = &previous {
                            assert_eq!(&pixels, previous);
                        }
                        previous = Some(pixels);
                    }
                }
            }
            assert_eq!(
                old_pixels,
                rendered(&context, &mut renderer, &mesh, &materials, off)
            );
        }
    }
    for (pitch, ambient, expected_sun) in [
        ("-90", Some("0 0 255 80"), DVec3::new(200.0, 0.0, 80.0)),
        ("90", Some("0 0 255 80"), DVec3::new(0.0, 0.0, 80.0)),
        ("-90", None, DVec3::new(300.0, 0.0, 0.0)),
    ] {
        let mut fields = vec![("_light", "255 0 0 200"), ("pitch", pitch)];
        if let Some(ambient) = ambient {
            fields.push(("_ambient", ambient));
        }
        let sun =
            Sun::from_entity_with_sky_ambient(&sun_source(None, &fields), &Default::default())
                .unwrap();
        assert!(sun.contribution(DVec3::Z).unwrap().distance(expected_sun) < 1e-10);
        let point = oracle(DVec3::Z * 64.0, &[("_light", "0 255 0 100")], false);
        let mut mesh = floor(&document, &game, &mut materials, false, true);
        mesh.lighting.lights.extend([
            pack_sun(sun.preview_parameters().unwrap()),
            pack(point.preview_parameters().unwrap()),
        ]);
        for quality in [
            render::lighting::Quality::Fast,
            render::lighting::Quality::Detail,
        ] {
            let position = if quality == render::lighting::Quality::Fast {
                DVec3::new(32.0, 32.0, 0.0)
            } else {
                DVec3::ZERO
            };
            let expected = displayed(
                expected_sun + point.contribution(position, DVec3::Z).unwrap(),
                0.0,
            );
            let pixels = rendered(
                &context,
                &mut renderer,
                &mesh,
                &materials,
                ViewOptions {
                    lighting_sky_ambient: true,
                    lighting_quality: quality,
                    ..on
                },
            );
            for (channel, expected_channel) in expected.iter().enumerate() {
                assert!(
                    pixels[center + channel].abs_diff(*expected_channel) <= 2,
                    "sun ambient {pitch} {ambient:?} {quality:?}: {:?} vs {expected:?}",
                    &pixels[center..center + 3]
                );
            }
        }
    }
    let point = oracle(DVec3::Z * 64.0, &[("_light", "255 128 32 100")], false);
    let sun = sun_oracle(None, &[("_light", "0 255 0 80"), ("pitch", "-30")]);
    let point_parameters = point.preview_parameters().unwrap();
    let sun_parameters = sun.preview_parameters().unwrap();
    for lights in [
        [pack(point_parameters), pack_sun(sun_parameters)],
        [pack_sun(sun_parameters), pack(point_parameters)],
    ] {
        let mut mesh = floor(&document, &game, &mut materials, false, true);
        mesh.lighting.lights.extend(lights);
        for quality in [
            render::lighting::Quality::Fast,
            render::lighting::Quality::Detail,
        ] {
            let position = if quality == render::lighting::Quality::Fast {
                DVec3::new(32.0, 32.0, 0.0)
            } else {
                DVec3::ZERO
            };
            let expected = displayed(
                point.contribution(position, DVec3::Z).unwrap() + DVec3::Y * 40.0,
                0.0,
            );
            let pixels = rendered(
                &context,
                &mut renderer,
                &mesh,
                &materials,
                ViewOptions {
                    lighting_quality: quality,
                    ..on
                },
            );
            for (channel, expected_channel) in expected.iter().enumerate() {
                assert!(
                    pixels[center + channel].abs_diff(*expected_channel) <= 2,
                    "mixed sun/point {quality:?}: {:?} vs {expected:?}",
                    &pixels[center..center + 3]
                );
            }
        }
    }
    let mut mesh = floor(&document, &game, &mut materials, false, true);
    assert_eq!(
        &rendered(&context, &mut renderer, &mesh, &materials, on)[center..center + 4],
        &[0, 0, 0, 255]
    );
    let light = oracle(
        DVec3::Z * 8.0,
        &[("_light", "255 255 255 1"), ("_quadratic_attn", "1")],
        false,
    );
    mesh.lighting
        .lights
        .push(pack(light.preview_parameters().unwrap()));
    let detailed = rendered(&context, &mut renderer, &mesh, &materials, on);
    let fast = rendered(
        &context,
        &mut renderer,
        &mesh,
        &materials,
        ViewOptions {
            lighting_quality: render::lighting::Quality::Fast,
            ..on
        },
    );
    let coarse = displayed(
        light
            .contribution(DVec3::new(32.0, 32.0, 0.0), DVec3::Z)
            .unwrap(),
        0.0,
    );
    assert!(fast[center].abs_diff(coarse[0]) <= 2);
    assert!(detailed[center] > fast[center] + 50);
    let mut normal = std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    normal[80..84].copy_from_slice(&[255, 128, 128, 255]);
    std::fs::write(directory.path().join("materials/test/normal.vtf"), normal).unwrap();
    std::fs::write(
        directory.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $bumpmap test/normal }",
    )
    .unwrap();
    materials.reload(&game);
    let mut mesh = floor(&document, &game, &mut materials, true, true);
    let light = oracle(
        DVec3::new(40.0, 0.0, 64.0),
        &[("_light", "255 255 255 100")],
        false,
    );
    mesh.lighting
        .lights
        .push(pack(light.preview_parameters().unwrap()));
    let pixels = rendered(
        &context,
        &mut renderer,
        &mesh,
        &materials,
        ViewOptions {
            normal_maps: true,
            ..on
        },
    );
    let expected = displayed(
        light
            .contribution(
                DVec3::ZERO,
                DVec3::new(1.0, 1.0 / 255.0, 1.0 / 255.0).normalize(),
            )
            .unwrap(),
        0.0,
    );
    assert!(pixels[center].abs_diff(expected[0]) <= 2);
    mesh.lighting.lights.clear();
    let sun = sun_oracle(
        None,
        &[
            ("_light", "255 255 255 100"),
            ("pitch", "-30"),
            ("angle", "180"),
        ],
    );
    mesh.lighting
        .lights
        .push(pack_sun(sun.preview_parameters().unwrap()));
    let pixels = rendered(
        &context,
        &mut renderer,
        &mesh,
        &materials,
        ViewOptions {
            normal_maps: true,
            ..on
        },
    );
    let normal = DVec3::new(1.0, 1.0 / 255.0, 1.0 / 255.0).normalize();
    let expected_radiance =
        DVec3::splat(100.0 * normal.dot(DVec3::new(3.0_f64.sqrt() / 2.0, 0.0, 0.5)));
    assert!(
        sun.contribution(normal)
            .unwrap()
            .distance(expected_radiance)
            < 1e-10
    );
    let expected = displayed(expected_radiance, 0.0);
    assert!(pixels[center].abs_diff(expected[0]) <= 2);
    let ambient = Sun::from_entity_with_sky_ambient(
        &sun_source(
            None,
            &[
                ("_light", "255 255 255 100"),
                ("pitch", "-30"),
                ("angle", "180"),
                ("_ambient", "0 0 255 30"),
            ],
        ),
        &Default::default(),
    )
    .unwrap();
    mesh.lighting.lights.clear();
    mesh.lighting
        .lights
        .push(pack_sun(ambient.preview_parameters().unwrap()));
    let pixels = rendered(
        &context,
        &mut renderer,
        &mesh,
        &materials,
        ViewOptions {
            lighting_sky_ambient: true,
            normal_maps: true,
            ..on
        },
    );
    let expected = displayed(expected_radiance + DVec3::Z * 30.0, 0.0);
    for (channel, expected_channel) in expected.iter().enumerate() {
        assert!(pixels[center + channel].abs_diff(*expected_channel) <= 2);
    }
}

#[test]
fn authored_lighting_reports_bounded_diagnostics_without_hiding_invalid_lights_or_stamping_old_source()
 {
    let (_directory, mut session, _) = fixture();
    enable(&mut session);
    for _ in 0..=render::lighting::MAX_DIAGNOSTICS {
        add(&mut session, "light", None, &[("_light", "255 255 255 10")]);
    }
    let first = session.build_scene().lighting.report;
    assert_eq!(first.excluded_lights, render::lighting::MAX_DIAGNOSTICS + 1);
    assert_eq!(first.diagnostics.len(), render::lighting::MAX_DIAGNOSTICS);
    assert!(first.diagnostics_truncated);
    let source = first.source.unwrap();
    add(
        &mut session,
        "light",
        Some(DVec3::Z * 32.0),
        &[("_light", "255 255 255 10")],
    );
    let current = session.build_scene().lighting.report;
    assert_eq!(current.admitted_lights, 1);
    assert_eq!(
        current.excluded_lights,
        render::lighting::MAX_DIAGNOSTICS + 1
    );
    assert!(current.source.as_ref().unwrap().scene_serial > source.scene_serial);
    assert!(current.source.as_ref().unwrap().revision > source.revision);
    assert_eq!(first.admitted_lights, 0);
}

mod terrain;

mod models;
