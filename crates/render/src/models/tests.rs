use super::*;
use map_core::Document;
use source_formats::{
    mdl::ModelHeader,
    model::{Part, Vertex},
};
use std::collections::HashSet;

mod lighting_tests;
mod scale_tests;

fn asset() -> Arc<Asset> {
    Arc::new(Asset::new(
        "models/test.mdl".into(),
        Model {
            reference_bones: 0,
            body_groups: Vec::new(),
            attachments: Vec::new(),
            header: ModelHeader {
                checksum: 0,
                flags: 16,
                hull_mins: [-32.0, -16.0, 0.0],
                hull_maxs: [32.0, 16.0, 0.0],
                render_mins: [-32.0, -16.0, 0.0],
                render_maxs: [32.0, 16.0, 0.0],
                illumination: [0.0; 3],
                skins: 1,
            },
            vertices: [
                [-32.0, -16.0, 0.0],
                [32.0, -16.0, 0.0],
                [32.0, 16.0, 0.0],
                [-32.0, 16.0, 0.0],
            ]
            .into_iter()
            .map(|position| Vertex {
                position,
                normal: [0.0, 0.0, 1.0],
                uv: [0.0; 2],
            })
            .collect(),
            parts: vec![Part {
                material: 0,
                indices: vec![0, 2, 1, 0, 3, 2],
            }],
            materials: vec!["test".into()],
            material_directories: vec!["dev".into()],
            skins: vec![vec![0]],
            bounds: [[-32.0, -16.0, 0.0], [32.0, 16.0, 0.0]],
            body_parts: 1,
        },
        Ok(vec!["dev/test".into()]),
    ))
}

fn cached_asset(asset: Arc<Asset>) -> Library {
    let bytes = asset.memory_bytes();
    Library {
        entries: BTreeMap::from([(
            asset.path.clone(),
            Entry {
                result: Ok(asset),
                used: 0,
                bytes,
                material_bindings: true,
            },
        )]),
        bytes,
        ..Library::default()
    }
}

#[test]
fn geometry_only_cache_skips_material_reload_until_a_normal_model_requests_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let fs = material_game(directory.path());
    let initial = asset();
    *initial.materials.write().unwrap() = Err("Deferred editor guide bindings".into());
    let mut library = cached_asset(initial.clone());
    library
        .entries
        .get_mut(&initial.path)
        .unwrap()
        .material_bindings = false;
    assert_eq!(library.reload_materials(&fs).refreshed, 0);
    assert!(library.reload_materials(&fs).materials.is_empty());
    assert!(Arc::ptr_eq(
        &initial,
        &library.get_geometry(&fs, &initial.path).unwrap()
    ));
    assert!(initial.materials().is_err());
    assert!(!library.entries[&initial.path].material_bindings);
    assert!(Arc::ptr_eq(
        &initial,
        &library.get(&fs, &initial.path).unwrap()
    ));
    assert!(library.entries[&initial.path].material_bindings);
    assert_eq!(&*initial.materials().unwrap(), &["test"]);
    assert_eq!(library.bytes, initial.memory_bytes());
    assert_eq!(library.reload_materials(&fs).refreshed, 1);
}

fn material_game(directory: &std::path::Path) -> GameFs {
    GameFs {
        game_dir: directory.into(),
        base_dir: directory.into(),
        mounts: vec![source_formats::gamefs::MountedPath {
            ids: vec!["game".into()],
            mount: source_formats::gamefs::Mount::Directory(directory.into()),
        }],
    }
}

