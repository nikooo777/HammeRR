use super::*;
use crate::Report;
use crate::map::Map;
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::gamefs::GameFs;

fn vertices(alphas: &[f32]) -> Vec<DisplacementVertex> {
    alphas
        .iter()
        .map(|&alpha| DisplacementVertex {
            alpha,
            ..Default::default()
        })
        .collect()
}

#[test]
fn endpoints_threshold_and_native_float_rounding_match_sdk_selection() {
    let rounded = 127.500_000_1_f64 as f32;
    assert_eq!(rounded, 127.5);
    for (alphas, expected) in [
        ([0.0; 3], 2),
        ([255.0; 3], 3),
        ([127.5; 3], 2),
        ([127.5_f32.next_down(); 3], 2),
        ([127.5_f32.next_up(); 3], 3),
        ([rounded; 3], 2),
        ([255.0, 127.5, 0.0], 2),
        ([255.0, 127.5_f32.next_up(), 0.0], 2),
        ([255.0, 127.500_04, 0.0], 3),
        ([-255.0; 3], 2),
        ([300.0; 3], 3),
    ] {
        let mut names = vec!["default".into(), "ice".into()];
        assert_eq!(
            materials(
                &vertices(&alphas),
                &[[0, 1, 2]],
                2,
                Some("rubber"),
                &mut names,
                &Control::default(),
            )
            .unwrap(),
            [expected],
            "{alphas:?}"
        );
        assert_eq!(names.len(), usize::from(expected));
    }
}

#[test]
fn missing_secondary_preserves_primary_and_existing_table_order() {
    let mut names = vec!["default".into(), "ice".into()];
    assert_eq!(
        materials(
            &vertices(&[255.0; 4]),
            &[[0, 1, 2], [1, 2, 3]],
            2,
            None,
            &mut names,
            &Control::default(),
        )
        .unwrap(),
        [2, 2]
    );
    assert_eq!(names, ["default", "ice"]);
    let mut names = vec!["default".into(), "rubber".into(), "ice".into()];
    assert_eq!(
        materials(
            &vertices(&[255.0, 255.0, 0.0, 0.0]),
            &[[0, 1, 2], [1, 2, 3], [2, 0, 1]],
            3,
            Some("rubber"),
            &mut names,
            &Control::default(),
        )
        .unwrap(),
        [2, 3, 2]
    );
    assert_eq!(names, ["default", "rubber", "ice"]);
}

#[test]
fn selection_rejects_overflow_invalid_native_data_and_cancellation() {
    let mut names = (0..super::super::MAX_SURFACE_PROPERTIES)
        .map(|index| format!("surface_{index}"))
        .collect::<Vec<_>>();
    let before = names.clone();
    assert!(
        materials(
            &vertices(&[255.0; 3]),
            &[[0, 1, 2]],
            1,
            Some("rubber"),
            &mut names,
            &Control::default(),
        )
        .unwrap_err()
        .contains("127 surface properties")
    );
    assert_eq!(names, before);
    names.pop();
    assert_eq!(
        materials(
            &vertices(&[255.0; 3]),
            &[[0, 1, 2]],
            1,
            Some("rubber"),
            &mut names,
            &Control::default(),
        )
        .unwrap(),
        [127]
    );
    for alphas in [
        [f32::NAN, 0.0, 0.0],
        [f32::INFINITY, 0.0, 0.0],
        [f32::NEG_INFINITY; 3],
    ] {
        assert!(
            materials(
                &vertices(&alphas),
                &[[0, 1, 2]],
                1,
                Some("rubber"),
                &mut names,
                &Control::default(),
            )
            .is_err()
        );
    }
    assert!(
        materials(
            &vertices(&[255.0; 3]),
            &[[0, 1, 3]],
            1,
            Some("rubber"),
            &mut names,
            &Control::default(),
        )
        .unwrap_err()
        .contains("vertex indices")
    );
    assert!(
        materials(
            &[],
            &vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1],
            1,
            None,
            &mut names,
            &Control::default(),
        )
        .unwrap_err()
        .contains("triangle budget")
    );
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        materials(&[], &[[0, 1, 2]], 1, None, &mut names, &control)
            .unwrap_err()
            .contains("cancelled")
    );
}

fn connected_fixture(half: bool) -> source_formats::vmf::Vmf {
    let mut document = Document::empty();
    let bounds = if half {
        vec![
            ([0., 0., 0.], [112., 224., 16.], 3),
            ([112., 0., 0.], [224., 112., 16.], 2),
            ([112., 112., 0.], [224., 224., 16.], 4),
        ]
    } else {
        vec![
            ([0., 0., 0.], [112., 224., 16.], 2),
            ([112., 0., 0.], [224., 224., 16.], 4),
        ]
    };
    document
        .edit("mixed terrain materials", |tx| {
            for (low, high, power) in bounds {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(low),
                    DVec3::from_array(high),
                    "blend",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.document().solid(id).unwrap();
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                let target = map_core::surface::FaceId {
                    solid: id,
                    side: solid.sides[face.side].id,
                };
                map_core::displacement::edit::create(tx, &[target], power, power % 4)?;
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

#[test]
fn full_and_half_lod_materials_ignore_inactive_vertices_at_mixed_powers() {
    let game = GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    for half in [false, true] {
        let source = connected_fixture(half);
        let before = source.clone();
        let mut report = Report::default();
        let mut map = Map::load(&source, &game, &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let stats = map
            .connect_terrain(&mut report, &Control::default())
            .unwrap();
        assert!(stats.disabled_vertices > 0);
        assert_eq!(stats.half_pairs > 0, half);
        let mut checked = 0;
        for terrain in &mut map.terrains {
            let used = terrain
                .indices
                .iter()
                .flatten()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let Some(inactive) = (0..terrain.vertices.len()).find(|index| !used.contains(index))
            else {
                continue;
            };
            for vertex in &mut terrain.vertices {
                vertex.alpha = 127.5;
            }
            terrain.vertices[inactive].alpha = 255.0;
            terrain.mesh.alphas.fill(255.0);
            let mut names = vec!["default".into(), "ice".into()];
            let selected = materials(
                &terrain.vertices,
                &terrain.indices,
                2,
                Some("rubber"),
                &mut names,
                &Control::default(),
            )
            .unwrap();
            assert_eq!(selected, vec![2; terrain.indices.len()]);
            let canonical = terrain.info.triangle_indices().unwrap();
            assert!(canonical.len() > terrain.indices.len());
            assert!(
                materials(
                    &terrain.vertices,
                    &canonical,
                    2,
                    Some("rubber"),
                    &mut names,
                    &Control::default(),
                )
                .unwrap()
                .contains(&3)
            );
            checked += 1;
        }
        assert!(checked > 0);
        assert_eq!(source, before);
    }
}
