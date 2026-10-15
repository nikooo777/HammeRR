mod common;
#[path = "common/areas.rs"]
mod fixtures;

use glam::DVec3;
use map_core::{Document, compile_region};
use source_formats::{
    bsp::{Bsp, Leaf, Model, Plane, contents},
    gamefs::{GameFs, Mount, MountedPath},
    vmf::{Entity, Vmf},
};

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    for (name, text) in [
        ("toolsblack", "UnlitGeneric { }"),
        ("toolsnodraw", "LightmappedGeneric { %compilenodraw 1 }"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/tools/{name}.vmt")),
            text,
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

fn bounds() -> compile_region::Bounds {
    compile_region::Bounds {
        mins: DVec3::new(-128.0, -128.0, 0.0),
        maxs: DVec3::new(128.0, 128.0, 256.0),
    }
}

fn region(source: Vmf, bounds: compile_region::Bounds, enabled: bool) -> Vmf {
    let mut document = Document::new(source);
    document
        .edit("compile region", |tx| {
            compile_region::apply(tx, Some(compile_region::Definition { enabled, bounds }))
        })
        .unwrap();
    document.into_vmf()
}

fn fixture(window: bool, foreground: bool) -> (Vmf, u32, Option<u32>) {
    if window {
        let (source, portal, target, _) = fixtures::window_room(foreground);
        (source, portal, Some(target))
    } else {
        let (source, portal) = fixtures::room(true);
        (source, portal, None)
    }
}

fn entity(source: &mut Vmf, id: u32) -> &mut Entity {
    source
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn translate(entity: &mut Entity, offset: DVec3) {
    for solid in &mut entity.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                *point += offset;
            }
        }
    }
}