#[test]
fn material_reload_reselects_preferred_missing_and_deleted_paths_without_copying_geometry() {
    let directory = tempfile::tempdir().unwrap();
    let fs = material_game(directory.path());
    let materials = directory.path().join("materials");
    std::fs::create_dir_all(materials.join("dev")).unwrap();
    std::fs::create_dir_all(materials.join("preferred")).unwrap();
    std::fs::write(materials.join("dev/test.vmt"), "VertexLitGeneric {}").unwrap();
    let mut initial = asset();
    Arc::get_mut(&mut initial)
        .unwrap()
        .model
        .material_directories = vec!["preferred".into(), "dev".into()];
    let mut library = cached_asset(initial.clone());
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    let original = Instance::new(&entity, initial.clone()).unwrap();
    let ray = Ray {
        origin: DVec3::Z * 64.0,
        direction: DVec3::NEG_Z,
    };
    for (file, create, expected) in [
        ("preferred/test.vmt", true, "preferred/test"),
        ("preferred/test.vmt", false, "dev/test"),
        ("dev/test.vmt", false, "test"),
        ("dev/test.vmt", true, "dev/test"),
    ] {
        if create {
            std::fs::write(materials.join(file), "VertexLitGeneric {}").unwrap();
        } else {
            std::fs::remove_file(materials.join(file)).unwrap();
        }
        let report = library.reload_materials(&fs);
        assert_eq!((report.refreshed, report.changed), (1, 1));
        assert!(report.failures.is_empty());
        assert_eq!(report.materials, [expected]);
        let current = library.get(&fs, &initial.path).unwrap();
        assert_eq!(&*current.materials().unwrap(), &[expected]);
        assert!(Arc::ptr_eq(&initial, &current));
        assert_eq!(library.bytes, current.memory_bytes());
        assert_eq!(
            current.model.vertices.as_ptr(),
            initial.model.vertices.as_ptr()
        );
        assert_eq!(
            current.model.parts[0].indices.as_ptr(),
            initial.model.parts[0].indices.as_ptr()
        );
        let instance = Instance::new(&entity, current).unwrap();
        assert_eq!(instance.bounds(), original.bounds());
        assert_eq!(instance.pick(&ray), original.pick(&ray));
        assert!(library.bytes <= MAX_CACHE_BYTES);
    }
    let current = library.get(&fs, &initial.path).unwrap();
    assert_eq!(library.reload_materials(&fs).changed, 0);
    assert!(Arc::ptr_eq(
        &current,
        &library.get(&fs, &initial.path).unwrap()
    ));
    drop(current);
    library.entries.get_mut(&initial.path).unwrap().used = 0;
    for index in 1..MAX_CACHE_ENTRIES {
        library.entries.insert(
            format!("models/unsupported{index}.mdl"),
            Entry {
                result: Err("Unsupported cached geometry".into()),
                used: index as u64,
                bytes: 0,
                material_bindings: true,
            },
        );
    }
    assert!(library.get(&fs, "models/new_missing.mdl").is_err());
    assert!(Arc::ptr_eq(
        &initial,
        &library.get(&fs, &initial.path).unwrap()
    ));
    assert_eq!(
        library.get(&fs, "models/unsupported2.mdl").unwrap_err(),
        "Unsupported cached geometry"
    );
}

#[cfg(unix)]
#[test]
fn failed_material_resolution_discards_old_bindings_and_repairs_with_the_same_geometry() {
    let directory = tempfile::tempdir().unwrap();
    let fs = material_game(directory.path());
    let materials = directory.path().join("materials");
    std::fs::create_dir_all(&materials).unwrap();
    std::os::unix::fs::symlink("dev", materials.join("dev")).unwrap();
    let initial = asset();
    let mut library = cached_asset(initial.clone());
    let report = library.reload_materials(&fs);
    assert_eq!((report.refreshed, report.changed), (1, 1));
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].0, initial.path);
    assert!(report.failures[0].1.contains("dev/test"));
    let current = library.get(&fs, &initial.path).unwrap();
    assert!(current.materials().is_err());
    assert!(Arc::ptr_eq(&initial, &current));
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    let instance = Instance::new(&entity, current).unwrap();
    let mut vmf = Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "");
    vmf.entities.push(entity.clone());
    let mut display = crate::MaterialLibrary::default();
    let scene = crate::SceneMesh::build_with_models(
        &vmf,
        &fs,
        &mut display,
        &Default::default(),
        &Default::default(),
        &[],
        &[instance],
    );
    assert_eq!(scene.stats.model_entities, 1);
    assert_eq!(scene.stats.missing_materials, 1);
    assert_eq!(scene.indices, initial.model.parts[0].indices);
    assert!(display.get(scene.batches[0].material).missing);
    assert_eq!(display.reload(&fs).failed, 0);
    std::fs::remove_file(materials.join("dev")).unwrap();
    std::fs::create_dir(materials.join("dev")).unwrap();
    std::fs::write(materials.join("dev/test.vmt"), "VertexLitGeneric {}").unwrap();
    let report = library.reload_materials(&fs);
    assert!(report.failures.is_empty());
    assert_eq!(report.changed, 1);
    let repaired = library.get(&fs, &initial.path).unwrap();
    assert_eq!(&*repaired.materials().unwrap(), &["dev/test"]);
    assert!(Arc::ptr_eq(&initial, &repaired));
    assert_eq!(
        Instance::new(&entity, repaired).unwrap().bounds(),
        Instance::new(&entity, initial).unwrap().bounds()
    );
}

