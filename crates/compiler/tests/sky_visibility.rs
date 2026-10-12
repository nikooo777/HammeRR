mod common;

use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Leaf, Model, Node};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    for (name, text) in [
        ("sky", "UnlitGeneric { %compilesky 1 }"),
        ("sky2d", "UnlitGeneric { %compilesky 1 %compile2dsky 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn ceiling(vmf: &mut Vmf, material: &str) {
    let ceiling = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| {
            solid
                .sides
                .iter()
                .map(|side| side.plane.0[0].z)
                .fold(f64::INFINITY, f64::min)
                >= 256.0
        })
        .unwrap();
    let bottom = ceiling
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
        .unwrap();
    bottom.material = material.into();
}

fn compile(vmf: &Vmf, game: &GameFs) -> compiler::Output {
    let output = compiler::compile(vmf, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn leaf_at(bsp: &bsp::Bsp, point: DVec3) -> Leaf {
    let nodes = bsp.records::<Node>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let mut next = bsp.records::<Model>().unwrap()[0].head_node;
    while next >= 0 {
        let node = nodes[next as usize];
        let plane = planes[node.plane as usize];
        let distance =
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) - f64::from(plane.dist);
        next = node.children[usize::from(distance < 0.0)];
    }
    leaves[(-1 - next) as usize]
}

#[test]
fn enclosed_room_has_no_sky_or_radial_flags() {
    let (_directory, game) = game();
    let output = compile(&common::room(), &game);
    assert_eq!(output.stats.sky_clusters, 0);
    assert!(
        output
            .bsp
            .unwrap()
            .records::<Leaf>()
            .unwrap()
            .iter()
            .all(|leaf| leaf.flags() == 0)
    );
}

#[test]
fn sky_surface_marks_visible_clusters_with_the_correct_sky_type() {
    let (_directory, game) = game();
    for (material, expected) in [
        ("sky", bsp::leaf_flags::SKY),
        ("sky2d", bsp::leaf_flags::SKY2D),
    ] {
        let mut vmf = common::room();
        ceiling(&mut vmf, material);
        let source = vmf.clone();
        let output = compile(&vmf, &game);
        assert_eq!(output.stats.sky_clusters, 1);
        let bsp = output.bsp.unwrap();
        let leaf = leaf_at(&bsp, DVec3::new(0.0, 0.0, 64.0));
        assert_eq!(leaf.flags(), expected);
        assert_eq!(leaf.area(), 1);
        assert_eq!(vmf, source);
    }
}

#[test]
fn disconnected_room_does_not_inherit_the_other_rooms_sky() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    document
        .edit("two ceilings", |tx| {
            let ceiling = tx
                .vmf()
                .world
                .solids
                .iter()
                .find(|solid| {
                    solid
                        .sides
                        .iter()
                        .map(|side| side.plane.0[0].z)
                        .fold(f64::INFINITY, f64::min)
                        >= 256.0
                })
                .unwrap()
                .id;
            edit::delete(tx, &[map_core::ObjectId::Solid(ceiling)])?;
            edit::create_block(
                tx,
                DVec3::new(-272.0, -272.0, 256.0),
                DVec3::new(0.0, 272.0, 272.0),
                "sky",
            )?;
            edit::create_block(
                tx,
                DVec3::new(0.0, -272.0, 256.0),
                DVec3::new(272.0, 272.0, 272.0),
                "dev/dev_measuregeneric01",
            )?;
            edit::create_block(
                tx,
                DVec3::new(-8.0, -256.0, 0.0),
                DVec3::new(8.0, 256.0, 256.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let output = compile(document.vmf(), &game);
    assert_eq!(output.stats.sky_clusters, 1);
    let bsp = output.bsp.unwrap();
    assert_eq!(
        leaf_at(&bsp, DVec3::new(-64.0, 0.0, 64.0)).flags(),
        bsp::leaf_flags::SKY
    );
    assert_eq!(leaf_at(&bsp, DVec3::new(64.0, 0.0, 64.0)).flags(), 0);
}

#[test]
fn three_dimensional_sky_takes_precedence_over_two_dimensional_sky() {
    let (_directory, game) = game();
    let mut vmf = common::room();
    ceiling(&mut vmf, "sky");
    let wall = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| {
            solid
                .sides
                .iter()
                .any(|side| Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
                && solid.sides.iter().all(|side| side.plane.0[0].z < 256.0)
        })
        .unwrap();
    for side in &mut wall.sides {
        side.material = "sky2d".into();
    }
    let output = compile(&vmf, &game);
    assert_eq!(
        leaf_at(&output.bsp.unwrap(), DVec3::new(0.0, 0.0, 64.0)).flags(),
        bsp::leaf_flags::SKY
    );
}
