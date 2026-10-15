mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{Brush, Bsp, Face, Model, OCCLUDER_INACTIVE, Plane, lump};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    for (name, flags) in [
        ("toolsoccluder", "$translucent 1 %compilenolight 1"),
        ("toolstrigger", "%compiletrigger 1"),
        ("toolsnodraw", "%compilenodraw 1"),
        ("toolsskip", "%compileskip 1"),
        ("toolsorigin", "%compileorigin 1"),
        ("toolsareaportal", "%compileareaportal 1"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/tools/{name}.vmt")),
            format!("UnlitGeneric {{ {flags} }}"),
        )
        .unwrap();
    }
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, fs)
}

fn add_occluder(document: &mut Document, mins: DVec3, maxs: DVec3, active: bool) -> u32 {
    let id = document
        .edit("occluder", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(tx, mins, maxs, "tools/toolsoccluder")?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[solid],
                "func_occluder",
                &[
                    ("targetname".into(), "screen".into()),
                    ("StartActive".into(), if active { "1" } else { "0" }.into()),
                    ("occludernumber".into(), "777".into()),
                    ("model".into(), "*77".into()),
                ],
            )
        })
        .unwrap();
    let ObjectId::Entity(id) = id else {
        unreachable!()
    };
    id
}

fn fixture() -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let id = add_occluder(
        &mut document,
        DVec3::new(-8.0, -64.0, 16.0),
        DVec3::new(8.0, 64.0, 192.0),
        false,
    );
    (document.vmf().clone(), id)
}

#[test]
fn native_polygons_share_global_tables_without_collision_faces_or_brush_models() {
    let (_directory, fs) = game();
    let baseline = compiler::compile(&common::room(), &fs).bsp.unwrap();
    let (mut vmf, id) = fixture();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("origin", "96 48 32");
    let before = vmf.clone();
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(vmf, before);
    assert_eq!(output.stats.occluders, 1);
    assert_eq!(output.stats.occluder_polygons, 6);
    assert_eq!(output.stats.occluder_vertex_refs, 24);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    assert_eq!(bsp.records::<Model>().unwrap().len(), 1);
    assert_eq!(
        bsp.records::<Brush>().unwrap().len(),
        baseline.records::<Brush>().unwrap().len()
    );
    assert_eq!(
        bsp.records::<Face>().unwrap().len(),
        baseline.records::<Face>().unwrap().len()
    );
    let data = bsp.occlusion().unwrap().unwrap();
    let record = data.occluders[0];
    assert_eq!(record.flags, OCCLUDER_INACTIVE);
    assert_eq!(record.mins, [-8.0, -64.0, 16.0]);
    assert_eq!(record.maxs, [8.0, 64.0, 192.0]);
    assert_eq!(record.area, 1);
    let planes = bsp.records::<Plane>().unwrap();
    let vertices = bsp.vertices().unwrap();
    for polygon in &data.polygons {
        let plane = planes[polygon.plane as usize];
        let normal = DVec3::from_array(plane.normal.map(f64::from));
        let points = data.vertex_indices[polygon.first_vertex_index as usize..]
            [..polygon.vertex_count as usize]
            .iter()
            .map(|&index| DVec3::from_array(vertices[index as usize].map(f64::from)))
            .collect::<Vec<_>>();
        for point in &points {
            assert!((normal.dot(*point) - f64::from(plane.dist)).abs() < 0.01);
        }
        assert!(
            (points[2] - points[0])
                .cross(points[1] - points[0])
                .dot(normal)
                > 0.0
        );
    }
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"occludernumber\" \"0\""));
    assert!(entities.contains("\"StartActive\" \"0\""));
    assert!(entities.contains("\"targetname\" \"screen\""));
    assert!(!entities.contains("\"model\" \"*77\""));
    assert!(!entities.contains("\"occludernumber\" \"777\""));
    assert_eq!(bsp.lumps[lump::OCCLUSION].version, 2);
}

