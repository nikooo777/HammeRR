mod common;

use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{Bsp, Leaf, Node, Plane as BspPlane, VisibilityKind};
use source_formats::vmf::Vmf;

fn fixture(walls: &[([f64; 3], [f64; 3])], spawns: [[f64; 3]; 2]) -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("visibility walls", |tx| {
            for &(mins, maxs) in walls {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    for (entity, origin) in vmf.entities.iter_mut().zip(spawns) {
        entity.properties.set(
            "origin",
            source_formats::vmf::format_vec3(DVec3::from_array(origin)),
        );
    }
    vmf
}

fn compile(vmf: &Vmf) -> compiler::Output {
    let output = compiler::compile(vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert!(output.bsp.is_some());
    output
}

fn cluster(bsp: &Bsp, point: DVec3) -> usize {
    let nodes = bsp.records::<Node>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let planes = bsp.records::<BspPlane>().unwrap();
    let mut index = bsp.records::<source_formats::bsp::Model>().unwrap()[0].head_node;
    while index >= 0 {
        let node = nodes[index as usize];
        let plane = planes[node.plane as usize];
        let distance =
            point.dot(DVec3::from_array(plane.normal.map(f64::from))) - f64::from(plane.dist);
        index = node.children[usize::from(distance < 0.0)];
    }
    usize::try_from(leaves[(-1 - index) as usize].cluster).unwrap()
}

fn contains(row: &[u8], cluster: usize) -> bool {
    row[cluster / 8] & (1 << (cluster % 8)) != 0
}

#[test]
fn detailed_visibility_removes_broad_false_positives_without_losing_clear_sightlines() {
    let mut walls = Vec::new();
    for (x, opening) in [
        (-160.0, [-224.0, -128.0]),
        (0.0, [128.0, 224.0]),
        (160.0, [-224.0, -128.0]),
    ] {
        walls.push(([x - 8.0, -256.0, 0.0], [x + 8.0, opening[0], 256.0]));
        walls.push(([x - 8.0, opening[1], 0.0], [x + 8.0, 256.0, 256.0]));
    }
    let vmf = fixture(&walls, [[-224.0, -176.0, 8.0], [224.0, -176.0, 8.0]]);
    let fast = compile(&vmf).bsp.unwrap();
    let output = compiler::compile_with_options(
        &vmf,
        &common::empty_game(),
        compiler::Options {
            visibility: compiler::VisibilityMode::Detailed,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.stats.visibility_removed_pairs >= 2);
    assert_eq!(
        output.stats.visibility_mode,
        compiler::VisibilityMode::Detailed
    );
    let detailed = output.bsp.unwrap();
    let fast_vis = fast.visibility().unwrap().unwrap();
    let detailed_vis = detailed.visibility().unwrap().unwrap();
    let a = cluster(&detailed, DVec3::new(-224.0, -176.0, 64.0));
    let b = cluster(&detailed, DVec3::new(224.0, -176.0, 64.0));
    assert!(contains(
        &fast_vis.row(a, VisibilityKind::Visible).unwrap(),
        b
    ));
    assert!(!contains(
        &detailed_vis.row(a, VisibilityKind::Visible).unwrap(),
        b
    ));
    let points = [-224.0, -96.0, 96.0, 224.0]
        .into_iter()
        .flat_map(|x| {
            [-224.0, -176.0, -96.0, 0.0, 96.0, 176.0, 224.0]
                .into_iter()
                .flat_map(move |y| {
                    [16.0, 128.0, 240.0]
                        .into_iter()
                        .map(move |z| DVec3::new(x, y, z))
                })
        })
        .collect::<Vec<_>>();
    let mut clear = 0;
    for &a in &points {
        let row = detailed_vis
            .row(cluster(&detailed, a), VisibilityKind::Visible)
            .unwrap();
        for &b in &points {
            if !blocked(&vmf, a, b) {
                clear += 1;
                assert!(
                    contains(&row, cluster(&detailed, b)),
                    "clear {a:?} -> {b:?}"
                );
            }
        }
    }
    assert!(clear > 1000);
}

fn blocked(vmf: &Vmf, a: DVec3, b: DVec3) -> bool {
    vmf.world.solids.iter().any(|solid| {
        let mut enter: f64 = 0.0;
        let mut exit: f64 = 1.0;
        for side in &solid.sides {
            let plane = Plane::from_points(side.plane.0).unwrap();
            let start = plane.distance_to(a);
            let end = plane.distance_to(b);
            if start > 0.0 && end > 0.0 {
                return false;
            }
            if start > 0.0 {
                enter = enter.max(start / (start - end));
            } else if end > 0.0 {
                exit = exit.min(start / (start - end));
            }
            if enter > exit {
                return false;
            }
        }
        enter <= exit
    })
}

#[test]
fn sealed_room_keeps_self_visibility_and_emits_valid_audibility() {
    let output = compile(&common::room());
    assert_eq!(output.stats.clusters, 1);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let visibility = bsp.visibility().unwrap().unwrap();
    assert_eq!(visibility.clusters(), 1);
    assert_eq!(visibility.row(0, VisibilityKind::Visible).unwrap(), [1]);
    assert_eq!(visibility.row(0, VisibilityKind::Audible).unwrap(), [1]);
    assert_eq!(cluster(&bsp, DVec3::new(0.0, 0.0, 64.0)), 0);
}

#[test]
fn disconnected_rooms_do_not_share_visibility_or_audibility() {
    let vmf = fixture(
        &[([-8.0, -256.0, 0.0], [8.0, 256.0, 256.0])],
        [[-64.0, 0.0, 8.0], [64.0, 0.0, 8.0]],
    );
    let output = compile(&vmf);
    assert_eq!(output.stats.clusters, 2);
    assert_eq!(output.stats.visibility_portals, 0);
    let bsp = output.bsp.unwrap();
    let a = cluster(&bsp, DVec3::new(-64.0, 0.0, 64.0));
    let b = cluster(&bsp, DVec3::new(64.0, 0.0, 64.0));
    assert_ne!(a, b);
    let visibility = bsp.visibility().unwrap().unwrap();
    for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
        assert!(!contains(&visibility.row(a, kind).unwrap(), b));
        assert!(!contains(&visibility.row(b, kind).unwrap(), a));
    }
}

#[test]
fn doorway_and_bent_rooms_preserve_every_sampled_clear_sightline() {
    for (case, walls) in [
        vec![
            ([-8.0, -256.0, 0.0], [8.0, -32.0, 256.0]),
            ([-8.0, 32.0, 0.0], [8.0, 256.0, 256.0]),
        ],
        vec![
            ([-8.0, -256.0, 0.0], [8.0, 128.0, 256.0]),
            ([-256.0, -8.0, 0.0], [-8.0, 8.0, 256.0]),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let vmf = fixture(&walls, [[-64.0, -64.0, 8.0], [64.0, -64.0, 8.0]]);
        let source = vmf.clone();
        let output = compile(&vmf);
        assert!(output.stats.clusters > 1);
        assert!(output.stats.visibility_portals > 0);
        let bsp = output.bsp.unwrap();
        let visibility = bsp.visibility().unwrap().unwrap();
        let mut points = Vec::new();
        for x in [-192.0, -64.0, 64.0, 192.0] {
            for y in [-192.0, -64.0, 64.0, 192.0] {
                for z in [16.0, 128.0, 240.0] {
                    points.push(DVec3::new(x, y, z));
                }
            }
        }
        let mut clear = 0;
        for &a in &points {
            let a_cluster = cluster(&bsp, a);
            let row = visibility.row(a_cluster, VisibilityKind::Visible).unwrap();
            let audible = visibility.row(a_cluster, VisibilityKind::Audible).unwrap();
            assert!(contains(&row, a_cluster));
            for &b in &points {
                let b_cluster = cluster(&bsp, b);
                let reverse = visibility.row(b_cluster, VisibilityKind::Visible).unwrap();
                assert_eq!(contains(&row, b_cluster), contains(&reverse, a_cluster));
                assert!(!contains(&row, b_cluster) || contains(&audible, b_cluster));
                if !blocked(&vmf, a, b) {
                    clear += 1;
                    assert!(
                        contains(&row, b_cluster),
                        "clear sightline {a:?} -> {b:?} was culled"
                    );
                }
            }
        }
        assert!(
            clear > points.len() * 10,
            "only {clear} clear sightlines checked"
        );
        if case == 1 {
            let a = cluster(&bsp, DVec3::new(-64.0, -64.0, 64.0));
            let b = cluster(&bsp, DVec3::new(64.0, -64.0, 64.0));
            assert!(!contains(
                &visibility.row(a, VisibilityKind::Visible).unwrap(),
                b
            ));
        }
        assert_eq!(vmf, source);
        assert_eq!(bsp.to_bytes(), compile(&vmf).bsp.unwrap().to_bytes());
    }
}

#[test]
fn detail_and_brush_entity_geometry_do_not_create_world_visibility_clusters() {
    let mut document = Document::new(common::room());
    document
        .edit("detail and model", |tx| {
            for classname in ["func_detail", "func_brush"] {
                let map_core::ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-32.0, -32.0, 64.0),
                    DVec3::new(32.0, 32.0, 128.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(tx, &[id], classname, &[])?;
            }
            Ok(())
        })
        .unwrap();
    let output = compile(document.vmf());
    assert_eq!(output.stats.clusters, 1);
    let bsp = output.bsp.unwrap();
    let visibility = bsp.visibility().unwrap().unwrap();
    assert_eq!(visibility.clusters(), 1);
    let models = bsp.records::<source_formats::bsp::Model>().unwrap();
    assert_eq!(models.len(), 2);
    let nodes = bsp.records::<Node>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let mut pending = vec![models[1].head_node];
    while let Some(index) = pending.pop() {
        if index < 0 {
            assert_eq!(leaves[(-1 - index) as usize].cluster, -1);
        } else {
            pending.extend(nodes[index as usize].children);
        }
    }
}

#[test]
fn oblique_and_reflected_structural_portals_preserve_sampled_clear_sightlines() {
    for (reflection, mode) in [false, true].into_iter().flat_map(|reflection| {
        [
            compiler::VisibilityMode::Fast,
            compiler::VisibilityMode::Detailed,
        ]
        .map(|mode| (reflection, mode))
    }) {
        let vmf = fixture(
            &[
                ([-8.0, -256.0, 0.0], [8.0, 128.0, 256.0]),
                ([-256.0, -8.0, 0.0], [-8.0, 8.0, 256.0]),
            ],
            [[-64.0, -64.0, 8.0], [64.0, -64.0, 8.0]],
        );
        let ids: Vec<_> = vmf
            .world
            .solids
            .iter()
            .map(|solid| map_core::ObjectId::Solid(solid.id))
            .collect();
        let mut document = Document::new(vmf);
        document
            .edit("oblique portal room", |tx| {
                map_core::edit::transform(
                    tx,
                    &ids,
                    map_core::transform::Transform {
                        rotation: DVec3::new(0.0, 0.0, 17.0),
                        reflection: [reflection, false, false],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let linear = glam::DMat3::from_rotation_z(17.0_f64.to_radians())
            * glam::DMat3::from_diagonal(DVec3::new(if reflection { -1.0 } else { 1.0 }, 1.0, 1.0));
        let mut vmf = document.vmf().clone();
        for entity in &mut vmf.entities {
            if let Some(origin) = entity.origin() {
                entity
                    .properties
                    .set("origin", source_formats::vmf::format_vec3(linear * origin));
            }
        }
        let output = compiler::compile_with_options(
            &vmf,
            &common::empty_game(),
            compiler::Options {
                visibility: mode,
                ..Default::default()
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let visibility = bsp.visibility().unwrap().unwrap();
        let points: Vec<_> = [-192.0, -64.0, 64.0, 192.0]
            .into_iter()
            .flat_map(|x| {
                [-192.0, -64.0, 64.0, 192.0]
                    .into_iter()
                    .map(move |y| linear * DVec3::new(x, y, 128.0))
            })
            .collect();
        let mut clear = 0;
        for &a in &points {
            let row = visibility
                .row(cluster(&bsp, a), VisibilityKind::Visible)
                .unwrap();
            for &b in &points {
                if !blocked(&vmf, a, b) {
                    clear += 1;
                    assert!(
                        contains(&row, cluster(&bsp, b)),
                        "reflection={reflection}: {a:?} -> {b:?}"
                    );
                }
            }
        }
        assert!(clear > 50);
    }
}
