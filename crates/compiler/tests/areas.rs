mod common;
#[path = "common/areas.rs"]
mod fixtures;

use fixtures::{room, window_room};
use glam::DVec3;
use source_formats::bsp::{Bsp, Leaf, Model, Node, Plane, contents};

#[test]
fn authored_portal_splits_two_areas_and_emits_reciprocal_runtime_bindings() {
    let (mut vmf, id) = room(true);
    let portal = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap();
    portal.properties.set("PortalVersion", "0");
    portal.properties.set("portalnumber", "999");
    portal.properties.set("model", "*77");
    let original = vmf.clone();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.areas, 2);
    assert_eq!(output.stats.areaportals, 1);
    assert_eq!(output.stats.areaportal_vertices, 8);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let areas = bsp.area_data().unwrap().unwrap();
    assert_eq!(areas.areas.len(), 3);
    assert_eq!(areas.portals.len(), 3);
    assert_eq!(areas.portals[1].portal_key, 1);
    assert_eq!(areas.portals[2].portal_key, 1);
    assert_eq!(areas.portals[1].plane ^ 1, areas.portals[2].plane);
    let polygons = areas.portals[1..]
        .iter()
        .map(|portal| {
            let first = portal.first_clip_vert as usize;
            &areas.clip_vertices[first..first + portal.clip_vert_count as usize]
        })
        .collect::<Vec<_>>();
    assert_eq!(polygons[0], polygons[1]);
    let polygon = polygons[0];
    let a = DVec3::from_array(polygon[0].map(f64::from));
    let b = DVec3::from_array(polygon[1].map(f64::from));
    let c = DVec3::from_array(polygon[2].map(f64::from));
    let planes = bsp.records::<Plane>().unwrap();
    let normal = DVec3::from_array(
        planes[(areas.portals[1].plane & !1) as usize]
            .normal
            .map(f64::from),
    );
    assert!((b - a).cross(c - a).dot(normal) > 0.0);
    assert_eq!(bsp.records::<Model>().unwrap().len(), 1);
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"portalnumber\" \"1\""));
    assert!(entities.contains("\"PortalVersion\" \"1\""));
    assert!(entities.contains("\"targetname\" \"partition_gate\""));
    assert!(entities.contains(&format!("\"hammerid\" \"{id}\"")));
    assert!(!entities.contains("\"model\" \"*1\""));
    let leaves = bsp.records::<Leaf>().unwrap();
    let nodes = bsp.records::<Node>().unwrap();
    for node in &nodes {
        let child_areas = node.children.map(|child| {
            if child < 0 {
                leaves[(-child - 1) as usize].area()
            } else {
                nodes[child as usize].area
            }
        });
        let expected = if child_areas[0] == child_areas[1] {
            child_areas[0]
        } else {
            -1
        };
        assert_eq!(node.area, expected);
    }
    assert!(
        leaves
            .iter()
            .any(|leaf| leaf.area() == 1 && leaf.contents & contents::SOLID == 0)
    );
    assert!(
        leaves
            .iter()
            .any(|leaf| leaf.area() == 2 && leaf.contents & contents::SOLID == 0)
    );
    assert!(
        leaves
            .iter()
            .any(|leaf| leaf.contents & contents::AREAPORTAL != 0
                && leaf.contents & contents::SOLID == 0)
    );
    assert_eq!(vmf, original);
}

#[test]
fn hidden_portals_and_hidden_child_brushes_do_not_create_native_area_controls() {
    for hide_child in [false, true] {
        let (mut vmf, id) = room(true);
        let portal = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        if hide_child {
            portal.solids[0].hidden = true;
        } else {
            portal.hidden = true;
        }
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.areaportals, 0);
        assert_eq!(output.stats.areas, 1);
        let bsp = output.bsp.unwrap();
        assert_eq!(bsp.area_data().unwrap().unwrap().portals.len(), 1);
        assert!(!bsp.entities().unwrap().contains("func_areaportal"));
    }
}

