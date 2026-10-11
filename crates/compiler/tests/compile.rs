mod common;

#[test]
fn authored_lightmap_projection_and_distinct_smoothing_groups_reach_the_bsp() {
    use glam::DVec3;
    use map_core::{ObjectId, Plane, edit};
    use source_formats::bsp::{Face, TexInfo};
    let mut document = map_core::Document::new(common::room());
    document
        .edit("lighting fixture", |tx| {
            for (x, group) in [(0.0, 1u32), (64.0, 0x8000_0000)] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, 32.0, 0.0),
                    DVec3::new(x + 64.0, 96.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .solid_mut(id)?
                    .sides
                    .iter_mut()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                    .unwrap();
                side.smoothing_groups = Some(group);
                side.lightmap_scale = 8;
                side.u_axis.axis = DVec3::new(2.0, 0.5, 0.0);
                side.u_axis.offset = 12.0;
                side.u_axis.scale = -0.25;
            }
            Ok(())
        })
        .unwrap();
    let output = compiler::compile(document.vmf(), &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let texinfos = bsp.records::<TexInfo>().unwrap();
    for group in [1, 0x8000_0000] {
        let authored = faces
            .iter()
            .filter(|face| face.smoothing_groups == group)
            .collect::<Vec<_>>();
        assert!(!authored.is_empty());
        for face in authored {
            assert_eq!(
                texinfos[face.texinfo as usize].lightmap_vecs[0],
                [0.25, 0.0625, 0.0, -0.375]
            );
            assert_eq!(face.light_offset, -1);
        }
    }
}

#[test]
fn authored_primitives_compile_as_ordinary_closed_world_brushes() {
    use glam::DVec3;
    use map_core::primitive::{self, Axis, Shape, Spec};
    let mut document = map_core::Document::new(common::room());
    let initial = document.vmf().solids().count();
    let mut added = 0;
    for (index, shape) in [
        Shape::Wedge,
        Shape::Cylinder,
        Shape::Cone,
        Shape::Sphere,
        Shape::Arch,
    ]
    .into_iter()
    .enumerate()
    {
        let spec = Spec {
            shape,
            axis: Axis::Y,
            mins: DVec3::new(-200.0 + index as f64 * 80.0, 80.0, 0.0),
            maxs: DVec3::new(-136.0 + index as f64 * 80.0, 112.0, 64.0),
            inset: 8.0,
            material: "dev/dev_measuregeneric01".into(),
            ..Spec::default()
        };
        added += document
            .edit("primitive", |tx| primitive::create(tx, &spec))
            .unwrap()
            .len();
    }
    let saved = document.vmf().write(&Default::default());
    let reopened = source_formats::vmf::Vmf::parse(&saved).unwrap();
    let output = compiler::compile(&reopened, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(output.stats.brushes, initial + added);
    assert!(output.bsp.is_some());
}

#[test]
fn design_context_never_changes_compiled_bsp_bytes() {
    use glam::DVec3;
    use map_core::context::{Region, Scope, set_note};
    let mut document = map_core::Document::new(common::room());
    let game = common::empty_game();
    let before = compiler::compile(document.vmf(), &game)
        .bsp
        .unwrap()
        .to_bytes();
    let object = document.objects().next().unwrap();
    document
        .edit("design context", |tx| {
            set_note(
                tx,
                Scope::Map,
                "Map intent".into(),
                "Private editor context".into(),
                "agent",
                None,
            )?;
            set_note(
                tx,
                Scope::Object(object),
                "Object intent".into(),
                "Keep this wall".into(),
                "human",
                None,
            )?;
            set_note(
                tx,
                Scope::Region(1),
                "Region intent".into(),
                "Keep this route clear".into(),
                "agent",
                Some(Region {
                    mins: DVec3::splat(-128.0),
                    maxs: DVec3::splat(128.0),
                    color: [90, 180, 240],
                }),
            )
        })
        .unwrap();
    let saved = document.vmf().write(&Default::default());
    let reopened = source_formats::vmf::Vmf::parse(&saved).unwrap();
    assert!(saved.contains("hammerr_context"));
    let after = compiler::compile(&reopened, &game).bsp.unwrap().to_bytes();
    assert_eq!(before, after);
}

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

#[test]
fn safe_rename_preserves_entity_links_in_compiled_bsp() {
    let mut document = Document::new(common::room());
    let target = document
        .edit("logic", |tx| {
            let ObjectId::Entity(target) = edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::Z * 64.0),
                &[("targetname".into(), "old_relay".into())],
                Vec::new(),
            )?
            else {
                panic!()
            };
            let ObjectId::Entity(source) = edit::create_entity(
                tx,
                "logic_relay",
                Some(DVec3::Z * 96.0),
                &[
                    ("target".into(), "old_relay".into()),
                    ("parentname".into(), "old_relay,hinge".into()),
                ],
                Vec::new(),
            )?
            else {
                panic!()
            };
            for _ in 0..2 {
                map_core::connections::set(
                    tx,
                    source,
                    None,
                    &map_core::connections::Spec {
                        output: "OnTrigger".into(),
                        target: "old_relay".into(),
                        input: "Trigger".into(),
                        parameter: "".into(),
                        delay: 0.75,
                        times: 1,
                    },
                )?;
            }
            Ok(target)
        })
        .unwrap();
    let catalog = source_formats::fgd::Catalog::parse(r#"@PointClass = logic_relay [ targetname(target_source) : "Name" target(target_destination) : "Target" parentname(target_destination) : "Parent" ]"#).unwrap();
    let plan = map_core::references::plan(&document, &catalog, target, "new_relay", false).unwrap();
    document
        .edit("rename", |tx| map_core::references::apply(tx, &plan, false))
        .unwrap();
    let saved = document.vmf().write(&Default::default());
    let reloaded = source_formats::vmf::Vmf::parse(&saved).unwrap();
    let output = compiler::compile(&reloaded, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"targetname\" \"new_relay\""));
    assert!(entities.contains("\"target\" \"new_relay\""));
    assert!(entities.contains("\"parentname\" \"new_relay,hinge\""));
    assert_eq!(entities.matches("new_relay,Trigger,,0.75,1").count(), 2);
    assert!(!entities.contains("old_relay"));
}

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