fn compiled(source: &Vmf, game: &GameFs) -> compiler::Output {
    let output = compiler::compile(source, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    assert!(output.leak.is_none());
    output
}

fn assert_native_graph(bsp: &Bsp, id: u32, window: bool, foreground: bool) {
    let data = bsp.area_data().unwrap().unwrap();
    assert_eq!(data.areas.len(), 3);
    assert_eq!(data.portals.len(), 3);
    assert_eq!(data.clip_vertices.len(), 8);
    assert_eq!(data.portals[1].portal_key, 1);
    assert_eq!(data.portals[2].portal_key, 1);
    assert_eq!(data.portals[1].plane ^ 1, data.portals[2].plane);
    let planes = bsp.records::<Plane>().unwrap();
    for (owner, area) in data.areas.iter().enumerate() {
        let first = area.first_portal as usize;
        for portal in &data.portals[first..first + area.portal_count as usize] {
            assert_ne!(portal.other_area as usize, owner);
            assert!((1..data.areas.len()).contains(&(portal.other_area as usize)));
            assert!((portal.plane as usize) < planes.len());
            assert!(
                portal.first_clip_vert as usize + portal.clip_vert_count as usize
                    <= data.clip_vertices.len()
            );
        }
    }
    let polygons = data.portals[1..]
        .iter()
        .map(|portal| {
            let first = portal.first_clip_vert as usize;
            &data.clip_vertices[first..first + portal.clip_vert_count as usize]
        })
        .collect::<Vec<_>>();
    assert_eq!(polygons[0], polygons[1]);
    for point in &data.clip_vertices {
        assert_eq!(point[0].abs(), 4.0);
        assert_eq!(point[1].abs(), 64.0);
        assert!(point[2] == 0.0 || point[2] == 192.0);
    }
    let leaves = bsp.records::<Leaf>().unwrap();
    for area in [1, 2] {
        assert!(
            leaves
                .iter()
                .any(|leaf| leaf.area() == area && leaf.contents & contents::SOLID == 0)
        );
    }
    assert!(
        leaves
            .iter()
            .any(|leaf| leaf.contents & contents::AREAPORTAL != 0)
    );
    assert_eq!(
        bsp.records::<Model>().unwrap().len(),
        1 + usize::from(window) + usize::from(foreground)
    );
    let entities = bsp.entities().unwrap();
    let classname = if window {
        "func_areaportalwindow"
    } else {
        "func_areaportal"
    };
    let control = entities
        .split("{\n")
        .find(|entity| entity.contains(&format!("\"classname\" \"{classname}\"")))
        .unwrap();
    for field in [
        "\"portalnumber\" \"1\"".to_owned(),
        "\"PortalVersion\" \"1\"".to_owned(),
        "\"targetname\" \"partition_gate\"".to_owned(),
        "\"origin\" \"512 512 96\"".to_owned(),
        format!("\"hammerid\" \"{id}\""),
    ] {
        assert!(control.contains(&field), "{control}");
    }
    assert!(!control.contains("\"model\""));
    if window {
        for field in [
            "\"target\" \"rendered_window\"",
            "\"FadeStartDist\" \"128\"",
            "\"FadeDist\" \"512\"",
            "\"TranslucencyLimit\" \"0.2\"",
        ] {
            assert!(control.contains(field), "{control}");
        }
        assert!(entities.contains("\"origin\" \"0.5 -0.25 96.25\""));
        if foreground {
            assert!(control.contains("\"BackgroundBModel\" \"window_foreground\""));
        }
    }
}

#[test]
fn contained_helpers_keep_world_geometry_and_native_controls_in_both_compile_entries() {
    let (_directory, game) = game();
    for (window, foreground) in [(false, false), (true, false), (true, true)] {
        let (mut source, id, _) = fixture(window, foreground);
        let portal = entity(&mut source, id);
        portal.properties.set("origin", "512 512 96");
        portal.properties.set("PortalVersion", "0");
        portal.properties.set("portalnumber", "999");
        portal.properties.set("model", "*99");
        let source = region(source, bounds(), true);
        let before = source.clone();
        let output = compiled(&source, &game);
        assert_eq!(output.stats.areas, 2);
        assert_eq!(output.stats.areaportals, 1);
        assert_eq!(output.stats.areaportal_windows, usize::from(window));
        assert_eq!(output.stats.areaportal_vertices, 8);
        let summary = output.stats.compile_region.as_ref().unwrap();
        assert!(summary.included.contains(&format!("entity:{id}")));
        assert!(!summary.omitted.contains(&format!("entity:{id}")));
        assert_eq!(summary.generated_brushes.len(), 6);
        let bytes = output.bsp.unwrap().to_bytes();
        let bsp = Bsp::parse(&bytes).unwrap();
        assert_native_graph(&bsp, id, window, foreground);
        let controlled = compiler::compile_controlled(&source, &game, &Default::default());
        assert!(!controlled.report.has_errors(), "{:?}", controlled.report);
        assert_eq!(controlled.bsp.unwrap().to_bytes(), bytes);
        assert_eq!(source, before);
    }
}

#[test]
fn disjoint_helpers_are_omitted_despite_inside_origins() {
    let (_directory, game) = game();
    for window in [false, true] {
        let (mut source, id, _) = fixture(window, false);
        let portal = entity(&mut source, id);
        translate(portal, DVec3::new(512.0, 0.0, 0.0));
        portal.properties.set("origin", "0 0 96");
        let source = region(source, bounds(), true);
        let before = source.clone();
        let output = compiled(&source, &game);
        assert_eq!(output.stats.areas, 1);
        assert_eq!(output.stats.areaportals, 0);
        assert_eq!(output.stats.areaportal_windows, 0);
        let summary = output.stats.compile_region.unwrap();
        assert!(summary.omitted.contains(&format!("entity:{id}")));
        let bsp = output.bsp.unwrap();
        assert_eq!(bsp.area_data().unwrap().unwrap().portals.len(), 1);
        assert!(!bsp.entities().unwrap().contains("func_areaportal"));
        assert_eq!(
            bsp.records::<Model>().unwrap().len(),
            1 + usize::from(window)
        );
        assert_eq!(source, before);
    }
}

#[test]
fn crossing_any_region_plane_rejects_whole_helpers_and_preserves_previous_output() {
    let (directory, game) = game();
    let path = directory.path().join("previous.bsp");
    for window in [false, true] {
        let (source, id, _) = fixture(window, false);
        let valid = region(source.clone(), bounds(), true);
        let bytes = compiled(&valid, &game).bsp.unwrap().to_bytes();
        std::fs::write(&path, &bytes).unwrap();
        for axis in 0..3 {
            for upper in [false, true] {
                let mut scope = bounds();
                let plane = if axis == 2 { 96.0 } else { 0.0 };
                if upper {
                    scope.maxs[axis] = plane;
                } else {
                    scope.mins[axis] = plane;
                }
                let source = region(source.clone(), scope, true);
                let before = source.clone();
                let output = compiler::compile(&source, &game);
                assert!(output.report.has_errors(), "{:?}", output.report);
                assert!(output.bsp.is_none());
                assert!(output.leak.is_none());
                assert_eq!(output.stats.brushes, 0);
                assert!(output.report.diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .message
                        .contains("crosses a compile-region boundary")
                        && diagnostic.message.contains(&format!("entity:{id}"))
                }));
                assert!(!output.write(&path).unwrap());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert_eq!(source, before);
            }
        }
    }
}

