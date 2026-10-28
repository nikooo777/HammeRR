use compiler::map::Map;
use compiler::tree::{Polyhedron, Tree};
use compiler::winding::Side;
use glam::DVec3;
use map_core::brush::BrushGeometry;
use map_core::{Document, edit};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const MATERIAL: &str = "tree/solid";
const WITNESS: DVec3 = DVec3::new(4000.0, 0.0, -250.0);

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/tree");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(materials.join("solid.vmt"), "UnlitGeneric {}").unwrap();
    let fs = GameFs {
        game_dir: directory.path().to_owned(),
        base_dir: directory.path().to_owned(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        }],
    };
    (directory, fs)
}

fn acute_brush_fixture(cut: f64) -> (Vmf, u32) {
    let mut document = Document::empty();
    let id = document
        .edit("acute brush", |tx| {
            let mut solid = edit::block_solid(tx, DVec3::ZERO, DVec3::ONE, MATERIAL)?;
            solid.sides.clear();
            let vertices = [
                DVec3::new(16000.0, 0.0, 0.0),
                DVec3::new(0.0, -1000.0, -1000.0),
                DVec3::new(0.0, 1000.0, -1000.0),
                DVec3::new(0.0, 0.0, 1000.0),
            ];
            for indices in [[1, 2, 3], [0, 2, 1], [0, 3, 2], [0, 1, 3]] {
                let mut points = indices.map(|i| vertices[i]);
                let plane = map_core::Plane::from_points(points).unwrap();
                if plane.distance_to(WITNESS) > 0.0 {
                    points.swap(0, 2);
                }
                solid.sides.push(edit::new_side(tx, points, MATERIAL)?);
            }
            let id = solid.id;
            tx.add_solid(solid)?;
            edit::create_block(
                tx,
                DVec3::new(cut, -2000.0, -2000.0),
                DVec3::new(16001.0, 2000.0, 2000.0),
                MATERIAL,
            )?;
            Ok(id)
        })
        .unwrap();
    let text = document.vmf().write(&Default::default());
    (Vmf::parse(&text).unwrap(), id)
}

#[test]
fn crossed_brush_cap_failure_refuses_to_erase_a_strict_interior() {
    let (_directory, fs) = game();
    for cut in [15999.98, 15990.0] {
        let (vmf, id) = acute_brush_fixture(cut);
        let before = vmf.clone();
        let geometry = BrushGeometry::from_solid(&vmf.world.solids[0]).unwrap();
        assert!(
            geometry
                .faces
                .iter()
                .all(|face| face.plane.distance_to(WITNESS) < -400.0)
        );
        let mut report = compiler::Report::default();
        let mut map = Map::load(&vmf, &fs, &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let target = map.brushes.iter().position(|brush| brush.id == id).unwrap();
        let shape = Polyhedron::from_brush(&map.brushes[target]);
        assert!(
            shape
                .sides
                .iter()
                .all(|side| map.planes.get(side.plane).distance(WITNESS) < -400.0)
        );
        let cut_plane = map.planes.find(DVec3::X, cut);
        assert_eq!(shape.classify(cut_plane, &map.planes), Side::Cross);
        assert!(map.planes.get(cut_plane).distance(WITNESS) < -10000.0);
        let split = shape.split(cut_plane, &map.planes);
        let indices = map.entities[0].brushes.clone();
        let tree = Tree::build(&map.brushes, &indices, &mut map.planes);
        if cut == 15999.98 {
            assert!(split.unwrap_err().contains("split cap"));
            let error = tree.unwrap_err();
            assert!(error.contains("split cap"), "{error}");
            assert!(error.contains(&format!("solid:{id}:")), "{error}");
            assert!(error.contains("split plane:"), "{error}");
            assert!(error.contains("normal ["), "{error}");
            assert!(error.contains("volume bounds ["), "{error}");
            let output = compiler::compile(&vmf, &fs);
            let object = format!("solid:{id}");
            assert!(output.report.has_errors());
            assert!(output.bsp.is_none());
            assert!(output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("split cap")
                    && diagnostic.message.contains(&format!("solid:{id}:"))
                    && diagnostic.object.as_deref() == Some(object.as_str())
                    && diagnostic.position.is_some_and(DVec3::is_finite)
            }));
            assert_eq!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.level == compiler::Level::Error)
                    .count(),
                1
            );
        } else {
            assert!(split.unwrap().iter().all(Option::is_some));
            let tree = tree.unwrap();
            let leaf = tree.leaf_at(WITNESS, &map.planes);
            assert!(tree.nodes[leaf].solid());
            assert!(tree.nodes[leaf].brushes.contains(&target));
            assert!(
                tree.brush_leaves(&map.brushes[target], &map.planes)
                    .unwrap()
                    .contains(&leaf)
            );
        }
        assert_eq!(vmf, before);
    }
}