#[test]
fn world_settings_survive_reload_and_hidden_entities_are_reported_as_omitted() {
    let mut document = Document::new(common::room());
    document
        .edit("world settings and visibility", |tx| {
            tx.world_properties_mut().set("skyname", "sky_day01_09");
            tx.world_properties_mut()
                .set("detailmaterial", "detail/detailsprites");
            let ObjectId::Entity(hidden) = edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::Z * 64.0),
                &[("targetname".into(), "hidden_marker".into())],
                Vec::new(),
            )?
            else {
                panic!()
            };
            tx.entity_mut(hidden)?.hidden = true;
            Ok(())
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    let bsp = output.bsp.unwrap();
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"skyname\" \"sky_day01_09\""));
    assert!(entities.contains("\"detailmaterial\" \"detail/detailsprites\""));
    assert!(!entities.contains("hidden_marker"));
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("hidden objects were not compiled")
    }));
}

#[test]
fn cancelled_compiles_and_writes_preserve_existing_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("room.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous leak").unwrap();
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let output = compiler::compile_controlled(&common::room(), &common::empty_game(), &control);
    assert!(output.bsp.is_none());
    assert!(output.leak.is_none());
    let output = compiler::compile(&common::room(), &common::empty_game());
    assert!(output.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous leak"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    let control = compiler::Control::default();
    assert!(output.write_controlled(&path, &control).unwrap());
    assert!(control.cancel().is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        output.bsp.unwrap().to_bytes()
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    let blocked = directory.path().join("blocked.bsp");
    std::fs::create_dir(&blocked).unwrap();
    let output = compiler::compile(&common::room(), &common::empty_game());
    assert!(output.write(&blocked).is_err());
    assert!(blocked.is_dir());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
}

#[test]
fn cancellation_interrupts_world_tree_partitioning() {
    use glam::DVec3;
    let mut document = map_core::Document::new(common::room());
    document
        .edit("dense geometry", |tx| {
            for i in 0..400 {
                let min = DVec3::new(
                    (i % 20) as f64 * 20.0 - 200.0,
                    (i / 20) as f64 * 20.0 - 200.0,
                    16.0,
                );
                map_core::edit::create_block(
                    tx,
                    min,
                    min + DVec3::splat(8.0),
                    "dev/dev_measuregeneric01",
                )?;
            }
            Ok(())
        })
        .unwrap();
    let control = compiler::Control::default();
    let worker_control = control.clone();
    let worker = std::thread::spawn(move || {
        compiler::compile_controlled(document.vmf(), &common::empty_game(), &worker_control)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while control.progress().stage != compiler::Stage::WorldTree
        && std::time::Instant::now() < deadline
        && !worker.is_finished()
    {
        std::thread::yield_now();
    }
    let stage = control.progress().stage;
    control.cancel().unwrap();
    let output = worker.join().unwrap();
    assert_eq!(stage, compiler::Stage::WorldTree);
    assert!(output.bsp.is_none());
    assert!(output.leak.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|d| d.message == "Compilation cancelled")
    );
}
