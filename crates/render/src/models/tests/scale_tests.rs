use super::*;

fn offset_asset(reference: bool) -> Arc<Asset> {
    let mut asset = asset();
    let model = &mut Arc::get_mut(&mut asset).unwrap().model;
    model.reference_bones = usize::from(reference) * 2;
    model.header.flags = if reference { 0 } else { 16 };
    for (vertex, point) in model.vertices.iter_mut().zip([
        [2.0, 4.0, 6.0],
        [6.0, 4.0, 6.0],
        [6.0, 8.0, 6.0],
        [2.0, 8.0, 6.0],
    ]) {
        vertex.position = point;
    }
    model.bounds = [[2.0, 4.0, 6.0], [6.0, 8.0, 6.0]];
    asset
}

fn entity(scale: Option<&str>) -> Entity {
    let mut entity = Entity::new(7, "prop_dynamic");
    entity.properties.set("origin", "10 20 30");
    entity.properties.set("angles", "0 90 0");
    if let Some(scale) = scale {
        entity.properties.set("modelscale", scale);
    }
    entity
}

#[test]
fn model_scale_native_float_admission_and_writer_boundaries_are_explicit() {
    for (raw, expected) in [
        (None, 1.0),
        (Some("1"), 1.0),
        (Some("1.00000001"), 1.0),
        (Some("0.015625"), 1.0 / 64.0),
        (Some("64"), 64.0),
        (Some("64.000001"), 64.0),
        (Some("2.5e-1"), 0.25),
    ] {
        assert_eq!(scale(&entity(raw)).unwrap(), expected);
    }
    for raw in [
        "",
        " ",
        "NaN",
        "inf",
        "-inf",
        "0",
        "-0",
        "-1",
        "1e40",
        "1e-99",
        "0.01562499",
        "64.00001",
        "2 2 2",
        "0x1",
        " 1",
    ] {
        assert!(scale(&entity(Some(raw))).is_err(), "{raw}");
    }
    assert_eq!(
        scale(&entity(Some(&format!("1.{}", "0".repeat(62))))).unwrap(),
        1.0
    );
    assert!(
        scale(&entity(Some(&format!("1.{}", "0".repeat(63)))))
            .unwrap_err()
            .contains("64 UTF-8 bytes")
    );
    for class in [
        "prop_dynamic",
        "prop_dynamic_override",
        "prop_physics",
        "custom_model_entity",
    ] {
        let mut entity = entity(Some("2"));
        entity.properties.set("classname", class);
        assert_eq!(scale(&entity).unwrap(), 2.0);
        assert!(Instance::new(&entity, asset()).is_ok());
    }
    for (class, diagnostic) in [
        ("prop_static", "Static-prop output"),
        ("prop_detail", "Native detail records"),
        ("detail_prop", "Native detail records"),
    ] {
        let mut entity = entity(Some("2"));
        entity.properties.set("classname", class);
        assert!(scale(&entity).unwrap_err().contains(diagnostic));
        assert!(!scale_editable(&entity));
    }
    let mut detail = entity(Some("1.00000001"));
    detail.properties.set("classname", "prop_detail");
    assert!(scale(&detail).is_err());
    assert!(
        Instance::editor_guide(&entity(Some("2")), asset())
            .err()
            .unwrap()
            .contains("FGD model guides")
    );
    let mut outside = entity(Some("64"));
    outside.properties.set("origin", "10 65535 30");
    assert!(
        Instance::new(&outside, offset_asset(false))
            .err()
            .unwrap()
            .contains("Transformed model bounds")
    );
}