#[test]
fn nodraw_faces_are_omitted_trigger_faces_retained_and_hidden_indices_are_dense() {
    let (_directory, fs) = game();
    let (mut vmf, first) = fixture();
    let entity = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == first)
        .unwrap();
    entity.solids[0].sides[0].material = "tools/toolstrigger".into();
    for side in &mut entity.solids[0].sides[1..] {
        side.material = "tools/toolsnodraw".into();
    }
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluder_polygons, 1);
    let mut document = Document::new(vmf);
    let second = add_occluder(
        &mut document,
        DVec3::new(64.0, -32.0, 32.0),
        DVec3::new(80.0, 32.0, 160.0),
        true,
    );
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == first)
        .unwrap()
        .hidden = true;
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluders, 1);
    let bsp = output.bsp.unwrap();
    let data = bsp.occlusion().unwrap().unwrap();
    assert_eq!(data.occluders[0].flags, 0);
    assert_eq!(data.occluders[0].mins, [64.0, -32.0, 32.0]);
    assert!(
        bsp.entities()
            .unwrap()
            .contains(&format!("\"hammerid\" \"{second}\""))
    );
    assert!(bsp.entities().unwrap().contains("\"occludernumber\" \"0\""));
}

#[test]
fn invalid_helpers_have_object_errors_and_do_not_overwrite_a_successful_build() {
    let (_directory, fs) = game();
    let (vmf, id) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful build").unwrap();
    for case in 0..7 {
        let mut invalid = vmf.clone();
        let entity = invalid
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        match case {
            0 => entity.solids.clear(),
            1 => entity.properties.set("StartActive", "2"),
            2 => entity.properties.set("parentname", "door"),
            3 => {
                for side in &mut entity.solids[0].sides {
                    side.material = "tools/toolsnodraw".into();
                }
            }
            4 => entity.solids[0].sides[0].material = "missing/material".into(),
            5 => entity.solids[0].sides[0].material = "tools/toolsskip".into(),
            6 => {
                let mut extra = entity.solids[0].clone();
                extra.id += 10_000;
                for side in &mut extra.sides {
                    side.id += 10_000;
                }
                entity.solids.push(extra);
            }
            _ => unreachable!(),
        }
        let output = compiler::compile(&invalid, &fs);
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == compiler::Level::Error
                    && diagnostic.object.as_deref() == Some(&format!("entity:{id}"))),
            "case {case}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous successful build");
    }
}

#[test]
fn separate_areas_receive_distinct_indices_and_straddling_occluders_are_rejected() {
    let (_directory, fs) = game();
    let mut document = Document::new(common::room());
    document
        .edit("divide room", |tx| {
            for (mins, maxs) in [
                ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
                ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
                ([-8.0, -64.0, 192.0], [8.0, 64.0, 256.0]),
            ] {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "tools/toolsnodraw",
                )?;
            }
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-4.0, -64.0, 0.0),
                DVec3::new(4.0, 64.0, 192.0),
                "tools/toolsareaportal",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[id], "func_areaportal", &[])?;
            Ok(())
        })
        .unwrap();
    add_occluder(
        &mut document,
        DVec3::new(-160.0, -32.0, 16.0),
        DVec3::new(-144.0, 32.0, 160.0),
        true,
    );
    add_occluder(
        &mut document,
        DVec3::new(144.0, -32.0, 16.0),
        DVec3::new(160.0, 32.0, 160.0),
        true,
    );
    let output = compiler::compile(document.vmf(), &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let data = output.bsp.unwrap().occlusion().unwrap().unwrap();
    assert_eq!(data.occluders.len(), 2);
    assert_ne!(data.occluders[0].area, data.occluders[1].area);
    let mut joined = document.vmf().clone();
    let second = joined
        .entities
        .iter()
        .rposition(|entity| entity.classname() == Some("func_occluder"))
        .unwrap();
    let second = joined.entities.remove(second);
    let first = joined
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_occluder"))
        .unwrap();
    let combined_id = first.id;
    first.solids.extend(second.solids);
    let combined = compiler::compile(&joined, &fs);
    assert!(combined.bsp.is_none());
    assert!(
        combined
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.object.as_deref()
                == Some(&format!("entity:{combined_id}"))
                && diagnostic.message.contains("spans 2 visibility areas")),
        "{:?}",
        combined.report
    );

    let spanning = add_occluder(
        &mut document,
        DVec3::new(-32.0, -16.0, 16.0),
        DVec3::new(32.0, 16.0, 160.0),
        true,
    );
    let output = compiler::compile(document.vmf(), &fs);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some(&format!("entity:{spanning}"))
                    && diagnostic.message.contains("spans 2 visibility areas")
            ),
        "{:?}",
        output.report
    );
}

