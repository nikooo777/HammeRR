mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{Bsp, Brush, Face, Model, OCCLUDER_INACTIVE, Plane, lump};
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
        std::fs::write(directory.path().join(format!("materials/tools/{name}.vmt")), format!("UnlitGeneric {{ {flags} }}")).unwrap();
    }
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()], mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, fs)
}

fn add_occluder(document: &mut Document, mins: DVec3, maxs: DVec3, active: bool) -> u32 {
    let id = document.edit("occluder", |tx| {
        let ObjectId::Solid(solid) = edit::create_block(tx, mins, maxs, "tools/toolsoccluder")? else { unreachable!() };
        edit::tie_to_entity(tx, &[solid], "func_occluder", &[
            ("targetname".into(), "screen".into()),
            ("StartActive".into(), if active { "1" } else { "0" }.into()),
            ("occludernumber".into(), "777".into()),
            ("model".into(), "*77".into()),
        ])
    }).unwrap();
    let ObjectId::Entity(id) = id else { unreachable!() };
    id
}

fn fixture() -> (Vmf, u32) {
    let mut document = Document::new(common::room());
    let id = add_occluder(&mut document, DVec3::new(-8.0, -64.0, 16.0), DVec3::new(8.0, 64.0, 192.0), false);
    (document.vmf().clone(), id)
}

#[test]
fn native_polygons_share_global_tables_without_collision_faces_or_brush_models() {
    let (_directory, fs) = game();
    let baseline = compiler::compile(&common::room(), &fs).bsp.unwrap();
    let (mut vmf, id) = fixture();
    vmf.entities.iter_mut().find(|entity| entity.id == id).unwrap().properties.set("origin", "96 48 32");
    let before = vmf.clone();
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(vmf, before);
    assert_eq!(output.stats.occluders, 1);
    assert_eq!(output.stats.occluder_polygons, 6);
    assert_eq!(output.stats.occluder_vertex_refs, 24);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    assert_eq!(bsp.records::<Model>().unwrap().len(), 1);
    assert_eq!(bsp.records::<Brush>().unwrap().len(), baseline.records::<Brush>().unwrap().len());
    assert_eq!(bsp.records::<Face>().unwrap().len(), baseline.records::<Face>().unwrap().len());
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
        let points = data.vertex_indices[polygon.first_vertex_index as usize..][..polygon.vertex_count as usize]
            .iter().map(|&index| DVec3::from_array(vertices[index as usize].map(f64::from))).collect::<Vec<_>>();
        for point in &points {
            assert!((normal.dot(*point) - f64::from(plane.dist)).abs() < 0.01);
        }
        assert!((points[2] - points[0]).cross(points[1] - points[0]).dot(normal) > 0.0);
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
    let entity = vmf.entities.iter_mut().find(|entity| entity.id == first).unwrap();
    entity.solids[0].sides[0].material = "tools/toolstrigger".into();
    for side in &mut entity.solids[0].sides[1..] { side.material = "tools/toolsnodraw".into(); }
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluder_polygons, 1);
    let mut document = Document::new(vmf);
    let second = add_occluder(&mut document, DVec3::new(64.0, -32.0, 32.0), DVec3::new(80.0, 32.0, 160.0), true);
    let mut vmf = document.vmf().clone();
    vmf.entities.iter_mut().find(|entity| entity.id == first).unwrap().hidden = true;
    let output = compiler::compile(&vmf, &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluders, 1);
    let bsp = output.bsp.unwrap();
    let data = bsp.occlusion().unwrap().unwrap();
    assert_eq!(data.occluders[0].flags, 0);
    assert_eq!(data.occluders[0].mins, [64.0, -32.0, 32.0]);
    assert!(bsp.entities().unwrap().contains(&format!("\"hammerid\" \"{second}\"")));
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
        let entity = invalid.entities.iter_mut().find(|entity| entity.id == id).unwrap();
        match case {
            0 => entity.solids.clear(),
            1 => entity.properties.set("StartActive", "2"),
            2 => entity.properties.set("parentname", "door"),
            3 => { for side in &mut entity.solids[0].sides { side.material = "tools/toolsnodraw".into(); } }
            4 => entity.solids[0].sides[0].material = "missing/material".into(),
            5 => entity.solids[0].sides[0].material = "tools/toolsskip".into(),
            6 => {
                let mut extra = entity.solids[0].clone();
                extra.id += 10_000;
                for side in &mut extra.sides { side.id += 10_000; }
                entity.solids.push(extra);
            }
            _ => unreachable!(),
        }
        let output = compiler::compile(&invalid, &fs);
        assert!(output.report.diagnostics.iter().any(|diagnostic| diagnostic.level == compiler::Level::Error && diagnostic.object.as_deref() == Some(&format!("entity:{id}"))), "case {case}: {:?}", output.report);
        assert!(output.bsp.is_none());
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous successful build");
    }
}

#[test]
fn separate_areas_receive_distinct_indices_and_straddling_occluders_are_rejected() {
    let (_directory, fs) = game();
    let mut document = Document::new(common::room());
    document.edit("divide room", |tx| {
        for (mins, maxs) in [
            ([-8.0, -256.0, 0.0], [8.0, -64.0, 256.0]),
            ([-8.0, 64.0, 0.0], [8.0, 256.0, 256.0]),
            ([-8.0, -64.0, 192.0], [8.0, 64.0, 256.0]),
        ] {
            edit::create_block(tx, DVec3::from_array(mins), DVec3::from_array(maxs), "tools/toolsnodraw")?;
        }
        let ObjectId::Solid(id) = edit::create_block(tx, DVec3::new(-4.0, -64.0, 0.0), DVec3::new(4.0, 64.0, 192.0), "tools/toolsareaportal")? else { unreachable!() };
        edit::tie_to_entity(tx, &[id], "func_areaportal", &[])?;
        Ok(())
    }).unwrap();
    add_occluder(&mut document, DVec3::new(-160.0, -32.0, 16.0), DVec3::new(-144.0, 32.0, 160.0), true);
    add_occluder(&mut document, DVec3::new(144.0, -32.0, 16.0), DVec3::new(160.0, 32.0, 160.0), true);
    let output = compiler::compile(document.vmf(), &fs);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let data = output.bsp.unwrap().occlusion().unwrap().unwrap();
    assert_eq!(data.occluders.len(), 2);
    assert_ne!(data.occluders[0].area, data.occluders[1].area);
    let spanning = add_occluder(&mut document, DVec3::new(-32.0, -16.0, 16.0), DVec3::new(32.0, 16.0, 160.0), true);
    let output = compiler::compile(document.vmf(), &fs);
    assert!(output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| diagnostic.object.as_deref() == Some(&format!("entity:{spanning}")) && diagnostic.message.contains("spans 2 visibility areas")), "{:?}", output.report);
}