#[test]
fn model_scale_points_bounds_and_ray_distances_match_independent_world_coordinates() {
    for reference in [false, true] {
        let asset = offset_asset(reference);
        let original = asset.model.vertices.clone();
        for (raw, low, high, ray_origin, expected_distance) in [
            (
                "2",
                [-6.0, 24.0, 42.0],
                [2.0, 32.0, 42.0],
                [-2.0, 28.0, 70.0],
                28.0,
            ),
            (
                "0.5",
                [6.0, 21.0, 33.0],
                [8.0, 23.0, 33.0],
                [7.0, 22.0, 70.0],
                37.0,
            ),
        ] {
            let instance = Instance::new(&entity(Some(raw)), asset.clone()).unwrap();
            let (a, b) = instance.bounds();
            assert!(a.abs_diff_eq(DVec3::from_array(low), 1e-10));
            assert!(b.abs_diff_eq(DVec3::from_array(high), 1e-10));
            let ray = Ray {
                origin: DVec3::from_array(ray_origin),
                direction: DVec3::NEG_Z * 7.0,
            };
            let hit = instance.pick_surface(&ray).unwrap();
            assert!((hit.hit.distance - expected_distance).abs() < 1e-10);
            assert!(
                hit.hit
                    .point
                    .abs_diff_eq(DVec3::new(ray_origin[0], ray_origin[1], low[2]), 1e-10)
            );
            assert!(hit.normal.abs_diff_eq(DVec3::Z, 1e-10));
            assert!(
                instance
                    .pick(&Ray {
                        origin: DVec3::new(high[0] + 0.25, ray_origin[1], 70.0),
                        ..ray
                    })
                    .is_none()
            );
        }
        let instance = Instance::new(&entity(Some("2")), asset.clone()).unwrap();
        assert!(
            instance
                .point([2.0, 4.0, 6.0])
                .abs_diff_eq(DVec3::new(2.0, 24.0, 42.0), 1e-10)
        );
        let basis = DMat3::from_quat(instance.rotation);
        let (low, high) = instance.bounds_in_basis(basis);
        assert!(low.abs_diff_eq(DVec3::new(24.0, -2.0, 42.0), 1e-10));
        assert!(high.abs_diff_eq(DVec3::new(32.0, 6.0, 42.0), 1e-10));
        let mut source = entity(Some("2"));
        source.properties.set("angles", "0 90 90");
        let instance = Instance::new(&source, asset.clone()).unwrap();
        assert!(
            instance
                .point([2.0, 4.0, 6.0])
                .abs_diff_eq(DVec3::new(22.0, 24.0, 38.0), 1e-10)
        );
        let hit = instance
            .pick_surface(&Ray {
                origin: DVec3::new(50.0, 28.0, 42.0),
                direction: DVec3::NEG_X * 5.0,
            })
            .unwrap();
        assert!((hit.hit.distance - 28.0).abs() < 1e-10);
        assert!(
            hit.hit
                .point
                .abs_diff_eq(DVec3::new(22.0, 28.0, 42.0), 1e-10)
        );
        assert!(hit.normal.abs_diff_eq(DVec3::X, 1e-10));
        assert!(
            instance
                .pick(&Ray {
                    origin: DVec3::new(0.0, 28.0, 42.0),
                    direction: DVec3::X
                })
                .is_none()
        );
        assert_eq!(asset.model.vertices, original);
    }
}

#[test]
fn model_scale_ray_thresholds_retain_normalized_local_direction() {
    let mut asset = asset();
    let model = &mut Arc::get_mut(&mut asset).unwrap().model;
    for (vertex, point) in model.vertices.iter_mut().zip([
        [0.0, 0.0, 0.0],
        [0.000004, 0.0, 0.0],
        [0.000004, 0.000004, 0.0],
        [0.0, 0.000004, 0.0],
    ]) {
        vertex.position = point;
    }
    model.bounds = [[0.0; 3], [0.000004, 0.000004, 0.0]];
    for (raw, coordinate) in [("0.015625", 0.00000003125), ("64", 0.000128)] {
        let mut source = entity(Some(raw));
        source.properties.set("origin", "0 0 0");
        source.properties.set("angles", "0 0 0");
        let instance = Instance::new(&source, asset.clone()).unwrap();
        let hit = instance
            .pick_surface(&Ray {
                origin: DVec3::new(coordinate, coordinate, 3.0),
                direction: DVec3::NEG_Z * 7.0,
            })
            .unwrap();
        assert!((hit.hit.distance - 3.0).abs() < 1e-12);
        assert!(
            hit.hit
                .point
                .abs_diff_eq(DVec3::new(coordinate, coordinate, 0.0), 1e-12)
        );
        assert!(hit.normal.abs_diff_eq(DVec3::Z, 1e-12));
    }
}

#[test]
fn model_scale_attachments_preserve_world_alignment_and_shared_asset_geometry() {
    let mut asset = offset_asset(true);
    for flags in [0, 0x10000] {
        Arc::get_mut(&mut asset).unwrap().model.attachments.push(
            source_formats::model::Attachment {
                name: "scaled attachment".into(),
                bone: 0,
                flags,
                transform: glam::DAffine3::from_rotation_translation(
                    DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
                    DVec3::new(4.0, 2.0, 8.0),
                ),
            },
        );
    }
    let snapshot = |asset: &Asset| {
        asset
            .model
            .attachments
            .iter()
            .map(|attachment| {
                (
                    attachment.name.clone(),
                    attachment.bone,
                    attachment.flags,
                    attachment.transform,
                )
            })
            .collect::<Vec<_>>()
    };
    let original = snapshot(&asset);
    let mut source = entity(Some("2"));
    source.properties.set("origin", "100 200 20");
    let large = Instance::new(&source, asset.clone()).unwrap();
    source.properties.set("modelscale", "0.5");
    let small = Instance::new(&source, asset.clone()).unwrap();
    assert!(Arc::ptr_eq(&small.asset, &large.asset));
    let ordinary = large.attachment(1).unwrap();
    let aligned = large.attachment(2).unwrap();
    assert!(
        ordinary
            .translation
            .abs_diff_eq(DVec3::new(96.0, 208.0, 36.0), 1e-10)
    );
    assert_eq!(aligned.translation, ordinary.translation);
    assert!(
        ordinary
            .matrix3
            .x_axis
            .abs_diff_eq(DVec3::NEG_X * 2.0, 1e-10)
    );
    assert!(
        ordinary
            .matrix3
            .y_axis
            .abs_diff_eq(DVec3::NEG_Y * 2.0, 1e-10)
    );
    assert!(ordinary.matrix3.z_axis.abs_diff_eq(DVec3::Z * 2.0, 1e-10));
    assert_eq!(aligned.matrix3, DMat3::IDENTITY);
    assert!(
        small
            .attachment(1)
            .unwrap()
            .translation
            .abs_diff_eq(DVec3::new(99.0, 202.0, 24.0), 1e-10)
    );
    assert_eq!(snapshot(&asset), original);
}