#[test]
fn retained_assets_share_reloaded_bindings_and_budget_failures_clear_them_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let fs = material_game(directory.path());
    let materials = directory.path().join("materials");
    let preferred = "a_longer_preferred_material_directory";
    std::fs::create_dir_all(materials.join("dev")).unwrap();
    std::fs::create_dir_all(materials.join(preferred)).unwrap();
    std::fs::write(materials.join("dev/test.vmt"), "VertexLitGeneric {}").unwrap();
    let mut original = Arc::try_unwrap(asset()).unwrap();
    original.model.materials = vec!["test".into(); 4096];
    original.model.material_directories = vec![preferred.into(), "dev".into()];
    let initial = Arc::new(Asset::new(
        original.path,
        original.model,
        Ok(vec!["dev/test".into(); 4096]),
    ));
    let mut library = cached_asset(initial.clone());
    let mut retained = vec![initial.clone()];
    for _ in 0..4 {
        std::fs::write(
            materials.join(preferred).join("test.vmt"),
            "VertexLitGeneric {}",
        )
        .unwrap();
        library.reload_materials(&fs);
        let current = library.get(&fs, &initial.path).unwrap();
        retained.push(current);
        assert!(retained.iter().all(|asset| Arc::ptr_eq(asset, &initial)));
        assert!(
            retained
                .iter()
                .all(|asset| asset.materials().unwrap()[0] == format!("{preferred}/test"))
        );
        assert_eq!(library.bytes, initial.memory_bytes());
        std::fs::remove_file(materials.join(preferred).join("test.vmt")).unwrap();
        library.reload_materials(&fs);
        assert_eq!(library.bytes, initial.memory_bytes());
    }
    library.bytes = MAX_CACHE_BYTES;
    std::fs::write(
        materials.join(preferred).join("test.vmt"),
        "VertexLitGeneric {}",
    )
    .unwrap();
    let report = library.reload_materials(&fs);
    assert!(report.failures[0].1.contains("cache budget"));
    assert!(retained.iter().all(|asset| asset.materials().is_err()));
    assert!(library.bytes <= MAX_CACHE_BYTES);
    assert_eq!(initial.model.vertices.len(), 4);
}

#[test]
fn poisoned_material_bindings_report_failure_without_exposing_previous_paths() {
    let initial = asset();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _bindings = initial.materials.write().unwrap();
        panic!("interrupted update");
    }));
    assert_eq!(initial.materials().unwrap_err(), MATERIAL_BINDINGS_ERROR);
    let directory = tempfile::tempdir().unwrap();
    let mut library = cached_asset(initial.clone());
    let report = library.reload_materials(&material_game(directory.path()));
    assert_eq!(
        report.failures,
        [(initial.path.clone(), MATERIAL_BINDINGS_ERROR.into())]
    );
    assert!(Arc::ptr_eq(
        &initial,
        &library
            .get(&material_game(directory.path()), &initial.path)
            .unwrap()
    ));
}

