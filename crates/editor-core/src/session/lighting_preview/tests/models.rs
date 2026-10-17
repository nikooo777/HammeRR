use super::*;

fn fixture() -> (tempfile::TempDir, Session, ObjectId) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    let mut session = Session::new(Ok(game));
    let model = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::ZERO),
            std::collections::BTreeMap::from([("model".into(), "models/test/receiver.mdl".into())]),
        )
        .unwrap();
    (directory, session, model)
}

#[test]
fn model_receivers_use_the_render_document_and_transfer_counts_on_whole_preview_failure() {
    let (_directory, mut session, model) = fixture();
    let off = session.build_scene();
    assert_eq!(off.lighting.report.models, Default::default());
    assert!(
        off.vertices
            .iter()
            .all(|vertex| vertex.lighting_receiver == 0)
    );
    enable(&mut session);
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let on = session.build_scene();
    assert!(on.lighting.report.active);
    assert_eq!(on.lighting.report.admitted_lights, 0);
    assert_eq!(on.lighting.report.models.admitted_instances, 1);
    assert_eq!(on.lighting.report.models.admitted_triangles, 2);
    session
        .preview_transform(
            Actor::User,
            &[model],
            edit::Transform {
                translation: DVec3::new(16., 8., 4.),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let draft = session.render_document().vmf().clone();
    let moved = session.build_scene();
    assert_eq!(moved.lighting.report.models.admitted_instances, 1);
    for (a, b) in off.vertices.iter().zip(&moved.vertices) {
        assert_eq!(
            DVec3::from_array(a.position.map(f64::from)) + DVec3::new(16., 8., 4.),
            DVec3::from_array(b.position.map(f64::from))
        );
    }
    for quality in ["fast", "detail"] {
        let view = session
            .resolve_view_update(ViewUpdate {
                lighting_quality: Some(quality),
                lighting_exposure_ev: Some(2.),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        assert_eq!(
            session
                .build_scene()
                .lighting
                .report
                .models
                .admitted_triangles,
            2
        );
        assert_eq!(session.render_document().vmf(), &draft);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
    }
    session.cancel_transform_preview();
    for index in 0..=MAX_LIGHTS {
        add(
            &mut session,
            "light",
            Some(DVec3::new(index as f64, 0., 64.)),
            &[("_light", "255 255 255 100")],
        );
    }
    let failed = session.build_scene();
    assert!(!failed.lighting.report.active);
    assert_eq!(failed.lighting.report.models.admitted_instances, 0);
    assert_eq!(failed.lighting.report.models.excluded_instances, 1);
    assert_eq!(failed.lighting.report.models.excluded_parts, 1);
    assert_eq!(failed.lighting.report.models.excluded_triangles, 2);
    assert!(
        failed
            .vertices
            .iter()
            .all(|vertex| vertex.lighting_receiver == 0)
    );
}

#[test]
fn source_bounds_skin_opacity_hidden_and_working_region_do_not_repair_or_edit_models() {
    let (_directory, mut session, ObjectId::Entity(model)) = fixture() else {
        unreachable!()
    };
    enable(&mut session);
    for (fields, reason) in [
        (
            vec![("origin".into(), Some("17000 0 0".into()))],
            "Source compile coordinates",
        ),
        (
            vec![
                ("origin".into(), Some("0 0 0".into())),
                ("skin".into(), Some("1".into())),
            ],
            "$phong",
        ),
        (
            vec![
                ("skin".into(), Some("0".into())),
                ("rendermode".into(), Some("2".into())),
                ("renderamt".into(), Some("255".into())),
            ],
            "Normal render mode",
        ),
    ] {
        session
            .set_entity_properties(Actor::User, model, &fields)
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let mesh = session.build_scene();
        assert_eq!(mesh.lighting.report.models.excluded_instances, 1);
        assert_eq!(mesh.lighting.report.models.excluded_triangles, 2);
        assert!(
            mesh.lighting
                .report
                .diagnostics
                .iter()
                .any(|row| row.entity == Some(model) && row.reason.contains(reason))
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
    }
    session
        .set_entity_properties(
            Actor::User,
            model,
            &[("rendermode".into(), Some("0".into()))],
        )
        .unwrap();
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Set {
                bounds: [DVec3::splat(1000.), DVec3::splat(1100.)],
                enabled: true,
            },
            &stamp,
        )
        .unwrap();
    assert_eq!(
        session.build_scene().lighting.report.models,
        Default::default()
    );
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Clear,
            &stamp,
        )
        .unwrap();
    session
        .edit(Actor::User, "hide model", |tx| {
            tx.entity_mut(model)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        session.build_scene().lighting.report.models,
        Default::default()
    );
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn model_authored_lighting_gpu_matches_compiler_fast_detail_scaled_world_normals_and_tint() {
    let (directory, game) = crate::test_assets::model_lighting::game();
    let image = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&image).unwrap();
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes.truncate(80);
    bytes.extend([255; 4]);
    std::fs::write(image, bytes).unwrap();
    let vertices = directory.path().join("models/test/receiver.vvd");
    let mut bytes = std::fs::read(&vertices).unwrap();
    for (axis, value) in [0.6f32, 0., 0.8].into_iter().enumerate() {
        let offset = 64 + 2 * 48 + 28 + axis * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    std::fs::write(vertices, bytes).unwrap();
    let mut session = Session::new(Ok(game));
    session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::new(12., -8., 4.)),
            std::collections::BTreeMap::from([
                ("model".into(), "models/test/receiver.mdl".into()),
                ("angles".into(), "30 0 0".into()),
                ("modelscale".into(), "2".into()),
                ("rendercolor".into(), "64 128 255".into()),
            ]),
        )
        .unwrap();
    let context = render::GpuContext::new().unwrap();
    let mut renderer = render::Renderer::new(&context);
    let camera =
        render::Camera::planar(render::Projection::Top, glam::Vec3::new(12., -8., 4.), 129.);
    let origin = DVec3::new(12., -8., 4.);
    let (sine, cosine) = 30f64.to_radians().sin_cos();
    let rotate = |point: DVec3| {
        DVec3::new(
            cosine * point.x + sine * point.z,
            point.y,
            -sine * point.x + cosine * point.z,
        )
    };
    let points = [
        origin + rotate(DVec3::new(-48., -48., 0.)),
        origin + rotate(DVec3::new(48., 48., 0.)),
    ];
    let normals = [
        rotate(DVec3::Z),
        rotate(DVec3::new(0.6, 0., 0.8)).normalize(),
    ];
    let detail_normal = (normals[0] + normals[1]).normalize();
    let tint = DVec3::new(64. / 255., 128. / 255., 1.);
    let center = (32 * 65 + 32) * 4;
    for spot in [false, true] {
        let light_origin = origin + DVec3::new(40., 0., 64.);
        let mut fields = vec![
            ("_light", "255 128 32 1000"),
            ("_fifty_percent_distance", "64"),
            ("_zero_percent_distance", "160"),
        ];
        if spot {
            fields.extend([
                ("pitch", "-90"),
                ("_inner_cone", "10"),
                ("_cone", "70"),
                ("_exponent", "2"),
            ]);
        }
        let light = oracle(light_origin, &fields, spot);
        let light_id = add(
            &mut session,
            if spot { "light_spot" } else { "light" },
            Some(light_origin),
            &fields,
        );
        let off = ViewOptions {
            grid_visible: false,
            ..session.view()
        };
        let baseline = session.build_scene_for_view(off);
        let uploaded = renderer.upload(&context, &baseline, session.materials());
        let ordinary = renderer.render_to_rgba(&context, &uploaded, &camera, &off, 65, 65);
        for exposure in [0., 2.] {
            for quality in [
                render::lighting::Quality::Fast,
                render::lighting::Quality::Detail,
            ] {
                let view = ViewOptions {
                    lighting_preview: true,
                    lighting_quality: quality,
                    lighting_exposure_ev: exposure,
                    ..off
                };
                let mesh = session.build_scene_for_view(view);
                assert_eq!(mesh.lighting.report.models.admitted_instances, 1);
                assert_eq!(mesh.lighting.report.models.admitted_triangles, 2);
                let uploaded = renderer.upload(&context, &mesh, session.materials());
                let pixels = renderer.render_to_rgba(&context, &uploaded, &camera, &view, 65, 65);
                let contribution = if quality == render::lighting::Quality::Fast {
                    (light.contribution(points[0], normals[0])
                        + light.contribution(points[1], normals[1]))
                        * 0.5
                } else {
                    light.contribution(origin, detail_normal)
                };
                let expected = displayed(contribution * tint, f64::from(exposure));
                for channel in 0..3 {
                    assert!(
                        pixels[center + channel].abs_diff(expected[channel]) <= 3,
                        "{quality:?} spot={spot} EV={exposure}: {:?} vs {expected:?}",
                        &pixels[center..center + 3]
                    );
                }
                assert_ne!(&pixels[center..center + 3], &ordinary[center..center + 3]);
            }
        }
        for surface in [
            SurfaceView::Flat,
            SurfaceView::Wireframe,
            SurfaceView::LightmapDensity,
            SurfaceView::SmoothingGroups,
        ] {
            let view = ViewOptions { surface, ..off };
            let a = session.build_scene_for_view(view);
            let ua = renderer.upload(&context, &a, session.materials());
            let pa = renderer.render_to_rgba(&context, &ua, &camera, &view, 65, 65);
            let view = ViewOptions {
                lighting_preview: true,
                ..view
            };
            let b = session.build_scene_for_view(view);
            assert_eq!(b.lighting.report.models, Default::default());
            let ub = renderer.upload(&context, &b, session.materials());
            assert_eq!(
                pa,
                renderer.render_to_rgba(&context, &ub, &camera, &view, 65, 65)
            );
        }
        session
            .edit(Actor::User, "remove oracle light", |tx| tx.remove(light_id))
            .unwrap();
    }
}