#[test]
fn helpers_inside_solid_cover_use_pruned_leaf_neighbors_and_outside_helpers_fail() {
    let (_directory, fs) = game();
    let mut document = Document::new(common::room());
    add_occluder(
        &mut document,
        DVec3::new(-264.0, -64.0, 16.0),
        DVec3::new(-260.0, 64.0, 192.0),
        true,
    );
    let output = compiler::compile(document.vmf(), &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(
        output.bsp.unwrap().occlusion().unwrap().unwrap().occluders[0].area,
        1
    );
    let outside = add_occluder(
        &mut document,
        DVec3::new(1000.0, -64.0, 16.0),
        DVec3::new(1004.0, 64.0, 192.0),
        true,
    );
    let output = compiler::compile(document.vmf(), &fs);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some(&format!("entity:{outside}"))
                    && diagnostic.message.contains("outside the world bounds")
            ),
        "{:?}",
        output.report
    );
}

fn multibrush_fixture(count: usize) -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let id = document
        .edit("multiple helper brushes", |tx| {
            let mut solids = Vec::new();
            for index in 0..count {
                let x = -180.0 + index as f64 * 20.0;
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, -48.0, 16.0),
                    DVec3::new(x + 8.0, 48.0, 192.0),
                    "tools/toolsoccluder",
                )?
                else {
                    unreachable!()
                };
                solids.push(id);
            }
            edit::tie_to_entity(
                tx,
                &solids,
                "func_occluder",
                &[
                    ("targetname".into(), "islands".into()),
                    ("StartActive".into(), "1".into()),
                ],
            )
        })
        .unwrap();
    let ObjectId::Entity(id) = id else {
        unreachable!()
    };
    (document.vmf().clone(), id)
}

#[test]
fn separated_helper_islands_share_one_index_and_hidden_children_do_not_emit() {
    let (_directory, fs) = game();
    let baseline = compiler::compile(&common::room(), &fs);
    let (mut vmf, id) = multibrush_fixture(2);
    let original = vmf.clone();
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(vmf, original);
    assert_eq!(output.stats.occluders, 1);
    assert_eq!(output.stats.occluder_polygons, 12);
    assert_eq!(output.stats.occluder_vertex_refs, 48);
    assert_eq!(output.stats.brushes, baseline.stats.brushes);
    assert_eq!(output.stats.faces, baseline.stats.faces);
    assert_eq!(output.stats.clusters, baseline.stats.clusters);
    let bsp = output.bsp.unwrap();
    let native = bsp.occlusion().unwrap().unwrap();
    assert_eq!(native.occluders.len(), 1);
    assert_eq!(native.occluders[0].polygon_count, 12);
    assert_eq!(native.occluders[0].mins, [-180.0, -48.0, 16.0]);
    assert_eq!(native.occluders[0].maxs, [-152.0, 48.0, 192.0]);
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .solids[1]
        .hidden = true;
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluder_polygons, 6);
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .solids[0]
        .hidden = true;
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluders, 0);
}

#[test]
fn touching_later_invalid_and_over_budget_helpers_reject_without_output() {
    let (_directory, fs) = game();
    for case in 0..5 {
        let (mut vmf, id) = multibrush_fixture(if case == 2 { 17 } else { 2 });
        let entity = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        match case {
            0 => {
                let original = entity.solids[0].clone();
                let id = entity.solids[1].id;
                let sides = entity.solids[1]
                    .sides
                    .iter()
                    .map(|side| side.id)
                    .collect::<Vec<_>>();
                entity.solids[1] = original;
                entity.solids[1].id = id;
                for (side, id) in entity.solids[1].sides.iter_mut().zip(sides) {
                    side.id = id;
                }
            }
            1 => entity.solids[1].sides[0].material = "missing/second_brush".into(),
            2 => {}
            3 => {
                for side in &mut entity.solids[1].sides {
                    for point in &mut side.plane.0 {
                        point.x -= 12.0;
                    }
                }
            }
            4 => {
                for index in 0..250 {
                    let mut side = entity.solids[1].sides[0].clone();
                    side.id = 100_000 + index;
                    entity.solids[1].sides.push(side);
                }
            }
            _ => unreachable!(),
        }
        let output = compiler::compile(&vmf, &fs);
        assert!(output.report.has_errors(), "case {case}");
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == compiler::Level::Error
                    && diagnostic.object.as_deref() == Some(&format!("entity:{id}"))),
            "{:?}",
            output.report
        );
    }
}