#[test]
fn leaking_area_boundaries_and_cancelled_builds_preserve_previous_output() {
    let (vmf, _) = room(false);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous area build").unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("two distinct areas"))
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous area build");
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let output = compiler::compile_with_options_controlled(
        &room(true).0,
        &common::empty_game(),
        Default::default(),
        &control,
    );
    assert!(output.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous area build");
}

#[test]
fn overlapping_portal_volumes_fail_instead_of_emitting_ambiguous_area_keys() {
    let (mut vmf, id) = room(true);
    let mut copy = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    copy.id += 1000;
    for solid in &mut copy.solids {
        solid.id += 1000;
        for side in &mut solid.sides {
            side.id += 1000;
        }
    }
    vmf.entities.push(copy);
    let original = vmf.clone();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("overlap"))
    );
    assert!(output.bsp.is_none());
    assert_eq!(vmf, original);
}

#[test]
fn window_portals_keep_render_targets_pivots_defaults_and_native_area_geometry() {
    for foreground in [false, true] {
        let (mut vmf, id, _, _) = window_room(foreground);
        let portal = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        portal.properties.set("PortalVersion", "0");
        portal.properties.set("model", "*99");
        portal.properties.set("portalnumber", "999");
        let original = vmf.clone();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.areas, 2);
        assert_eq!(output.stats.areaportals, 1);
        assert_eq!(output.stats.areaportal_windows, 1);
        let bsp = output.bsp.unwrap();
        assert_eq!(
            bsp.records::<Model>().unwrap().len(),
            if foreground { 3 } else { 2 }
        );
        assert_eq!(bsp.area_data().unwrap().unwrap().portals.len(), 3);
        let entities = bsp.entities().unwrap();
        let window = entities
            .split("{\n")
            .find(|entity| entity.contains("\"classname\" \"func_areaportalwindow\""))
            .unwrap();
        for field in [
            "\"target\" \"rendered_window\"",
            "\"PortalVersion\" \"1\"",
            "\"FadeStartDist\" \"128\"",
            "\"FadeDist\" \"512\"",
            "\"TranslucencyLimit\" \"0.2\"",
        ] {
            assert!(window.contains(field), "{window}");
        }
        assert!(!window.contains("\"model\""));
        assert!(entities.contains("\"origin\" \"0.5 -0.25 96.25\""));
        assert_eq!(vmf, original);
    }
}

#[test]
fn window_portals_reject_unsafe_fades_and_missing_ambiguous_or_undersized_models() {
    for case in 0..11 {
        let (mut vmf, id, target, foreground) = window_room(true);
        let portal = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        match case {
            0 => portal.properties.set("target", "missing_model"),
            1 => portal.properties.set("FadeDist", "NaN"),
            2 => portal.properties.set("FadeStartDist", "-1"),
            3 => portal.properties.set("FadeDist", "64"),
            4 => portal.properties.set("TranslucencyLimit", "1.1"),
            5 => portal
                .properties
                .set("BackgroundBModel", "missing_foreground"),
            6 => {
                vmf.entities
                    .iter_mut()
                    .find(|entity| entity.id == target)
                    .unwrap()
                    .hidden = true
            }
            7 => vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == target)
                .unwrap()
                .solids
                .clear(),
            8 => vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == foreground.unwrap())
                .unwrap()
                .properties
                .set("targetname", "rendered_window"),
            9 => vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == foreground.unwrap())
                .unwrap()
                .properties
                .set("origin", "0 0 0"),
            10 => {
                let entity = vmf
                    .entities
                    .iter_mut()
                    .find(|entity| entity.id == target)
                    .unwrap();
                for side in &mut entity.solids[0].sides {
                    for point in &mut side.plane.0 {
                        point.y *= 0.5;
                    }
                }
            }
            _ => unreachable!(),
        }
        let original = vmf.clone();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(
            output.report.has_errors(),
            "case {case}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert_eq!(vmf, original);
    }
}

#[test]
fn hidden_window_portals_leave_their_render_models_and_equal_fades_are_valid() {
    let (mut vmf, id, _, _) = window_room(false);
    let portal = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap();
    portal.properties.set("FadeStartDist", "128");
    portal.properties.set("FadeDist", "128");
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .hidden = true;
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.areaportal_windows, 0);
    assert_eq!(output.stats.areaportals, 0);
    assert_eq!(output.bsp.unwrap().records::<Model>().unwrap().len(), 2);
}