#[test]
fn model_tints_modulate_vertices_and_keep_black_selection_visible() {
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    entity.properties.set("rendercolor", "64 128 255 032");
    let instance = Instance::new(&entity, asset()).unwrap();
    assert_eq!(instance.tint, [64, 128, 255]);
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.entities.push(entity.clone());
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        &game,
        &mut crate::MaterialLibrary::default(),
        &Default::default(),
        &Default::default(),
        &[],
        &[instance],
    );
    assert!(
        mesh.vertices
            .iter()
            .all(|vertex| vertex.color == [64, 128, 255, 0])
    );
    entity.properties.set("rendercolor", "0 0 0");
    let instance = Instance::new(&entity, asset()).unwrap();
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        &game,
        &mut crate::MaterialLibrary::default(),
        &[ObjectId::Entity(2)].into_iter().collect(),
        &Default::default(),
        &[],
        &[instance],
    );
    assert!(
        mesh.vertices
            .iter()
            .all(|vertex| vertex.color == [96, 80, 64, 0])
    );
    entity.properties.set("classname", "prop_static");
    assert!(
        Instance::new(&entity, asset())
            .err()
            .unwrap()
            .contains("Tinted static props")
    );
}

#[test]
fn model_texture_opacity_keeps_geometry_tint_picking_and_guide_appearance_independent() {
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    entity.properties.set("rendercolor", "64 128 255 032");
    entity.properties.set("rendermode", "0");
    entity.properties.set("renderamt", "128");
    let normal = Instance::new(&entity, asset()).unwrap();
    let ray = Ray {
        origin: DVec3::new(0.0, 0.0, 32.0),
        direction: DVec3::NEG_Z,
    };
    let expected_hit = normal.pick(&ray).unwrap();
    let expected_bounds = normal.bounds();
    let mut vmf = Document::empty().vmf().clone();
    vmf.entities.push(entity.clone());
    let game = material_game(std::path::Path::new("/nonexistent-model-opacity-fixture"));
    for amount in [0, 128, 255] {
        entity.properties.set("rendermode", "2");
        entity.properties.set("renderamt", amount.to_string());
        let instance = Instance::new(&entity, asset()).unwrap();
        assert_eq!(instance.bounds(), expected_bounds);
        assert_eq!(instance.pick(&ray).unwrap(), expected_hit);
        let mesh = crate::SceneMesh::build_with_models(
            &vmf,
            &game,
            &mut crate::MaterialLibrary::default(),
            &HashSet::from([ObjectId::Entity(2)]),
            &HashSet::new(),
            &[],
            &[instance],
        );
        assert_eq!(mesh.stats.model_entities, 1);
        assert_eq!(mesh.indices.len(), 6);
        assert!(mesh.batches.iter().all(|batch| batch.model_translucent));
        assert!(mesh.vertices.iter().all(|vertex| {
            vertex.opacity == amount as f32 / 255.0
                && vertex.texture_alpha == 1
                && vertex.color == [96, 105, 150, 0]
        }));
        assert!(!mesh.selection_outline_ranges.is_empty());
    }
    entity.properties.set("rendermode", "5");
    assert!(
        Instance::new(&entity, asset())
            .err()
            .unwrap()
            .contains("mode 5")
    );
    let guide = Instance::editor_guide(&entity, asset()).unwrap();
    assert_eq!(guide.appearance.opacity(), 1.0);
    assert_eq!(guide.tint, normal.tint);
    entity.properties.set("rendermode", "2");
    entity.properties.set("classname", "prop_static");
    entity.properties.set("rendercolor", "255 255 255");
    assert!(
        Instance::new(&entity, asset())
            .err()
            .unwrap()
            .contains("Static-prop output")
    );
}

#[test]
fn transparent_model_batches_count_each_missing_material_once() {
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    entity.properties.set("rendermode", "2");
    let mut model = asset();
    let parts = &mut Arc::get_mut(&mut model).unwrap().model.parts;
    parts.push(Part {
        material: parts[0].material,
        indices: parts[0].indices.clone(),
    });
    let instance = Instance::new(&entity, model).unwrap();
    let mut vmf = Document::empty().vmf().clone();
    vmf.entities.push(entity);
    let directory = tempfile::tempdir().unwrap();
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        &material_game(directory.path()),
        &mut crate::MaterialLibrary::default(),
        &HashSet::new(),
        &HashSet::new(),
        &[],
        &[instance],
    );
    assert_eq!(mesh.batches.len(), 2);
    assert_eq!(mesh.stats.missing_materials, 1);
}

