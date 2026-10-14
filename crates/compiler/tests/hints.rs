mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{self, Brush, Face, Leaf, Model, Node, TexInfo, VisibilityKind};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/tools");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, flag) in [("toolshint", "%compilehint"), ("toolsskip", "%compileskip")] {
        std::fs::write(
            materials.join(format!("{name}.vmt")),
            format!("UnlitGeneric {{ {flag} 1 }}"),
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn hint(vmf: Vmf, low: DVec3, high: DVec3) -> (Vmf, u32) {
    let mut document = Document::new(vmf);
    let id = document
        .edit("hint", |tx| {
            let ObjectId::Solid(id) = edit::create_block(tx, low, high, "tools/toolsskip")? else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            let face = solid
                .sides
                .iter_mut()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.x > 0.99)
                .unwrap();
            face.material = "tools/toolshint".into();
            Ok(id)
        })
        .unwrap();
    (document.vmf().clone(), id)
}

fn output(vmf: &Vmf, game: &GameFs, mode: compiler::VisibilityMode) -> compiler::Output {
    let before = vmf.clone();
    let output = compiler::compile_with_options(
        vmf,
        game,
        compiler::Options {
            visibility: mode,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    assert_eq!(vmf, &before);
    output
}

fn cluster(bsp: &bsp::Bsp, point: DVec3) -> i16 {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let nodes = bsp.records::<Node>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let mut index = bsp.records::<Model>().unwrap()[0].head_node;
    while index >= 0 {
        let node = nodes[index as usize];
        let plane = planes[node.plane as usize];
        index = node.children[usize::from(
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) < f64::from(plane.dist),
        )];
    }
    leaves[(-1 - index) as usize].cluster
}

#[test]
fn one_hint_face_partitions_space_without_skip_cuts_rendering_or_collision_brushes() {
    let (_directory, game) = game();
    let (vmf, _) = hint(
        common::room(),
        DVec3::new(-8.0, -64.0, 64.0),
        DVec3::new(8.0, 64.0, 192.0),
    );
    for mode in [
        compiler::VisibilityMode::Fast,
        compiler::VisibilityMode::Detailed,
    ] {
        let output = output(&vmf, &game, mode);
        assert_eq!(output.stats.hint_surfaces, 1);
        assert_eq!(output.stats.hint_splits, 1);
        assert_eq!(output.stats.clusters, 2);
        assert_eq!(output.stats.brushes, 6);
        assert!(output.stats.hint_work > 0);
        let bsp = output.bsp.unwrap();
        assert_eq!(bsp.records::<Brush>().unwrap().len(), 6);
        let infos = bsp.records::<TexInfo>().unwrap();
        assert!(bsp.records::<Face>().unwrap().iter().all(
            |face| infos[face.texinfo as usize].flags & (bsp::surf::HINT | bsp::surf::SKIP) == 0
        ));
        let a = cluster(&bsp, DVec3::new(0.0, 0.0, 32.0));
        let b = cluster(&bsp, DVec3::new(16.0, 0.0, 32.0));
        assert_ne!(a, b);
        let visibility = bsp.visibility().unwrap().unwrap();
        assert_eq!(
            visibility.row(a as usize, VisibilityKind::Visible).unwrap(),
            vec![3]
        );
    }
}

#[test]
fn hint_scope_ignores_other_rooms_solids_and_existing_planes_and_preserves_hidden_bytes() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    document
        .edit("separated rooms", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-256.0, -8.0, 0.0),
                DVec3::new(256.0, 8.0, 256.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut room = document.vmf().clone();
    for (entity, origin) in room.entities.iter_mut().zip(["-64 -64 8", "64 64 8"]) {
        entity.properties.set("origin", origin);
    }
    let (vmf, _) = hint(
        room,
        DVec3::new(-8.0, -128.0, 64.0),
        DVec3::new(8.0, -64.0, 192.0),
    );
    let compiled = output(&vmf, &game, compiler::VisibilityMode::Detailed);
    assert_eq!(compiled.stats.hint_splits, 1);
    assert_eq!(compiled.stats.clusters, 3);
    let bsp = compiled.bsp.unwrap();
    assert_ne!(
        cluster(&bsp, DVec3::new(0.0, -64.0, 64.0)),
        cluster(&bsp, DVec3::new(16.0, -64.0, 64.0))
    );
    assert_eq!(
        cluster(&bsp, DVec3::new(0.0, 64.0, 64.0)),
        cluster(&bsp, DVec3::new(16.0, 64.0, 64.0))
    );
    for (low, high) in [
        ([-16.0, -64.0, -12.0], [16.0, 64.0, -4.0]),
        ([1000.0, -64.0, 64.0], [1016.0, 64.0, 192.0]),
        ([240.0, -64.0, 64.0], [256.0, 64.0, 192.0]),
    ] {
        let (vmf, _) = hint(
            common::room(),
            DVec3::from_array(low),
            DVec3::from_array(high),
        );
        assert_eq!(
            output(&vmf, &game, compiler::VisibilityMode::Fast)
                .stats
                .hint_splits,
            0,
            "{low:?} {high:?}"
        );
    }
    let base = common::room();
    let (mut hidden, id) = hint(
        base.clone(),
        DVec3::new(-8.0, -64.0, 64.0),
        DVec3::new(8.0, 64.0, 192.0),
    );
    hidden
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .hidden = true;
    assert_eq!(
        output(&hidden, &game, compiler::VisibilityMode::Detailed)
            .bsp
            .unwrap()
            .to_bytes(),
        output(&base, &game, compiler::VisibilityMode::Detailed)
            .bsp
            .unwrap()
            .to_bytes()
    );
}

#[test]
fn oblique_and_partial_hint_planes_retain_checked_native_geometry() {
    let (_directory, game) = game();
    let (vmf, id) = hint(
        common::room(),
        DVec3::new(-8.0, -512.0, 64.0),
        DVec3::new(8.0, 64.0, 192.0),
    );
    let mut document = Document::new(vmf);
    document
        .edit("oblique hint", |tx| {
            edit::transform(
                tx,
                &[ObjectId::Solid(id)],
                map_core::transform::Transform {
                    rotation: DVec3::new(0.0, 0.0, 17.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let compiled = output(document.vmf(), &game, compiler::VisibilityMode::Detailed);
    assert_eq!(compiled.stats.hint_splits, 1);
    let bsp = compiled.bsp.unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    assert!(bsp.records::<Node>().unwrap().iter().any(|node| {
        let normal = planes[node.plane as usize].normal;
        (f64::from(normal[0]) - 17.0_f64.to_radians().cos()).abs() < 1e-5
            && (f64::from(normal[1]) - 17.0_f64.to_radians().sin()).abs() < 1e-5
    }));
}

#[test]
fn hints_do_not_seal_leaks_and_invalid_helpers_preserve_prior_output() {
    let (_directory, game) = game();
    let (valid, id) = hint(
        common::room(),
        DVec3::new(-8.0, -64.0, 64.0),
        DVec3::new(8.0, 64.0, 192.0),
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prior.bsp");
    let previous = output(&valid, &game, compiler::VisibilityMode::Fast)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&path, &previous).unwrap();
    let mut leaking = valid.clone();
    leaking.world.solids[1].hidden = true;
    let leak = compiler::compile(&leaking, &game);
    assert!(leak.leak.is_some() && leak.bsp.is_none());
    assert!(!leak.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    for class in [None, Some("func_detail"), Some("func_brush")] {
        let mut document = Document::new(valid.clone());
        document
            .edit("invalid hint", |tx| {
                if let Some(class) = class {
                    edit::tie_to_entity(tx, &[id], class, &[])?;
                } else {
                    tx.solid_mut(id)?
                        .sides
                        .iter_mut()
                        .find(|side| side.material.eq_ignore_ascii_case("tools/toolsskip"))
                        .unwrap()
                        .material = "dev/dev_measuregeneric01".into();
                }
                Ok(())
            })
            .unwrap();
        let output = compiler::compile(document.vmf(), &game);
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
    }
}

#[test]
fn priority_hint_precedes_structural_planes_crossing_its_finite_polygon() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    document
        .edit("crossing solid", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 96.0),
                DVec3::new(32.0, 32.0, 112.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let (source, _) = hint(
        document.vmf().clone(),
        DVec3::new(-8.0, -64.0, 64.0),
        DVec3::new(0.0, 64.0, 192.0),
    );
    let result = output(&source, &game, compiler::VisibilityMode::Detailed);
    assert!(result.stats.hint_priority_splits > 0);
    assert!(result.stats.hint_priority_splits <= result.stats.hint_splits);
    let bsp = result.bsp.unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let nodes = bsp.records::<Node>().unwrap();
    let point = DVec3::new(-16.0, 0.0, 80.0);
    let mut path = Vec::new();
    let mut index = bsp.records::<Model>().unwrap()[0].head_node;
    while index >= 0 {
        let node = nodes[index as usize];
        let plane = planes[node.plane as usize];
        path.push(plane);
        let distance =
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) - f64::from(plane.dist);
        index = node.children[usize::from(distance < 0.0)];
    }
    let hint = path
        .iter()
        .position(|plane| plane.normal == [1.0, 0.0, 0.0] && plane.dist == 0.0)
        .expect("Missing priority hint plane");
    let crossing = path
        .iter()
        .position(|plane| {
            plane.normal == [0.0, 0.0, 1.0] && plane.dist == 96.0
                || plane.normal == [0.0, 1.0, 0.0] && plane.dist.abs() == 32.0
        })
        .expect("Missing structural blocker plane");
    assert!(
        hint < crossing,
        "Hint came after the crossing cut: {path:?}"
    );
    assert!(cluster(&bsp, point) >= 0);
}