#[test]
fn combined_visible_bounds_cannot_span_the_region_from_disjoint_brushes() {
    let (_directory, game) = game();
    for window in [false, true] {
        let (mut source, id, _) = fixture(window, false);
        let portal = entity(&mut source, id);
        translate(portal, DVec3::new(-512.0, 0.0, 0.0));
        let mut second = portal.solids[0].clone();
        second.id += 10_000;
        for side in &mut second.sides {
            side.id += 10_000;
            for point in &mut side.plane.0 {
                point.x += 1024.0;
            }
        }
        portal.solids.push(second);
        let source = region(source, bounds(), true);
        let output = compiler::compile(&source, &game);
        assert!(output.bsp.is_none());
        assert_eq!(output.stats.brushes, 0);
        assert!(output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("crosses a compile-region boundary")
                && diagnostic.message.contains(&format!("entity:{id}"))
        }));
    }
}

#[test]
fn hidden_brushes_are_omitted_and_empty_helpers_keep_the_existing_noop() {
    let (_directory, game) = game();
    for window in [false, true] {
        for empty in [false, true] {
            let (mut source, id, _) = fixture(window, false);
            let portal = entity(&mut source, id);
            portal.properties.set("origin", "512 512 96");
            if empty {
                portal.solids.clear();
            } else {
                portal.solids[0].hidden = true;
                portal.solids[0].sides[0].plane.0[0].x = f64::NAN;
            }
            let source = region(source, bounds(), true);
            let output = compiled(&source, &game);
            assert_eq!(output.stats.areaportals, 0);
            assert_eq!(output.stats.areas, 1);
            let summary = output.stats.compile_region.unwrap();
            assert_eq!(summary.included.contains(&format!("entity:{id}")), empty);
            assert_eq!(summary.omitted.contains(&format!("entity:{id}")), !empty);
            assert!(
                !output
                    .bsp
                    .unwrap()
                    .entities()
                    .unwrap()
                    .contains("func_areaportal")
            );
        }
    }
}

#[test]
fn malformed_visible_geometry_is_rejected_even_when_the_helper_is_outside() {
    let (directory, game) = game();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous successful build").unwrap();
    for window in [false, true] {
        for nonfinite in [false, true] {
            let (mut source, id, _) = fixture(window, false);
            let portal = entity(&mut source, id);
            translate(portal, DVec3::new(512.0, 0.0, 0.0));
            if nonfinite {
                portal.solids[0].sides[0].plane.0[0].x = f64::INFINITY;
            } else {
                portal.solids[0].sides.clear();
            }
            let object = format!("solid:{}", portal.solids[0].id);
            let source = region(source, bounds(), true);
            let output = compiler::compile(&source, &game);
            assert!(output.report.has_errors());
            assert!(output.bsp.is_none());
            assert_eq!(output.stats.brushes, 0);
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.level == compiler::Level::Error
                        && diagnostic.object.as_ref() == Some(&object))
            );
            assert!(!output.write(&path).unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), b"previous successful build");
        }
    }
}

#[test]
fn region_selection_keeps_existing_portal_and_window_validation() {
    let (_directory, game) = game();
    for case in 0..5 {
        let (mut source, id, target) = fixture(case >= 2, false);
        let expected = match case {
            0 => {
                let portal = entity(&mut source, id);
                let mut second = portal.solids[0].clone();
                second.id += 10_000;
                for side in &mut second.sides {
                    side.id += 10_000;
                }
                portal.solids.push(second);
                "one visible convex brush"
            }
            1 => {
                (source, _) = fixtures::room(false);
                "two distinct areas"
            }
            2 => {
                entity(&mut source, id).properties.set("FadeDist", "NaN");
                "finite number"
            }
            3 => {
                entity(&mut source, id)
                    .properties
                    .set("target", "absent_window");
                "exactly one visible rendered brush model"
            }
            4 => {
                entity(&mut source, target.unwrap())
                    .properties
                    .set("origin", "512 0 96");
                "exactly one visible rendered brush model"
            }
            _ => unreachable!(),
        };
        let source = region(source, bounds(), true);
        let before = source.clone();
        let output = compiler::compile(&source, &game);
        assert!(
            output.report.has_errors(),
            "case {case}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains(expected) }),
            "case {case}: {:?}",
            output.report
        );
        assert_eq!(source, before);
    }
}

#[test]
fn disabled_regions_keep_portal_and_window_bsp_bytes_identical() {
    let (_directory, game) = game();
    for (window, foreground) in [(false, false), (true, false), (true, true)] {
        let (source, _, _) = fixture(window, foreground);
        let baseline = compiled(&source, &game).bsp.unwrap().to_bytes();
        let source = region(source, bounds(), false);
        let before = source.clone();
        let output = compiled(&source, &game);
        assert!(output.stats.compile_region.is_none());
        assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
        assert_eq!(source, before);
    }
}
