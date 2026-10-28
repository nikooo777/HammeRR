use super::*;

fn game() -> GameFs {
    GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    }
}

fn lines_in(mesh: &SceneMesh, ranges: &[std::ops::Range<u32>]) -> Vec<LineVertex> {
    ranges
        .iter()
        .flat_map(|range| {
            mesh.lines[range.start as usize..range.end as usize]
                .iter()
                .copied()
        })
        .collect()
}

#[test]
fn selection_outlines_separate_selected_edges_from_appended_guides_without_reordering_bytes() {
    let mut document = map_core::Document::empty();
    let object = document
        .edit("block", |tx| {
            map_core::edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "test/material")
        })
        .unwrap();
    let mut mesh = SceneMesh::build(
        document.vmf(),
        &game(),
        &mut MaterialLibrary::default(),
        &HashSet::from([object]),
    );
    assert_eq!(mesh.lines.len(), 48);
    assert_eq!(mesh.selection_outline_ranges.len(), 1);
    assert_eq!(mesh.selection_outline_ranges[0], 0..48);
    assert!(mesh.guide_line_ranges().is_empty());
    let original = bytemuck::cast_slice::<LineVertex, u8>(&mesh.lines).to_vec();
    mesh.add_bounds_lines(DVec3::splat(-8.0), DVec3::splat(72.0), [80, 220, 255, 255]);
    assert_eq!(
        bytemuck::cast_slice::<LineVertex, u8>(&mesh.lines[..48]),
        original.as_slice()
    );
    assert_eq!(mesh.guide_line_ranges().len(), 1);
    assert_eq!(mesh.guide_line_ranges()[0], 48..72);
    assert!(
        lines_in(&mesh, &mesh.guide_line_ranges())
            .iter()
            .all(|line| line.color == [80, 220, 255, 255])
    );
    let face = FaceId {
        solid: document.vmf().world.solids[0].id,
        side: document.vmf().world.solids[0].sides[0].id,
    };
    let face_mesh = SceneMesh::build_with_faces(
        document.vmf(),
        &game(),
        &mut MaterialLibrary::default(),
        &HashSet::new(),
        &HashSet::from([face]),
    );
    assert_eq!(face_mesh.lines.len(), 8);
    assert_eq!(face_mesh.selection_outline_ranges.len(), 1);
    assert_eq!(face_mesh.selection_outline_ranges[0], 0..8);
    assert!(face_mesh.guide_line_ranges().is_empty());

    let mut vmf = document.vmf().clone();
    let mut light = Entity::new(100, "light");
    light.properties.set("origin", "256 0 0");
    vmf.entities.push(light);
    let mut replaced = SceneMesh::build_with_reference_models_and_surfaces_filtered(
        &vmf,
        &game(),
        &mut MaterialLibrary::default(),
        (&HashSet::from([object]), &HashSet::new()),
        &[],
        (&[], &BTreeMap::new()),
        (&BTreeMap::new(), &[]),
        None,
        true,
        true,
        false,
        false,
        &[],
        false,
        |_| true,
    );
    assert!(!replaced.vertices.is_empty());
    assert!(
        replaced
            .vertices
            .iter()
            .all(|vertex| vertex.position[0] > 240.0)
    );
    assert_eq!(
        lines_in(&replaced, &replaced.selection_outline_ranges).len(),
        48
    );
    assert!(replaced.wire_lines.is_empty());
    let reference = crate::references::Mesh {
        edges: vec![
            LineVertex {
                position: [0.0; 3],
                color: [0; 4]
            };
            4
        ],
        brushes: vec![
            crate::references::Brush {
                owner: 42,
                vertices: 0..0,
                indices: 0..0,
                edges: 0..2,
            },
            crate::references::Brush {
                owner: 43,
                vertices: 0..0,
                indices: 0..0,
                edges: 2..4,
            },
        ],
        ..Default::default()
    };
    let geometry = (replaced.vertices.len(), replaced.indices.len());
    replaced
        .append_reference_outlines(
            &reference,
            &BTreeSet::from([42, 43]),
            &HashSet::from([ObjectId::Entity(42)]),
        )
        .unwrap();
    assert_eq!((replaced.vertices.len(), replaced.indices.len()), geometry);
    assert_eq!(
        replaced.reference_selection_ranges.as_slice(),
        std::slice::from_ref(&(0..2))
    );
    assert_eq!(replaced.reference_lines.len(), 2);
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/test");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(materials.join("sky.vmt"), "UnlitGeneric { %compilesky 1 }").unwrap();
    let fs = GameFs {
        game_dir: directory.path().to_owned(),
        base_dir: directory.path().to_owned(),
        mounts: vec![source_formats::gamefs::MountedPath {
            ids: vec!["game".into()],
            mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
        }],
    };
    vmf.world.solids[0].sides[0].material = "test/sky".into();
    let mut library = MaterialLibrary::default();
    let sky = SceneMesh::build_with_reference_models_and_surfaces_filtered(
        &vmf,
        &fs,
        &mut library,
        (&HashSet::from([object]), &HashSet::new()),
        &[],
        (&[], &BTreeMap::new()),
        (&BTreeMap::new(), &[]),
        None,
        true,
        true,
        false,
        false,
        &[],
        false,
        |_| true,
    );
    let sky_batches = sky
        .batches
        .iter()
        .filter(|batch| library.get(batch.material).sky)
        .collect::<Vec<_>>();
    assert_eq!(sky_batches.len(), 1);
    assert!(library.get(sky_batches[0].material).sky3d_eligible);
    assert_eq!(sky_batches[0].index_count, 6);
    assert_eq!(lines_in(&sky, &sky.selection_outline_ranges).len(), 48);
    assert!(
        sky.batches
            .iter()
            .all(|batch| batch.material == MaterialLibrary::EDITOR
                || library.get(batch.material).sky)
    );
}

#[test]
fn selection_outlines_keep_light_shapes_and_authored_direction_and_cone_guides() {
    for class in [
        "info_target",
        "light",
        "light_spot",
        "light_dynamic",
        "light_environment",
    ] {
        let mut vmf = map_core::Document::empty().vmf().clone();
        let mut entity = Entity::new(2, class);
        entity.properties.set("origin", "0 0 0");
        entity.properties.set("angles", "0 90 0");
        entity.properties.set("pitch", "-90");
        vmf.entities.push(entity);
        let mesh = SceneMesh::build(
            &vmf,
            &game(),
            &mut MaterialLibrary::default(),
            &HashSet::from([ObjectId::Entity(2)]),
        );
        assert!(!mesh.vertices.is_empty());
        assert!(!mesh.selection_outline_ranges.is_empty(), "{class}");
        let outlines = lines_in(&mesh, &mesh.selection_outline_ranges);
        assert!(outlines.iter().all(|line| line.color == SELECTED_EDGE));
        let guides = lines_in(&mesh, &mesh.guide_line_ranges());
        match class {
            "light_spot" | "light_dynamic" => {
                assert_eq!(guides.len(), 80);
                assert!(guides.iter().any(|line| line.position[2] < -90.0));
            }
            "light_environment" => assert_eq!(guides.len(), 18),
            _ => assert!(guides.is_empty()),
        }
        for pair in mesh.selection_outline_ranges.windows(2) {
            assert!(pair[0].end < pair[1].start);
        }
        assert_eq!(outlines.len() + guides.len(), mesh.lines.len());
    }
}
