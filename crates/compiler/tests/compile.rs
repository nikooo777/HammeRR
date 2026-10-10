mod common;

#[test]
fn authored_io_survives_vmf_reload_and_bsp_emission() {
    let mut document = map_core::Document::new(common::room());
    document
        .edit("logic", |tx| {
            let source = map_core::edit::create_entity(
                tx,
                "logic_relay",
                Some(glam::DVec3::new(0.0, 0.0, 64.0)),
                &[("targetname".into(), "relay".into())],
                Vec::new(),
            )?;
            let map_core::ObjectId::Entity(source) = source else {
                panic!()
            };
            for (delay, times) in [(0.0, -1), (0.75, 1)] {
                map_core::connections::set(
                    tx,
                    source,
                    None,
                    &map_core::connections::Spec {
                        output: "OnTrigger".into(),
                        target: "!self".into(),
                        input: "Enable".into(),
                        parameter: String::new(),
                        delay,
                        times,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    let saved = document.vmf().write(&Default::default());
    let vmf = source_formats::vmf::Vmf::parse(&saved).unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let bsp = output.bsp.unwrap();
    let bsp = source_formats::bsp::Bsp::parse(&bsp.to_bytes()).unwrap();
    let entities = bsp.entities().unwrap();
    assert_eq!(entities.matches("\"OnTrigger\"").count(), 2);
    assert!(entities.contains("\"!self,Enable,,0,-1\""));
    assert!(entities.contains("\"!self,Enable,,0.75,1\""));
}

use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{
    Brush, BrushSide, Bsp, Edge, Face, Leaf, Model, Node, Plane, TexInfo, contents, lump,
};

fn bsp_leaf(bsp: &Bsp, point: DVec3) -> Leaf {
    let nodes = bsp.records::<Node>().unwrap();
    let planes = bsp.records::<Plane>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let mut child = bsp.records::<Model>().unwrap()[0].head_node;
    while child >= 0 {
        let node = nodes[child as usize];
        let plane = planes[node.plane as usize];
        let distance =
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) - plane.dist as f64;
        child = node.children[usize::from(distance < 0.0)];
    }
    leaves[(-1 - child) as usize]
}

#[test]
fn sealed_room_has_visible_interior_and_solid_exterior() {
    let output = compiler::compile(&common::room(), &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let bsp = output.bsp.unwrap();
    assert_eq!(bsp.version, 20);
    assert_eq!(bsp.lump(lump::LEAFS).version, 1);
    assert_eq!(
        bsp_leaf(&bsp, DVec3::new(0.0, 0.0, 64.0)).contents,
        contents::EMPTY
    );
    for point in [
        DVec3::new(0.0, 0.0, -8.0),
        DVec3::new(264.0, 0.0, 64.0),
        DVec3::splat(400.0),
    ] {
        assert_ne!(bsp_leaf(&bsp, point).contents & contents::SOLID, 0);
    }
    let faces = bsp.records::<Face>().unwrap();
    assert_eq!(faces.len(), 6);
    let vertices = bsp.vertices().unwrap();
    for p in vertices {
        assert!(p[0].abs() <= 256.0 && p[1].abs() <= 256.0 && (0.0..=256.0).contains(&p[2]));
    }
    assert!(bsp.entities().unwrap().contains("info_player_terrorist"));
    assert!(
        bsp.entities()
            .unwrap()
            .contains("info_player_counterterrorist")
    );
    assert_eq!(Bsp::parse(&bsp.to_bytes()).unwrap(), bsp);
}

#[test]
fn missing_wall_reports_a_leak_from_an_entity_to_outside() {
    let mut vmf = common::room();
    vmf.world.solids.pop();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
    let leak = output.leak.unwrap();
    assert!(
        vmf.entities
            .iter()
            .any(|e| e.id == leak.entity && e.origin() == Some(leak.points[0]))
    );
    assert!(leak.points.len() >= 2);
    assert!(leak.points.last().unwrap().y > 272.0);
}

#[test]
fn detail_brushes_do_not_seal_a_leak() {
    let mut vmf = common::room();
    let wall = vmf.world.solids.pop().unwrap();
    let mut doc = Document::new(vmf);
    doc.edit("detail wall", |tx| {
        edit::create_entity(tx, "func_detail", None, &[], vec![wall])
    })
    .unwrap();
    let output = compiler::compile(doc.vmf(), &common::empty_game());
    assert!(output.leak.is_some());
    assert!(output.bsp.is_none());
}

#[test]
fn emitted_references_and_face_windings_are_valid() {
    let output = compiler::compile(&common::room(), &common::empty_game());
    let bsp = output.bsp.unwrap();
    let planes = bsp.records::<Plane>().unwrap();
    let nodes = bsp.records::<Node>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let texinfo = bsp.records::<TexInfo>().unwrap();
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<Edge>().unwrap();
    let surfedges = bsp.surfedges().unwrap();
    let brushes = bsp.records::<Brush>().unwrap();
    let sides = bsp.records::<BrushSide>().unwrap();
    for node in &nodes {
        assert!((node.plane as usize) < planes.len());
        assert_eq!(node.plane & 1, 0);
        assert!(node.first_face as usize + node.face_count as usize <= faces.len());
        for child in node.children {
            assert!(if child < 0 {
                ((-1 - child) as usize) < leaves.len()
            } else {
                (child as usize) < nodes.len()
            });
        }
    }
    for face in &faces {
        assert_eq!(face.side as u16, face.plane & 1);
        assert!(face.texinfo >= 0 && (face.texinfo as usize) < texinfo.len());
        assert!(face.lightmap_size.iter().all(|&s| s <= 32));
        let points: Vec<DVec3> = surfedges
            [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
            .iter()
            .map(|&s| {
                let edge = edges[s.unsigned_abs() as usize].0;
                DVec3::from_array(vertices[edge[usize::from(s < 0)] as usize].map(f64::from))
            })
            .collect();
        let normal = (points[1] - points[0]).cross(points[2] - points[0]);
        assert!(
            normal.dot(DVec3::from_array(
                planes[face.plane as usize].normal.map(f64::from)
            )) < 0.0
        );
    }
    for brush in brushes {
        assert!(
            brush.first_side >= 0 && (brush.first_side + brush.side_count) as usize <= sides.len()
        );
    }
    assert!(
        bsp.leaf_faces()
            .unwrap()
            .iter()
            .all(|&f| (f as usize) < faces.len())
    );
    assert!(
        bsp.leaf_brushes()
            .unwrap()
            .iter()
            .all(|&b| (b as usize) < output.stats.brushes)
    );
    assert_eq!(
        bsp.values::<u16>(lump::VERTNORMALINDICES).unwrap().len(),
        surfedges.len()
    );
}

#[test]
fn brush_entities_get_separate_models_and_detail_stays_in_world() {
    let mut doc = Document::new(common::room());
    doc.edit("extra geometry", |tx| {
        let detail = edit::create_block(
            tx,
            DVec3::new(-128.0, -64.0, 0.0),
            DVec3::new(-96.0, -32.0, 64.0),
            "dev/dev_measuregeneric01",
        )?;
        let ObjectId::Solid(detail) = detail else {
            unreachable!()
        };
        edit::tie_to_entity(tx, &[detail], "func_detail", &[])?;
        let door = edit::create_block(
            tx,
            DVec3::new(96.0, -32.0, 0.0),
            DVec3::new(112.0, 32.0, 112.0),
            "dev/dev_measuregeneric01",
        )?;
        let ObjectId::Solid(door) = door else {
            unreachable!()
        };
        edit::tie_to_entity(tx, &[door], "func_door", &[])?;
        Ok(())
    })
    .unwrap();
    let output = compiler::compile(doc.vmf(), &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    let models = bsp.records::<Model>().unwrap();
    assert_eq!(models.len(), 2);
    assert!(models[0].face_count > 6);
    assert_eq!(models[1].face_count, 6);
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"model\" \"*1\""));
    assert!(!entities.contains("func_detail"));
}

#[test]
fn a_map_without_occupants_is_rejected() {
    let mut vmf = common::room();
    vmf.entities.clear();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
}

#[test]
fn compilation_is_deterministic() {
    let vmf = common::room();
    let game = common::empty_game();
    let first = compiler::compile(&vmf, &game).bsp.unwrap().to_bytes();
    let second = compiler::compile(&vmf, &game).bsp.unwrap().to_bytes();
    assert_eq!(first, second);
}

#[test]
fn rotated_room_preserves_sealing_and_collision() {
    let mut vmf = common::room();
    let rotation = glam::DQuat::from_rotation_z(0.31);
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                *point = rotation * *point;
            }
        }
    }
    for entity in &mut vmf.entities {
        let origin = rotation * entity.origin().unwrap();
        entity
            .properties
            .set("origin", source_formats::vmf::format_vec3(origin));
    }
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    assert_eq!(
        bsp_leaf(&bsp, DVec3::new(0.0, 0.0, 64.0)).contents,
        contents::EMPTY
    );
    assert_ne!(
        bsp_leaf(&bsp, rotation * DVec3::new(264.0, 0.0, 64.0)).contents & contents::SOLID,
        0
    );
    assert!(bsp.records::<Face>().unwrap().len() >= 6);
}

#[test]
fn large_faces_are_subdivided_to_lightmap_limits() {
    let mut vmf = common::room();
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                *point *= 4.0;
            }
        }
    }
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    let faces = bsp.records::<Face>().unwrap();
    assert!(faces.len() > 6);
    assert!(
        faces
            .iter()
            .all(|f| f.lightmap_size.iter().all(|&s| s <= 32))
    );
}

#[test]
fn leaks_write_pointfiles_and_preserve_existing_bsp() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("room.bsp");
    let mut vmf = common::room();
    let game = common::empty_game();
    let sealed = compiler::compile(&vmf, &game);
    assert!(sealed.write(&path).unwrap());
    let saved = std::fs::read(&path).unwrap();
    vmf.world.solids.pop();
    let leaked = compiler::compile(&vmf, &game);
    assert!(!leaked.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    let pointfile = std::fs::read_to_string(path.with_extension("lin")).unwrap();
    let points: Vec<DVec3> = pointfile
        .lines()
        .map(|line| source_formats::vmf::parse_vec3(line).unwrap())
        .collect();
    assert_eq!(points, leaked.leak.unwrap().points);
}

#[test]
fn invalid_brushes_report_the_source_object() {
    let mut vmf = common::room();
    let brush = &mut vmf.world.solids[0];
    let id = format!("solid:{}", brush.id);
    brush.sides.truncate(2);
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|d| d.level == compiler::Level::Error && d.object.as_ref() == Some(&id))
    );
}