fn opacity_texture(pixel: [u8; 4]) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7u32), (8, 2), (12, 80), (57, u32::MAX)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    bytes[56] = 1;
    bytes.extend(pixel);
    bytes
}

fn opacity_model(
    id: u32,
    height: f64,
    mode: &str,
    amount: &str,
    material: &str,
) -> (Entity, Instance) {
    let mut entity = Entity::new(id, "prop_dynamic");
    entity.properties.set("origin", format!("0 0 {height}"));
    entity.properties.set("rendermode", mode);
    entity.properties.set("renderamt", amount);
    let model = asset();
    *model.materials.write().unwrap() = Ok(vec![material.into()]);
    let instance = Instance::new(&entity, model).unwrap();
    (entity, instance)
}

fn opacity_pixels(
    context: &crate::GpuContext,
    renderer: &mut crate::Renderer,
    materials: &mut crate::MaterialLibrary,
    game: &GameFs,
    models: &[(Entity, Instance)],
) -> Vec<u8> {
    let mut vmf = Document::empty().vmf().clone();
    vmf.entities
        .extend(models.iter().map(|(entity, _)| entity.clone()));
    let instances = models
        .iter()
        .map(|(_, instance)| instance.clone())
        .collect::<Vec<_>>();
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        game,
        materials,
        &HashSet::new(),
        &HashSet::new(),
        &[],
        &instances,
    );
    let scene = renderer.upload(context, &mesh, materials);
    renderer.render_to_rgba(
        context,
        &scene,
        &crate::Camera::looking_at(glam::Vec3::new(0.0, 0.0, 96.0), glam::Vec3::ZERO),
        &crate::ViewOptions {
            grid_visible: false,
            ..Default::default()
        },
        65,
        65,
    )
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn rendered_model_opacity_preserves_texture_alpha_depth_and_sorted_overlap() {
    let directory = tempfile::tempdir().unwrap();
    let game = material_game(directory.path());
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    for (name, parameters, pixel) in [
        ("opaque", "", [255, 80, 40, 128]),
        ("translucent", "$translucent 1", [255, 80, 40, 128]),
        ("cutout", "$alphatest 1", [255, 80, 40, 127]),
        ("cutout_edge", "$alphatest 1", [255, 80, 40, 128]),
        ("red", "", [255, 0, 0, 255]),
        ("green", "", [0, 255, 0, 255]),
        ("blue", "", [0, 0, 255, 255]),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/{name}.vmt")),
            format!("VertexLitGeneric {{ $basetexture {name} {parameters} }}"),
        )
        .unwrap();
        std::fs::write(
            directory.path().join(format!("materials/{name}.vtf")),
            opacity_texture(pixel),
        )
        .unwrap();
    }
    let context = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&context);
    let mut materials = crate::MaterialLibrary::default();
    let mut render = |models: &[(Entity, Instance)]| {
        opacity_pixels(&context, &mut renderer, &mut materials, &game, models)
    };
    let blank = render(&[]);
    let normal = render(&[opacity_model(2, 0.0, "0", "0", "opaque")]);
    assert_ne!(normal, blank);
    assert_eq!(
        normal,
        render(&[opacity_model(2, 0.0, "0", "128", "opaque")])
    );
    assert_eq!(
        normal,
        render(&[opacity_model(2, 0.0, "0", "255", "opaque")])
    );
    let source_alpha = render(&[opacity_model(2, 0.0, "0", "255", "translucent")]);
    assert_ne!(source_alpha, normal);
    assert_eq!(
        source_alpha,
        render(&[opacity_model(2, 0.0, "2", "255", "opaque")])
    );
    assert_eq!(
        source_alpha,
        render(&[opacity_model(2, 0.0, "2", "255", "translucent")])
    );
    let half = render(&[opacity_model(2, 0.0, "2", "128", "opaque")]);
    assert_ne!(half, source_alpha);
    assert_ne!(half, blank);
    assert_eq!(
        half,
        render(&[opacity_model(2, 0.0, "2", "128", "translucent")])
    );
    assert_eq!(blank, render(&[opacity_model(2, 0.0, "2", "0", "opaque")]));
    assert_eq!(
        blank,
        render(&[opacity_model(2, 0.0, "0", "255", "cutout")])
    );
    assert_eq!(
        blank,
        render(&[opacity_model(2, 0.0, "2", "128", "cutout")])
    );
    assert_eq!(
        normal,
        render(&[opacity_model(2, 0.0, "0", "255", "cutout_edge")])
    );
    assert_eq!(
        half,
        render(&[opacity_model(2, 0.0, "2", "128", "cutout_edge")])
    );
    let front = opacity_model(2, 16.0, "0", "255", "blue");
    let obscured = opacity_model(3, 0.0, "2", "128", "red");
    assert_eq!(
        render(std::slice::from_ref(&front)),
        render(&[front, obscured])
    );
    let far = opacity_model(2, 0.0, "2", "128", "red");
    let near = opacity_model(3, 8.0, "2", "128", "green");
    let ordered = render(&[far.clone(), near.clone()]);
    assert_eq!(ordered, render(&[near, far]));
    let center = (32 * 65 + 32) * 4;
    assert!(ordered[center + 1] > ordered[center]);
}