#[test]
fn captured_split_failure_keeps_the_original_instance_owner() {
    let (directory, fs) = game();
    let (child, id) = acute_brush_fixture(15999.98);
    let child_path = directory.path().join("acute_child.vmf");
    let child_bytes = child.write(&Default::default());
    std::fs::write(&child_path, &child_bytes).unwrap();
    let mut source = Document::empty().vmf().clone();
    let mut helper = source_formats::vmf::Entity::new(40_000, "func_instance");
    helper.properties.set("file", "acute_child.vmf");
    helper.properties.set("origin", "0 0 0");
    source.entities.push(helper);
    let source_path = directory.path().join("parent.vmf");
    let source_bytes = source.write(&Default::default());
    std::fs::write(&source_path, &source_bytes).unwrap();
    let before = source.clone();
    let control = compiler::Control::default();
    let context = compiler::instances::SourceContext::capture(&source_path, &control).unwrap();
    let output = compiler::compile_with_source_controlled(
        &source,
        &fs,
        compiler::Options::default(),
        None,
        Some(&context),
        &control,
    );
    assert!(output.bsp.is_none());
    let errors = output
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.level == compiler::Level::Error)
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{:?}", output.report);
    let error = errors[0];
    assert!(error.message.contains("split cap"), "{}", error.message);
    assert!(
        error.message.contains("acute_child.vmf"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains(&format!("child object {id}")),
        "{}",
        error.message
    );
    assert_eq!(error.object.as_deref(), Some("entity:40000"));
    assert!(error.position.is_some_and(DVec3::is_finite));
    assert_eq!(source, before);
    assert_eq!(std::fs::read_to_string(source_path).unwrap(), source_bytes);
    assert_eq!(std::fs::read_to_string(child_path).unwrap(), child_bytes);
}

#[test]
fn leaf_brush_membership_propagates_uncertain_cap_errors() {
    let (_directory, fs) = game();
    let (vmf, id) = acute_brush_fixture(15999.98);
    let mut report = compiler::Report::default();
    let mut map = Map::load(&vmf, &fs, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let tree = Tree::build(&map.brushes, &[1], &mut map.planes).unwrap();
    let error = tree.brush_leaves(&map.brushes[0], &map.planes).unwrap_err();
    assert!(error.contains("split cap"), "{error}");
    assert!(error.contains(&format!("solid:{id}:")), "{error}");
}

#[test]
fn noncrossing_and_on_band_splits_retain_their_clipping_behavior() {
    let (_directory, fs) = game();
    let (vmf, _) = acute_brush_fixture(15990.0);
    let mut report = compiler::Report::default();
    let mut map = Map::load(&vmf, &fs, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let shape = Polyhedron::from_brush(&map.brushes[0]);
    for (dist, expected) in [(0.0, [true, false]), (16001.0, [false, true])] {
        let plane = map.planes.find(DVec3::X, dist);
        let halves = shape.split(plane, &map.planes).unwrap();
        assert_eq!(halves.each_ref().map(Option::is_some), expected);
    }
    let mut document = Document::empty();
    document
        .edit("on-band brush", |tx| {
            edit::create_block(
                tx,
                DVec3::new(0.002, -1.0, -1.0),
                DVec3::new(0.008, 1.0, 1.0),
                MATERIAL,
            )?;
            Ok(())
        })
        .unwrap();
    let mut map = Map::load(document.vmf(), &fs, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let shape = Polyhedron::from_brush(&map.brushes[0]);
    let plane = map.planes.find(DVec3::X, 0.0);
    assert_eq!(shape.classify(plane, &map.planes), Side::On);
    assert!(
        shape
            .split(plane, &map.planes)
            .unwrap()
            .iter()
            .all(Option::is_none)
    );
}
