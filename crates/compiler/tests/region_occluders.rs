mod common;

use compiler::Control;
use glam::DVec3;
use map_core::{Document, ObjectId, compile_region, edit};
use source_formats::bsp::{Area, Bsp, Model, OCCLUDER_INACTIVE, Plane};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Connection, Entity, Vmf};

fn fixture() -> (tempfile::TempDir, GameFs, Document) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    for (name, flags) in [
        ("toolsblack", ""),
        ("toolsoccluder", "$translucent 1 %compilenolight 1"),
        ("toolsnodraw", "%compilenodraw 1"),
        ("toolstrigger", "%compiletrigger 1"),
        ("toolsorigin", "%compileorigin 1"),
        ("toolsskip", "%compileskip 1"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/tools/{name}.vmt")),
            format!("UnlitGeneric {{ {flags} }}"),
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut document = Document::new(common::room());
    document
        .edit("compile region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    (directory, game, document)
}

fn add_occluder(
    document: &mut Document,
    brushes: &[([f64; 3], [f64; 3])],
    point: DVec3,
    active: bool,
) -> u32 {
    let id = document
        .edit("occluder", |tx| {
            let solids = brushes
                .iter()
                .map(|&(mins, maxs)| {
                    edit::block_solid(
                        tx,
                        DVec3::from_array(mins),
                        DVec3::from_array(maxs),
                        "tools/toolsoccluder",
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            edit::create_entity(
                tx,
                "func_occluder",
                Some(point),
                &[
                    ("targetname".into(), "screen".into()),
                    ("StartActive".into(), if active { "1" } else { "0" }.into()),
                    ("occludernumber".into(), "777".into()),
                    ("model".into(), "*77".into()),
                ],
                solids,
            )
        })
        .unwrap();
    let ObjectId::Entity(id) = id else {
        unreachable!()
    };
    id
}

fn entity_mut(source: &mut Vmf, id: u32) -> &mut Entity {
    source
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn assert_rejected(source: &Vmf, game: &GameFs, path: &std::path::Path, message: &str) {
    let before = source.clone();
    for output in [
        compiler::compile(source, game),
        compiler::compile_controlled(source, game, &Control::default()),
    ] {
        assert!(output.bsp.is_none(), "{message}: {:?}", output.report);
        assert!(output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "{message}: {:?}",
            output.report
        );
        assert!(!output.write(path).unwrap());
        assert_eq!(std::fs::read(path).unwrap(), b"previous successful BSP");
    }
    assert_eq!(*source, before);
}

#[test]
fn contained_world_geometry_ignores_outside_markers_and_keeps_native_flags_and_indices() {
    let (_directory, game, mut document) = fixture();
    let baseline = compiler::compile(document.vmf(), &game);
    assert!(baseline.bsp.is_some(), "{:?}", baseline.report);
    let outside = add_occluder(
        &mut document,
        &[([160.0, -64.0, 16.0], [168.0, 64.0, 192.0])],
        DVec3::ZERO,
        true,
    );
    let enclosed = add_occluder(
        &mut document,
        &[
            ([-64.0, -64.0, 16.0], [-56.0, -16.0, 192.0]),
            ([-64.0, 16.0, 16.0], [-56.0, 64.0, 192.0]),
        ],
        DVec3::new(400.0, 400.0, 192.0),
        false,
    );
    let active = add_occluder(
        &mut document,
        &[([56.0, -64.0, 16.0], [64.0, 64.0, 192.0])],
        DVec3::new(-400.0, 400.0, 192.0),
        true,
    );
    let mut source = document.into_vmf();
    let entity = entity_mut(&mut source, enclosed);
    entity.properties.set("classname", "FUNC_OCCLUDER");
    entity.connections = Some(vec![Connection {
        output: "OnUser1".into(),
        value: "screen,Toggle,,0,-1".into(),
    }]);
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(source, before);
    assert_eq!(output.stats.occluders, 2);
    assert_eq!(output.stats.occluder_polygons, 18);
    assert_eq!(output.stats.occluder_vertex_refs, 72);
    assert_eq!(output.stats.brushes, baseline.stats.brushes);
    assert_eq!(output.stats.faces, baseline.stats.faces);
    assert_eq!(output.stats.clusters, baseline.stats.clusters);
    let region = output.stats.compile_region.as_ref().unwrap();
    assert!(region.omitted.contains(&format!("entity:{outside}")));
    for id in [enclosed, active] {
        assert!(region.included.contains(&format!("entity:{id}")));
    }
    let bytes = output.bsp.unwrap().to_bytes();
    let controlled = compiler::compile_controlled(&source, &game, &Control::default());
    assert!(!controlled.report.has_errors(), "{:?}", controlled.report);
    assert_eq!(controlled.bsp.unwrap().to_bytes(), bytes);
    let bsp = Bsp::parse(&bytes).unwrap();
    assert_eq!(bsp.records::<Model>().unwrap().len(), 1);
    let native = bsp.occlusion().unwrap().unwrap();
    assert_eq!(native.occluders.len(), 2);
    assert_eq!(native.occluders[0].flags, OCCLUDER_INACTIVE);
    assert_eq!(native.occluders[0].mins, [-64.0, -64.0, 16.0]);
    assert_eq!(native.occluders[0].maxs, [-56.0, 64.0, 192.0]);
    assert_eq!(native.occluders[0].polygon_count, 12);
    assert_eq!(native.occluders[1].flags, 0);
    assert_eq!(native.occluders[1].first_polygon, 12);
    let planes = bsp.records::<Plane>().unwrap();
    let vertices = bsp.vertices().unwrap();
    let areas = bsp.records::<Area>().unwrap();
    assert!(native.occluders.iter().all(|occluder| occluder.area > 0));
    native
        .validate(planes.len(), vertices.len(), areas.len())
        .unwrap();
    for polygon in &native.polygons {
        let plane = planes[polygon.plane as usize];
        let normal = DVec3::from_array(plane.normal.map(f64::from));
        let points = native.vertex_indices[polygon.first_vertex_index as usize..]
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
    assert_eq!(entities.matches("\"occludernumber\"").count(), 2);
    assert!(entities.contains("\"occludernumber\" \"0\""));
    assert!(entities.contains("\"occludernumber\" \"1\""));
    assert!(entities.contains("\"StartActive\" \"0\""));
    assert!(entities.contains("\"OnUser1\" \"screen,Toggle,,0,-1\""));
    assert!(entities.contains("\"origin\" \"400 400 192\""));
    assert!(!entities.contains("\"occludernumber\" \"777\""));
    assert!(!entities.contains("\"model\" \"*77\""));
}

#[test]
fn disjoint_helpers_and_fully_hidden_helpers_are_omitted_without_a_runtime_index() {
    let (_directory, game, mut document) = fixture();
    let baseline = compiler::compile(document.vmf(), &game)
        .bsp
        .unwrap()
        .to_bytes();
    let outside = add_occluder(
        &mut document,
        &[
            ([160.0, -64.0, 16.0], [168.0, -16.0, 192.0]),
            ([160.0, 16.0, 16.0], [168.0, 64.0, 192.0]),
        ],
        DVec3::new(0.0, 0.0, 64.0),
        true,
    );
    let hidden_children = add_occluder(
        &mut document,
        &[([-8.0, -64.0, 16.0], [8.0, 64.0, 192.0])],
        DVec3::ZERO,
        true,
    );
    let hidden_entity = add_occluder(
        &mut document,
        &[([-8.0, -64.0, 16.0], [8.0, 64.0, 192.0])],
        DVec3::ZERO,
        true,
    );
    let mut source = document.into_vmf();
    entity_mut(&mut source, hidden_children).solids[0].hidden = true;
    entity_mut(&mut source, hidden_entity).hidden = true;
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(source, before);
    assert_eq!(output.stats.occluders, 0);
    for id in [outside, hidden_children, hidden_entity] {
        assert!(
            output
                .stats
                .compile_region
                .as_ref()
                .unwrap()
                .omitted
                .contains(&format!("entity:{id}"))
        );
    }
    let bsp = output.bsp.unwrap();
    assert!(bsp.occlusion().unwrap().unwrap().occluders.is_empty());
    assert_eq!(bsp.to_bytes(), baseline);
}

#[test]
fn crossing_brushes_and_separated_children_on_both_sides_reject_without_truncation() {
    let (directory, game, document) = fixture();
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    for brushes in [
        vec![([120.0, -64.0, 16.0], [136.0, 64.0, 192.0])],
        vec![
            ([-64.0, -64.0, 16.0], [-56.0, 64.0, 192.0]),
            ([160.0, -64.0, 16.0], [168.0, 64.0, 192.0]),
        ],
        vec![
            ([-168.0, -64.0, 16.0], [-160.0, 64.0, 192.0]),
            ([160.0, -64.0, 16.0], [168.0, 64.0, 192.0]),
        ],
    ] {
        let mut document = document.clone();
        let id = add_occluder(
            &mut document,
            &brushes,
            DVec3::new(400.0, 400.0, 192.0),
            true,
        );
        assert_rejected(
            document.vmf(),
            &game,
            &path,
            &format!("every visible brush in the helper or disable the region (entity:{id})"),
        );
    }
}

#[test]
fn hidden_crossing_and_invalid_children_do_not_affect_the_visible_geometry() {
    let (_directory, game, mut document) = fixture();
    let id = add_occluder(
        &mut document,
        &[
            ([-64.0, -64.0, 16.0], [-56.0, 64.0, 192.0]),
            ([120.0, -64.0, 16.0], [136.0, 64.0, 192.0]),
        ],
        DVec3::new(400.0, 400.0, 192.0),
        true,
    );
    let mut source = document.into_vmf();
    let hidden = &mut entity_mut(&mut source, id).solids[1];
    hidden.hidden = true;
    hidden.sides.clear();
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluders, 1);
    assert_eq!(output.stats.occluder_polygons, 6);
    assert_eq!(source, before);
    let native = output.bsp.unwrap().occlusion().unwrap().unwrap();
    assert_eq!(native.occluders[0].maxs, [-56.0, 64.0, 192.0]);
}

#[test]
fn crossing_checks_include_nodraw_sides_in_the_whole_brush_bounds() {
    let (directory, game, mut document) = fixture();
    let id = add_occluder(
        &mut document,
        &[([120.0, -64.0, 16.0], [136.0, 64.0, 192.0])],
        DVec3::ZERO,
        true,
    );
    let mut source = document.into_vmf();
    for side in &mut entity_mut(&mut source, id).solids[0].sides {
        let plane = map_core::Plane::from_points(side.plane.0).unwrap();
        side.material = if plane.normal.x < -0.99 {
            "tools/toolstrigger"
        } else {
            "tools/toolsnodraw"
        }
        .into();
    }
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    assert_rejected(
        &source,
        &game,
        &path,
        "func_occluder crosses a compile-region boundary",
    );
}

#[test]
fn retained_terrain_helpers_use_the_ordinary_occluder_geometry_guard() {
    let (directory, game, mut document) = fixture();
    let id = document
        .edit("terrain helper", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-8.0, -64.0, 16.0),
                DVec3::new(8.0, 64.0, 192.0),
                "tools/toolsoccluder",
            )?
            else {
                unreachable!()
            };
            let side = tx.solid_mut(solid)?.sides[0].id;
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )?;
            edit::tie_to_entity(tx, &[solid], "func_occluder", &[])
        })
        .unwrap();
    let ObjectId::Entity(id) = id else {
        unreachable!()
    };
    let mut source = document.into_vmf();
    entity_mut(&mut source, id)
        .properties
        .set("origin", "400 400 192");
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    assert_rejected(&source, &game, &path, "without terrain or origin materials");
}

#[test]
fn empty_and_invalid_retained_helpers_keep_ordinary_occluder_rejection() {
    let (directory, game, mut document) = fixture();
    let id = add_occluder(
        &mut document,
        &[
            ([-64.0, -64.0, 16.0], [-56.0, 64.0, 192.0]),
            ([56.0, -64.0, 16.0], [64.0, 64.0, 192.0]),
        ],
        DVec3::new(400.0, 400.0, 192.0),
        true,
    );
    let source = document.into_vmf();
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    for case in 0..8 {
        let mut invalid = source.clone();
        let entity = entity_mut(&mut invalid, id);
        let message = match case {
            0 => {
                entity.solids.clear();
                "Occluders require 1–16 visible convex brushes"
            }
            1 => {
                entity.properties.set("StartActive", "2");
                "Occluder Start Active must be 0 or 1"
            }
            2 => {
                entity.properties.set("parentname", "door");
                "Occluder polygons are fixed in world space"
            }
            3 => {
                for solid in &mut entity.solids {
                    for side in &mut solid.sides {
                        side.material = "tools/toolsnodraw".into();
                    }
                }
                "Occluder has no active faces"
            }
            4 => {
                entity.solids[1].sides[0].material = "missing/occluder".into();
                "Occluder material missing/occluder could not be loaded"
            }
            5 => {
                entity.solids[1].sides[0].material = "tools/toolsorigin".into();
                "without terrain or origin materials"
            }
            6 => {
                edit::translate_solid(&mut entity.solids[1], DVec3::X * -120.0, true);
                "Occluder brush bounds touch or overlap"
            }
            7 => {
                entity.solids[1].sides[0].material = "tools/toolsskip".into();
                "Occluder sides cannot use hint, skip"
            }
            _ => unreachable!(),
        };
        assert_rejected(&invalid, &game, &path, message);
        let output = compiler::compile(&invalid, &game);
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
    }
}

#[test]
fn malformed_brushes_outside_the_region_are_rejected_without_using_the_marker() {
    let (directory, game, mut document) = fixture();
    let id = add_occluder(
        &mut document,
        &[([160.0, -64.0, 16.0], [168.0, 64.0, 192.0])],
        DVec3::new(400.0, 400.0, 192.0),
        true,
    );
    let source = document.into_vmf();
    let path = directory.path().join("preserved.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    for case in 0..2 {
        let mut invalid = source.clone();
        let solid = &mut entity_mut(&mut invalid, id).solids[0];
        let solid_id = solid.id;
        if case == 0 {
            solid.sides.clear();
        } else {
            solid.sides[0].plane.0[0].x = f64::INFINITY;
        }
        assert_rejected(
            &invalid,
            &game,
            &path,
            &format!("invalid func_occluder (entity:{id}, solid:{solid_id})"),
        );
    }
}

#[test]
fn disabled_region_preserves_exact_full_map_occluder_bytes() {
    let (_directory, game, mut document) = fixture();
    add_occluder(
        &mut document,
        &[([160.0, -64.0, 16.0], [168.0, 64.0, 192.0])],
        DVec3::new(400.0, 400.0, 192.0),
        false,
    );
    let mut source = document.into_vmf();
    let bounds = compile_region::inspect(&source).unwrap().unwrap().bounds;
    source.cordon = None;
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    let mut document = Document::new(source);
    document
        .edit("disable compile region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: false,
                    bounds,
                }),
            )
        })
        .unwrap();
    let source = document.into_vmf();
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.occluders, 1);
    assert!(output.stats.compile_region.is_none());
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    assert_eq!(source, before);
}