#[test]
fn model_surface_hits_use_world_triangle_normals() {
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    entity.properties.set("angles", "0 90 0");
    let instance = Instance::new(&entity, asset()).unwrap();
    let ray = Ray {
        origin: DVec3::new(0.0, 0.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    let surface = instance.pick_surface(&ray).unwrap();
    assert!(surface.normal.abs_diff_eq(DVec3::Z, 1e-10));
    assert_eq!(instance.pick(&ray), Some(surface.hit));
}

#[test]
fn attachments_compose_entity_rotations_and_native_world_alignment() {
    let mut asset = asset();
    let model = &mut Arc::get_mut(&mut asset).unwrap().model;
    for flags in [0, 0x10000] {
        model.attachments.push(source_formats::model::Attachment {
            name: "point".into(),
            bone: 0,
            flags,
            transform: glam::DAffine3::from_rotation_translation(
                DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
                DVec3::new(4.0, 2.0, 8.0),
            ),
        });
    }
    let mut entity = Entity::new(2, "prop_dynamic");
    entity.properties.set("origin", "100 200 20");
    entity.properties.set("angles", "0 90 0");
    let instance = Instance::new(&entity, asset).unwrap();
    let normal = instance.attachment(1).unwrap();
    let aligned = instance.attachment(2).unwrap();
    assert!(
        normal
            .translation
            .abs_diff_eq(DVec3::new(98.0, 204.0, 28.0), 1e-10)
    );
    assert_eq!(aligned.translation, normal.translation);
    assert!(normal.matrix3.x_axis.abs_diff_eq(-DVec3::X, 1e-10));
    assert_eq!(aligned.matrix3, DMat3::IDENTITY);
    assert!(instance.attachment(0).is_none());
    assert!(instance.attachment(3).is_none());
}

#[test]
fn model_instances_share_source_angles_bounds_front_faces_and_actual_mesh_picks() {
    let mut entity = Entity::new(7, "prop_static");
    entity.properties.set("origin", "100 200 32");
    entity.properties.set("angles", "0 90 0");
    let instance = Instance::new(&entity, asset()).unwrap();
    let (mins, maxs) = instance.bounds();
    assert!(mins.abs_diff_eq(DVec3::new(84.0, 168.0, 32.0), 1e-10));
    assert!(maxs.abs_diff_eq(DVec3::new(116.0, 232.0, 32.0), 1e-10));
    let mut angled = entity.clone();
    angled.properties.set("angles", "0 45 0");
    let angled = Instance::new(&angled, asset()).unwrap();
    let basis = glam::DMat3::from_quat(angled.rotation);
    let (low, high) = angled.bounds_in_basis(basis);
    let center = basis.transpose() * angled.origin;
    assert!(low.abs_diff_eq(center - DVec3::new(32.0, 16.0, 0.0), 1e-10));
    assert!(high.abs_diff_eq(center + DVec3::new(32.0, 16.0, 0.0), 1e-10));
    let ray = Ray {
        origin: DVec3::new(100.0, 222.0, 100.0),
        direction: DVec3::NEG_Z * 5.0,
    };
    let hit = instance.pick(&ray).unwrap();
    assert_eq!(hit.object, ObjectId::Entity(7));
    assert!((hit.distance - 68.0).abs() < 1e-10);
    assert!(hit.point.abs_diff_eq(DVec3::new(100.0, 222.0, 32.0), 1e-10));
    assert!(
        instance
            .pick(&Ray {
                origin: DVec3::new(120.0, 222.0, 100.0),
                ..ray
            })
            .is_none()
    );
    assert!(
        instance
            .pick(&Ray {
                origin: DVec3::new(100.0, 222.0, 0.0),
                direction: DVec3::Z
            })
            .is_none()
    );
    assert!(
        instance
            .pick(&Ray {
                direction: DVec3::splat(f64::NAN),
                ..ray
            })
            .is_none()
    );
    for angle in ["-1", "-2", "45"] {
        entity.properties.remove("angles");
        entity.properties.set("angle", angle);
        assert!(Instance::new(&entity, asset()).is_ok());
    }
    for (key, value) in [
        ("skin", "1"),
        ("skin", "-1"),
        ("body", "1"),
        ("modelscale", "2"),
        ("origin", "65536 0 0"),
    ] {
        let mut invalid = entity.clone();
        invalid.properties.set(key, value);
        assert!(Instance::new(&invalid, asset()).is_err(), "{key}={value}");
    }
}

#[test]
fn model_scene_uses_material_batches_normals_and_wire_geometry_without_a_helper_cube() {
    let mut entity = Entity::new(7, "prop_static");
    entity.properties.set("origin", "0 0 32");
    let instance = Instance::new(&entity, asset()).unwrap();
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.entities.push(entity);
    let fs = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        &fs,
        &mut crate::MaterialLibrary::default(),
        &Default::default(),
        &Default::default(),
        &[],
        &[instance],
    );
    assert_eq!(mesh.stats.point_entities, 1);
    assert_eq!(mesh.stats.model_entities, 1);
    assert_eq!(mesh.vertices.len(), 4);
    assert_eq!(mesh.indices, [0, 2, 1, 0, 3, 2]);
    assert_eq!(mesh.wire_lines.len(), 12);
    assert!(
        mesh.vertices
            .iter()
            .all(|vertex| vertex.normal == [0.0, 0.0, 1.0])
    );
}

#[test]
fn fgd_model_guides_replace_filled_boxes_without_expanding_selection_or_scene_bounds() {
    let mut entity = Entity::new(7, "sample");
    entity.properties.set("origin", "0 0 32");
    let guide_asset = asset();
    *guide_asset.materials.write().unwrap() = Err("Deferred editor guide bindings".into());
    let instance = Instance::new(&entity, guide_asset).unwrap();
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.entities.push(entity);
    let fs = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let selected = HashSet::from([ObjectId::Entity(7)]);
    let helpers = BTreeMap::from([(
        ObjectId::Entity(7),
        map_core::entity_helpers::Appearance {
            local_bounds: Some([DVec3::splat(-1.0), DVec3::splat(1.0)]),
            ..Default::default()
        },
    )]);
    let baseline = crate::SceneMesh::build_with_helpers_filtered(
        &vmf,
        &fs,
        &mut crate::MaterialLibrary::default(),
        (&selected, &HashSet::new()),
        &[],
        (&[], &helpers),
        |_| true,
    );
    let guides = [instance];
    let mut materials = crate::MaterialLibrary::default();
    let material_count = materials.len();
    let mesh = crate::SceneMesh::build_with_visual_helpers_filtered(
        &vmf,
        &fs,
        &mut materials,
        (&selected, &HashSet::new()),
        &[],
        (&[], &helpers),
        (&BTreeMap::new(), &guides),
        |_| true,
    );
    assert_eq!((mesh.mins, mesh.maxs), (baseline.mins, baseline.maxs));
    assert_eq!(mesh.lines, baseline.lines);
    assert_eq!(
        mesh.selection_outline_ranges,
        baseline.selection_outline_ranges
    );
    assert_eq!(mesh.stats.model_entities, 0);
    assert_eq!(mesh.stats.fgd_model_helpers, 1);
    assert_eq!(mesh.vertices.len(), 4);
    assert_eq!(mesh.indices.len(), 6);
    assert_eq!(materials.len(), material_count);
    assert!(
        mesh.batches
            .iter()
            .all(|batch| batch.material == crate::MaterialLibrary::EDITOR)
    );
    let hidden = crate::SceneMesh::build_with_visual_helpers_filtered(
        &vmf,
        &fs,
        &mut crate::MaterialLibrary::default(),
        (&selected, &HashSet::new()),
        &[],
        (&[], &helpers),
        (&BTreeMap::new(), &guides),
        |_| false,
    );
    assert!(hidden.vertices.is_empty());
    assert_eq!(hidden.stats.fgd_model_helpers, 0);
}

#[test]
fn selection_outlines_classify_model_brackets_without_replacing_meshes_with_helpers() {
    let mut entity = Entity::new(7, "prop_static");
    entity.properties.set("origin", "0 0 32");
    let instance = Instance::new(&entity, asset()).unwrap();
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.entities.push(entity);
    let fs = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mesh = crate::SceneMesh::build_with_models(
        &vmf,
        &fs,
        &mut crate::MaterialLibrary::default(),
        &std::collections::HashSet::from([ObjectId::Entity(7)]),
        &Default::default(),
        &[],
        &[instance],
    );
    assert_eq!(mesh.stats.model_entities, 1);
    assert_eq!(mesh.vertices.len(), 4);
    assert_eq!(mesh.indices, [0, 2, 1, 0, 3, 2]);
    assert_eq!(mesh.lines.len(), 48);
    assert_eq!(mesh.selection_outline_ranges.len(), 1);
    assert_eq!(mesh.selection_outline_ranges[0], 0..48);
    assert!(mesh.guide_line_ranges().is_empty());
}

#[test]
fn blank_bodygroups_keep_helpers_and_visible_variants_use_only_their_meshes() {
    use source_formats::model::{BodyGroup, BodyVariant};
    let mut asset = Arc::try_unwrap(asset()).unwrap();
    let bounds = asset.model.bounds;
    asset.model.body_groups = vec![BodyGroup {
        name: "Appearance".into(),
        base: 1,
        variants: vec![
            BodyVariant {
                name: "blank".into(),
                parts: 0..0,
                bounds: None,
            },
            BodyVariant {
                name: "quad".into(),
                parts: 0..1,
                bounds: Some(bounds),
            },
        ],
    }];
    let asset = Arc::new(asset);
    let mut entity = Entity::new(7, "prop_dynamic");
    entity.properties.set("origin", "0 0 32");
    let blank = Instance::new(&entity, asset.clone()).unwrap();
    assert!(!blank.has_geometry());
    assert_eq!(
        blank.bounds(),
        map_core::query::entity_bounds(&entity).unwrap()
    );
    let ray = Ray {
        origin: DVec3::new(0.0, 0.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    assert!(blank.pick(&ray).is_none());
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.entities.push(entity.clone());
    let fs = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    let mut materials = crate::MaterialLibrary::default();
    let scene = crate::SceneMesh::build_with_models(
        &vmf,
        &fs,
        &mut materials,
        &Default::default(),
        &Default::default(),
        &[],
        &[blank],
    );
    assert_eq!(scene.stats.model_entities, 0);
    assert_eq!(scene.vertices.len(), 24);
    entity.properties.set("body", "1");
    let visible = Instance::new(&entity, asset).unwrap();
    assert!(visible.has_geometry());
    assert!(visible.pick(&ray).is_some());
    assert!(visible.bounds().1.x > 8.0);
}