#[test]
fn model_scale_scene_vertices_wires_tint_opacity_and_unit_defaults_agree() {
    let asset = offset_asset(false);
    let mut source = entity(Some("2"));
    source.properties.set("rendercolor", "64 128 255");
    source.properties.set("rendermode", "2");
    source.properties.set("renderamt", "128");
    let instance = Instance::new(&source, asset.clone()).unwrap();
    let game = material_game(std::path::Path::new("/nonexistent-model-scale-fixture"));
    let mesh = |source: &Entity, instance: Instance| {
        let mut vmf = Document::empty().vmf().clone();
        vmf.entities.push(source.clone());
        crate::SceneMesh::build_with_models(
            &vmf,
            &game,
            &mut crate::MaterialLibrary::default(),
            &HashSet::from([ObjectId::Entity(7)]),
            &HashSet::new(),
            &[],
            &[instance],
        )
    };
    let scaled = mesh(&source, instance);
    let expected = [
        [2.0, 24.0, 42.0],
        [2.0, 32.0, 42.0],
        [-6.0, 32.0, 42.0],
        [-6.0, 24.0, 42.0],
    ];
    for (vertex, point) in scaled.vertices.iter().zip(expected) {
        assert!(
            glam::Vec3::from_array(vertex.position)
                .abs_diff_eq(glam::Vec3::from_array(point), 1e-5)
        );
        assert_eq!(vertex.color, [96, 105, 150, 0]);
        assert_eq!(vertex.opacity, 128.0 / 255.0);
        assert_eq!(vertex.texture_alpha, 1);
        assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
    }
    assert_eq!(scaled.vertices.len(), 4);
    assert!(
        scaled
            .wire_lines
            .iter()
            .all(|line| expected.contains(&line.position))
    );
    assert!(!scaled.selection_outline_ranges.is_empty());
    source.properties.remove("modelscale");
    let default = mesh(&source, Instance::new(&source, asset.clone()).unwrap());
    source.properties.set("modelscale", "1");
    let unit = mesh(&source, Instance::new(&source, asset.clone()).unwrap());
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&default.vertices),
        bytemuck::cast_slice::<_, u8>(&unit.vertices)
    );
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&default.wire_lines),
        bytemuck::cast_slice::<_, u8>(&unit.wire_lines)
    );
    assert_eq!(default.indices, unit.indices);
    assert_eq!(default.lines, unit.lines);
}

#[test]
fn surface_shading_model_normals_are_world_space_and_independent_of_uniform_scale() {
    let asset = offset_asset(false);
    let game = material_game(std::path::Path::new("/nonexistent-model-normal-fixture"));
    for scale in ["0.015625", "0.5", "2", "64"] {
        let mut source = entity(Some(scale));
        source.properties.set("angles", "0 90 90");
        let instance = Instance::new(&source, asset.clone()).unwrap();
        let mut vmf = Document::empty().vmf().clone();
        vmf.entities.push(source);
        let mesh = crate::SceneMesh::build_with_models(
            &vmf,
            &game,
            &mut crate::MaterialLibrary::default(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            &[instance],
        );
        assert_eq!(mesh.vertices.len(), 4);
        for vertex in mesh.vertices {
            let normal = glam::Vec3::from_array(vertex.normal);
            assert!(normal.abs_diff_eq(glam::Vec3::X, 1e-6), "{scale}: {normal}");
            assert!((normal.length_squared() - 1.0).abs() < 1e-6);
        }
    }
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn rendered_model_scale_keeps_default_pixels_and_changes_silhouette_coverage() {
    let directory = tempfile::tempdir().unwrap();
    let game = material_game(directory.path());
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/scale.vmt"),
        "VertexLitGeneric { $basetexture scale }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/scale.vtf"),
        opacity_texture([255, 80, 40, 255]),
    )
    .unwrap();
    let context = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&context);
    let mut materials = crate::MaterialLibrary::default();
    let mut render = |raw: Option<&str>| {
        let (mut source, instance) = opacity_model(2, 0.0, "0", "255", "scale");
        if let Some(raw) = raw {
            source.properties.set("modelscale", raw);
        }
        let instance = Instance::new(&source, instance.asset).unwrap();
        opacity_pixels(
            &context,
            &mut renderer,
            &mut materials,
            &game,
            &[(source, instance)],
        )
    };
    let normal = render(None);
    assert_eq!(normal, render(Some("1")));
    assert_eq!(normal, render(Some("1.00000001")));
    let small = render(Some("0.5"));
    let large = render(Some("2"));
    let blank = opacity_pixels(&context, &mut renderer, &mut materials, &game, &[]);
    let coverage = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(blank.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count()
    };
    assert!(coverage(&small) > 0);
    assert!(coverage(&small) < coverage(&normal));
    assert!(coverage(&normal) < coverage(&large));
}
