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
    for quality in ["fast", "detail"] {
        let view = session
            .resolve_view_update(ViewUpdate {
                lighting_quality: Some(quality),
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
