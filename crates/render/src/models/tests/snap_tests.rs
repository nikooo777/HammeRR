use super::*;
use source_formats::model::{BodyGroup, BodyVariant};

#[test]
fn mesh_surface_retains_selected_global_part_triangle_and_transformed_vertex() {
    let mut asset = asset();
    let model = &mut Arc::get_mut(&mut asset).unwrap().model;
    model
        .vertices
        .extend(model.vertices.clone().into_iter().map(|mut vertex| {
            vertex.position[2] = 8.0;
            vertex
        }));
    model.parts.push(Part {
        material: 0,
        indices: vec![4, 6, 5, 4, 7, 6],
    });
    model.skins.push(vec![0]);
    model.body_groups = vec![BodyGroup {
        name: "height".into(),
        base: 1,
        variants: vec![
            BodyVariant {
                name: "lower".into(),
                parts: 0..1,
                bounds: Some([[-32.0, -16.0, 0.0], [32.0, 16.0, 0.0]]),
            },
            BodyVariant {
                name: "upper".into(),
                parts: 1..2,
                bounds: Some([[-32.0, -16.0, 8.0], [32.0, 16.0, 8.0]]),
            },
            BodyVariant {
                name: "blank".into(),
                parts: 2..2,
                bounds: None,
            },
        ],
    }];
    let original = model.vertices.clone();
    let mut entity = Entity::new(7, "prop_dynamic");
    entity.properties.set("origin", "10 20 30");
    entity.properties.set("angles", "0 90 0");
    entity.properties.set("modelscale", "2");
    entity.properties.set("body", "1");
    entity.properties.set("skin", "1");
    let instance = Instance::new(&entity, asset.clone()).unwrap();
    let ray = Ray {
        origin: DVec3::new(41.0, -40.0, 150.0),
        direction: DVec3::NEG_Z * 7.0,
    };
    let hit = instance.pick_mesh_surface(&ray).unwrap();
    assert_eq!((hit.part, hit.triangle, hit.vertex), (1, 0, 4));
    assert!(
        hit.position
            .abs_diff_eq(DVec3::new(42.0, -44.0, 46.0), 1e-10)
    );
    assert!(
        hit.surface
            .hit
            .point
            .abs_diff_eq(DVec3::new(41.0, -40.0, 46.0), 1e-10)
    );
    assert_eq!(instance.pick_surface(&ray), Some(hit.surface));
    assert_eq!(instance.pick(&ray), Some(hit.surface.hit));
    assert_eq!(
        asset
            .model
            .indexed_parts_for_body(1)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![1]
    );
    entity.properties.set("body", "0");
    let lower = Instance::new(&entity, asset.clone())
        .unwrap()
        .pick_mesh_surface(&ray)
        .unwrap();
    assert_eq!((lower.part, lower.triangle, lower.vertex), (0, 0, 0));
    assert!(
        lower
            .position
            .abs_diff_eq(DVec3::new(42.0, -44.0, 30.0), 1e-10)
    );
    entity.properties.set("body", "2");
    assert!(
        Instance::new(&entity, asset.clone())
            .unwrap()
            .pick_mesh_surface(&ray)
            .is_none()
    );
    assert_eq!(asset.model.vertices, original);
}

#[test]
fn mesh_vertex_ties_keep_the_existing_first_triangle_and_smallest_global_index() {
    let asset = asset();
    let mut entity = Entity::new(9, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    let instance = Instance::new(&entity, asset).unwrap();
    let ray = Ray {
        origin: DVec3::new(0.0, 0.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    let hit = instance.pick_mesh_surface(&ray).unwrap();
    assert_eq!((hit.part, hit.triangle, hit.vertex), (0, 0, 0));
    assert_eq!(hit.position, DVec3::new(-32.0, -16.0, 0.0));
    assert_eq!(instance.pick_surface(&ray), Some(hit.surface));
}
