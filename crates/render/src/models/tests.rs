use super::*;
use source_formats::{
    mdl::ModelHeader,
    model::{Part, Vertex},
};

fn asset() -> Arc<Asset> {
    Arc::new(Asset {
        path: "models/test.mdl".into(),
        materials: vec!["dev/test".into()],
        model: Model {
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
    })
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
fn blank_bodygroups_keep_helpers_and_visible_variants_use_only_their_meshes() {
    use source_formats::model::{BodyGroup, BodyVariant};
    let mut asset = Arc::try_unwrap(asset()).unwrap();
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
                bounds: Some(asset.model.bounds),
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
