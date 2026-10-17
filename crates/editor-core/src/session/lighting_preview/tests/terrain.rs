use super::*;
use map_core::surface::FaceId;

fn patch(session: &mut Session, power: u32, x: f64) -> FaceId {
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::new(x, -32.0, -8.0),
            DVec3::new(x + 64.0, 32.0, 0.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let side = session
        .document
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    session
        .create_terrain(Actor::User, &[face], power, 0)
        .unwrap();
    face
}

fn curve(session: &mut Session, face: FaceId) {
    session
        .edit(Actor::User, "curved terrain", |tx| {
            let solid = tx.solid_mut(face.solid)?;
            let geometry = map_core::BrushGeometry::from_solid(solid).unwrap();
            let polygon = geometry
                .faces
                .iter()
                .find(|polygon| solid.sides[polygon.side].id == face.side)
                .unwrap();
            let info = solid.sides[polygon.side].displacement.as_mut().unwrap();
            let mesh = map_core::DisplacementMesh::build(polygon, info).unwrap();
            info.distances = Some(
                mesh.base_positions
                    .chunks(mesh.size)
                    .map(|row| {
                        row.iter()
                            .map(|point| point.x / 4.0 + point.y.powi(2) / 128.0)
                            .collect()
                    })
                    .collect(),
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn terrain_receiver_counts_preserve_flat_counts_sewn_normals_and_whole_preview_fallback() {
    let (_directory, game) = game();
    let mut session = Session::new(Ok(game));
    let mut faces = Vec::new();
    for (power, x) in [(2, -96.0), (3, -32.0), (4, 32.0)] {
        let face = patch(&mut session, power, x);
        curve(&mut session, face);
        faces.push(face);
    }
    let detail = faces[2].solid;
    session
        .edit(Actor::User, "detail terrain", |tx| {
            edit::tie_to_entity(tx, &[detail], "func_detail", &[])
        })
        .unwrap();
    let off = session.build_scene();
    assert_eq!(off.lighting.report.terrain, Default::default());
    enable(&mut session);
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let on = session.build_scene();
    assert_eq!(on.lighting.report.admitted_receivers, 0);
    assert_eq!(on.lighting.report.excluded_receivers, 0);
    assert_eq!(on.lighting.report.terrain.admitted_surfaces, 3);
    assert_eq!(
        on.lighting.report.terrain.admitted_triangles,
        32 + 128 + 512
    );
    assert_eq!(on.lighting.report.terrain.excluded_surfaces, 0);
    let mut vertices = on.vertices.clone();
    for vertex in &mut vertices {
        vertex.lighting_receiver = 0;
    }
    assert_eq!(vertices, off.vertices);
    assert_eq!(on.indices, off.indices);
    assert!(
        on.vertices
            .iter()
            .all(|vertex| vertex.lighting_receiver == 1 && vertex.tangent == [0.0; 4])
    );
    for vertex in on
        .vertices
        .iter()
        .filter(|vertex| vertex.position[0] == -32.0 || vertex.position[0] == 32.0)
    {
        assert!(
            on.vertices
                .iter()
                .filter(|other| other.position == vertex.position)
                .all(|other| other.normal == vertex.normal)
        );
    }
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    for _ in 0..=MAX_LIGHTS {
        add(
            &mut session,
            "light",
            Some(DVec3::Z * 64.0),
            &[("_light", "255 255 255 1")],
        );
    }
    let overflow = session.build_scene();
    assert!(!overflow.lighting.report.active);
    assert_eq!(overflow.lighting.report.terrain.admitted_surfaces, 0);
    assert_eq!(overflow.lighting.report.terrain.admitted_triangles, 0);
    assert_eq!(overflow.lighting.report.terrain.excluded_surfaces, 3);
    assert_eq!(overflow.lighting.report.terrain.excluded_triangles, 672);
    assert!(
        overflow
            .vertices
            .iter()
            .all(|vertex| vertex.lighting_receiver == 0)
    );
    for surface in [
        SurfaceView::Flat,
        SurfaceView::Wireframe,
        SurfaceView::LightmapDensity,
        SurfaceView::SmoothingGroups,
    ] {
        let mesh = session.build_scene_for_view(ViewOptions {
            surface,
            ..session.view()
        });
        assert_eq!(mesh.lighting.report.terrain, Default::default());
    }
}

#[test]
fn terrain_receiver_policy_is_cached_with_base_and_rejection_retains_ordinary_geometry() {
    let (directory, game) = game();
    let mut session = Session::new(Ok(game));
    let face = patch(&mut session, 2, -32.0);
    enable(&mut session);
    assert_eq!(
        session
            .build_scene()
            .lighting
            .report
            .terrain
            .admitted_surfaces,
        1
    );
    let vmt = directory.path().join("materials/test/checker.vmt");
    for extra in [
        "$bumpmap missing/normal",
        "$normalmap missing/normal",
        "$ssbump 0",
        "$forcebump 0",
        "$basetexture2 missing/base",
        "$detail missing/detail",
        "$blendmodulatetexture missing/blend",
        "$envmap env_cubemap",
        "$phong 1",
        "$lightwarptexture missing/warp",
        "$translucent 1",
        "$alphatest 1",
        "$selfillum 0",
        "%tooltexture test/checker",
        "Proxies { AnimatedTexture {} }",
    ] {
        std::fs::write(
            &vmt,
            format!("LightmappedGeneric {{ $basetexture test/checker {extra} }}"),
        )
        .unwrap();
        if extra == "$bumpmap missing/normal" {
            assert_eq!(
                session
                    .build_scene()
                    .lighting
                    .report
                    .terrain
                    .admitted_surfaces,
                1
            );
        }
        session.reload_material_assets(Actor::User);
        let mesh = session.build_scene();
        assert_eq!(mesh.lighting.report.terrain.excluded_surfaces, 1, "{extra}");
        assert_eq!(
            mesh.lighting.report.terrain.excluded_triangles, 32,
            "{extra}"
        );
        assert_eq!(mesh.lighting.report.terrain.admitted_surfaces, 0);
        assert!(
            mesh.lighting
                .report
                .diagnostics
                .iter()
                .any(|row| row.material.as_deref() == Some("test/checker")
                    && row.reason.contains(&format!("face {}", face.side)))
        );
        let off = session.build_scene_for_view(ViewOptions {
            lighting_preview: false,
            ..session.view()
        });
        assert_eq!(mesh.vertices, off.vertices, "{extra}");
        assert_eq!(mesh.indices, off.indices, "{extra}");
        assert_eq!(mesh.stats.displacements, 1);
    }
    for material in [
        "WorldVertexTransition { $basetexture test/checker $basetexture2 test/checker }",
        "LightmappedGeneric { $basetexture missing/base }",
    ] {
        std::fs::write(&vmt, material).unwrap();
        session.reload_material_assets(Actor::User);
        let mesh = session.build_scene();
        assert_eq!(mesh.lighting.report.terrain.excluded_surfaces, 1);
        assert_eq!(mesh.lighting.report.terrain.excluded_triangles, 32);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.lighting_receiver == 0)
        );
    }
    std::fs::write(vmt, "LightmappedGeneric { $basetexture test/checker }").unwrap();
    session.reload_material_assets(Actor::User);
    session
        .edit(Actor::User, "unsupported owner", |tx| {
            edit::tie_to_entity(tx, &[face.solid], "func_brush", &[])
        })
        .unwrap();
    let mesh = session.build_scene();
    assert_eq!(mesh.lighting.report.terrain.excluded_triangles, 32);
    assert!(
        mesh.lighting
            .report
            .diagnostics
            .iter()
            .any(|row| row.reason.contains("world or func_detail"))
    );
}

#[test]
fn terrain_receiver_rejections_count_outline_only_sheets_and_bounded_drawn_sheets() {
    for case in ["smoothing", "coordinates"] {
        let (_directory, game) = game();
        let mut session = Session::new(Ok(game));
        let face = patch(&mut session, 2, -32.0);
        session
            .edit(Actor::User, "unsupported receiver", |tx| {
                let side = tx
                    .solid_mut(face.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap();
                let info = side.displacement.as_mut().unwrap();
                if case == "smoothing" {
                    info.extra_properties.set("smooth", "broken");
                } else {
                    info.distances.as_mut().unwrap()[2][2] = 16400.0;
                }
                Ok(())
            })
            .unwrap();
        let off = session.build_scene();
        enable(&mut session);
        let on = session.build_scene();
        assert_eq!(on.vertices, off.vertices);
        assert_eq!(on.indices, off.indices);
        assert_eq!(on.lines, off.lines);
        assert_eq!(on.lighting.report.terrain.excluded_surfaces, 1);
        assert_eq!(
            on.lighting.report.terrain.excluded_triangles,
            if case == "smoothing" { 0 } else { 32 }
        );
        assert!(
            on.lighting
                .report
                .summary()
                .contains("Partial scene coverage")
        );
        assert!(
            on.lighting
                .report
                .diagnostics
                .iter()
                .any(|row| row.reason.contains(if case == "smoothing" {
                    "outline"
                } else {
                    "Source compile coordinates"
                }))
        );
    }
}

fn sample_triangle(
    positions: &[[f64; 3]],
    triangles: &[[usize; 3]],
    point: glam::DVec2,
) -> ([usize; 3], [f64; 3]) {
    for &triangle in triangles {
        let [a, b, c] = triangle.map(|index| DVec3::from_array(positions[index]).truncate());
        let cross = |a: glam::DVec2, b: glam::DVec2| a.x * b.y - a.y * b.x;
        let area = cross(b - a, c - a);
        let weights = [
            cross(b - point, c - point) / area,
            cross(c - point, a - point) / area,
            cross(a - point, b - point) / area,
        ];
        if weights.iter().all(|weight| *weight > 1e-6) {
            return (triangle, weights);
        }
    }
    panic!("oracle probe must be strictly inside a terrain triangle");
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn terrain_authored_lighting_gpu_matches_compiler_fast_detail_and_independent_normals() {
    let context = render::GpuContext::new().unwrap();
    let mut renderer = render::Renderer::new(&context);
    let camera = render::Camera::planar(render::Projection::Top, glam::Vec3::ZERO, 65.0);
    let off = ViewOptions {
        grid_visible: false,
        ..Default::default()
    };
    let on = ViewOptions {
        lighting_preview: true,
        ..off
    };
    let probe = glam::DVec2::new(3.0, 6.0);
    let center = (26 * 65 + 35) * 4;
    for power in [2, 3, 4] {
        let (_directory, game) = game();
        let mut session = Session::new(Ok(game));
        let face = patch(&mut session, power, -32.0);
        curve(&mut session, face);
        let game = session.game();
        let mut materials = render::MaterialLibrary::default();
        let baseline = floor(&session.document, game, &mut materials, false, false);
        let solid = session.document.solid(face.solid).unwrap();
        let geometry = map_core::BrushGeometry::from_solid(solid).unwrap();
        let polygon = geometry
            .faces
            .iter()
            .find(|polygon| solid.sides[polygon.side].id == face.side)
            .unwrap();
        let source = map_core::DisplacementMesh::build(
            polygon,
            solid.sides[polygon.side].displacement.as_ref().unwrap(),
        )
        .unwrap();
        let positions = source
            .positions
            .iter()
            .map(|point| point.to_array())
            .collect::<Vec<_>>();
        let info = source_formats::bsp::DisplacementInfo {
            power: power as i32,
            ..Default::default()
        };
        let normals = info.vertex_normals(&positions).unwrap();
        assert_eq!(baseline.vertices.len(), normals.len());
        let middle = source.size / 2 * source.size + source.size / 2;
        assert_eq!(source.positions[middle], DVec3::ZERO);
        let analytic = DVec3::new(-0.25, 0.0, 1.0).normalize();
        assert!(DVec3::from_array(normals[middle]).distance(analytic) < 1e-12);
        assert!(
            DVec3::from_array(baseline.vertices[middle].normal.map(f64::from)).distance(analytic)
                < 1e-6
        );
        for (vertex, normal) in baseline.vertices.iter().zip(&normals) {
            assert!(
                DVec3::from_array(vertex.normal.map(f64::from))
                    .distance(DVec3::from_array(*normal))
                    < 1e-6
            );
        }
        let (triangle, weights) = sample_triangle(
            &positions,
            &info.lightmap_triangle_indices().unwrap(),
            probe,
        );
        let point = triangle
            .iter()
            .zip(weights)
            .map(|(&index, weight)| DVec3::from_array(positions[index]) * weight)
            .sum::<DVec3>();
        let normal = triangle
            .iter()
            .zip(weights)
            .map(|(&index, weight)| DVec3::from_array(normals[index]) * weight)
            .sum::<DVec3>()
            .normalize();
        assert!(normal.distance(DVec3::Z) > 0.1);
        let uploaded = renderer.upload(&context, &baseline, &materials);
        let ordinary = renderer.render_to_rgba(&context, &uploaded, &camera, &off, 65, 65);
        for (origin, fields, spot) in [
            (
                DVec3::new(7.0, 6.0, 18.0),
                vec![("_light", "255 128 32 10"), ("_quadratic_attn", "1")],
                false,
            ),
            (
                DVec3::new(0.0, 0.0, 64.0),
                vec![
                    ("_light", "100 170 255 1000"),
                    ("pitch", "-90"),
                    ("_inner_cone", "5"),
                    ("_cone", "55"),
                    ("_exponent", "2"),
                    ("_fifty_percent_distance", "40"),
                    ("_zero_percent_distance", "100"),
                ],
                true,
            ),
        ] {
            let light = oracle(origin, &fields, spot);
            let mut mesh = floor(&session.document, game, &mut materials, false, true);
            mesh.lighting
                .lights
                .push(pack(light.preview_parameters().unwrap()));
            assert_eq!(mesh.lighting.report.terrain.admitted_surfaces, 1);
            for quality in [
                render::lighting::Quality::Fast,
                render::lighting::Quality::Detail,
            ] {
                let contribution = if quality == render::lighting::Quality::Fast {
                    triangle
                        .iter()
                        .zip(weights)
                        .map(|(&index, weight)| {
                            light
                                .contribution(
                                    DVec3::from_array(positions[index]),
                                    DVec3::from_array(normals[index]),
                                )
                                .unwrap()
                                * weight
                        })
                        .sum()
                } else {
                    light.contribution(point, normal).unwrap()
                };
                let uploaded = renderer.upload(&context, &mesh, &materials);
                let pixels = renderer.render_to_rgba(
                    &context,
                    &uploaded,
                    &camera,
                    &ViewOptions {
                        lighting_quality: quality,
                        ..on
                    },
                    65,
                    65,
                );
                let expected = displayed(contribution, 0.0);
                if quality == render::lighting::Quality::Detail && !spot {
                    let flat = displayed(light.contribution(point, DVec3::Z).unwrap(), 0.0);
                    assert!(
                        expected.iter().zip(flat).any(|(a, b)| a.abs_diff(b) > 3),
                        "the oracle must distinguish prepared curved normals from flat Z"
                    );
                }
                for channel in 0..3 {
                    assert!(
                        pixels[center + channel].abs_diff(expected[channel]) <= 3,
                        "power {power}, {quality:?}, spot {spot}: {:?} != {expected:?}",
                        &pixels[center..center + 3]
                    );
                }
            }
            let uploaded = renderer.upload(&context, &mesh, &materials);
            assert_eq!(
                renderer.render_to_rgba(&context, &uploaded, &camera, &off, 65, 65),
                ordinary
            );
            for surface in [
                SurfaceView::Flat,
                SurfaceView::Wireframe,
                SurfaceView::LightmapDensity,
                SurfaceView::SmoothingGroups,
            ] {
                let disabled = ViewOptions { surface, ..off };
                let enabled = ViewOptions {
                    lighting_preview: true,
                    ..disabled
                };
                assert_eq!(
                    renderer.render_to_rgba(&context, &uploaded, &camera, &disabled, 65, 65),
                    renderer.render_to_rgba(&context, &uploaded, &camera, &enabled, 65, 65)
                );
            }
        }
    }
}
