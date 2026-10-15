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
fn model_instances_share_source_angles_bounds_front_faces_and_actual_mesh_picks() {
    let mut entity = Entity::new(7, "prop_static");
    entity.properties.set("origin", "100 200 32");
    entity.properties.set("angles", "0 90 0");
    let instance = Instance::new(&entity, asset()).unwrap();
    let (mins, maxs) = instance.bounds();
    assert!(mins.abs_diff_eq(DVec3::new(84.0, 168.0, 32.0), 1e-10));
    assert!(maxs.abs_diff_eq(DVec3::new(116.0, 232.0, 32.0), 1e-10));
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
