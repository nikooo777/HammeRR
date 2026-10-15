mod common;

use compiler::map::{MAX_COMPILE_COORD, Map};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{self, Model};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Vmf, format_vec3};

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/origin.vmt"),
        "UnlitGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn fixture(class: &str, volume: bool, oblique: bool) -> (Vmf, u32, Option<u32>, u32) {
    let mut document = Document::new(common::room());
    let (body, origin, owner) = document
        .edit("brush entity", |tx| {
            let ObjectId::Solid(body) = edit::create_block(
                tx,
                DVec3::new(0.125, 0.25, 64.375),
                DVec3::new(64.125, 64.25, 128.375),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(body)?.sides {
                side.u_axis.axis = DVec3::new(1.25, 0.375, 0.5);
                side.v_axis.axis = DVec3::new(-0.25, -1.5, 0.125);
                side.u_axis.scale = 0.375;
                side.v_axis.scale = -0.625;
                side.u_axis.offset = 13.125;
                side.v_axis.offset = -27.5;
                side.lightmap_scale = 8;
            }
            if oblique {
                edit::transform(
                    tx,
                    &[ObjectId::Solid(body)],
                    map_core::transform::Transform {
                        rotation: DVec3::new(17.0, -23.0, 31.0),
                        pivot: DVec3::new(32.125, 32.25, 96.375),
                        ..Default::default()
                    },
                )?;
            }
            let origin = if volume {
                let center = DVec3::new(32.125, 16.25, 64.375);
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    center - DVec3::splat(4.0),
                    center + DVec3::splat(4.0),
                    "origin",
                )?
                else {
                    unreachable!()
                };
                Some(id)
            } else {
                None
            };
            let mut solids = vec![body];
            solids.extend(origin);
            let ObjectId::Entity(owner) = edit::tie_to_entity(
                tx,
                &solids,
                class,
                &[
                    ("origin".into(), "80.125 48.25 96.375".into()),
                    ("targetname".into(), "pivot_test".into()),
                    ("angles".into(), "0 0 0".into()),
                ],
            )?
            else {
                unreachable!()
            };
            Ok((body, origin, owner))
        })
        .unwrap();
    (document.vmf().clone(), body, origin, owner)
}

fn projected(row: [f32; 4], point: DVec3) -> f64 {
    point.dot(DVec3::new(
        f64::from(row[0]),
        f64::from(row[1]),
        f64::from(row[2]),
    )) + f64::from(row[3])
}

#[test]
fn origin_volumes_and_explicit_keys_preserve_local_geometry_uvs_and_bsp_models() {
    let (_directory, game) = game();
    for class in ["func_brush", "func_door_rotating", "trigger_multiple"] {
        for volume in [false, true] {
            for oblique in [false, true] {
                let (vmf, body, origin, owner) = fixture(class, volume, oblique);
                let source = vmf.clone();
                let mut report = compiler::Report::default();
                let map = Map::load(&vmf, &game, &mut report).unwrap();
                assert!(!report.has_errors(), "{report:?}");
                let entity = map
                    .entities
                    .iter()
                    .find(|entity| entity.id == owner)
                    .unwrap();
                let expected = if volume {
                    DVec3::new(32.125, 16.25, 64.375)
                } else {
                    DVec3::new(80.125, 48.25, 96.375)
                };
                let pivot = entity.origin.unwrap();
                assert!(pivot.abs_diff_eq(expected, 1e-8));
                assert!(
                    origin.is_none_or(|origin| map.brushes.iter().all(|brush| brush.id != origin))
                );
                let brush = map.brushes.iter().find(|brush| brush.id == body).unwrap();
                let original = vmf
                    .entities
                    .iter()
                    .find(|entity| entity.id == owner)
                    .unwrap()
                    .solids
                    .iter()
                    .find(|solid| solid.id == body)
                    .unwrap();
                let shape = map_core::BrushGeometry::from_solid(original).unwrap();
                assert!(brush.mins.abs_diff_eq(shape.mins - pivot, 1e-8));
                assert!(brush.maxs.abs_diff_eq(shape.maxs - pivot, 1e-8));
                for (side, face) in brush
                    .sides
                    .iter()
                    .filter(|side| !side.bevel)
                    .zip(&shape.faces)
                {
                    let source_side = &original.sides[face.side];
                    let info = &map.texinfos.infos[side.texinfo];
                    for (local, world) in
                        side.winding.as_ref().unwrap().0.iter().zip(&face.vertices)
                    {
                        assert!(local.abs_diff_eq(*world - pivot, 1e-8));
                        for (index, axis) in
                            [source_side.u_axis, source_side.v_axis].iter().enumerate()
                        {
                            let texel = world.dot(axis.axis) / axis.scale + axis.offset;
                            let luxel = (world.dot(axis.axis) + axis.offset * axis.scale)
                                / f64::from(source_side.lightmap_scale);
                            assert!(
                                (projected(info.texture_vecs[index], *local) - texel).abs() < 1e-4
                            );
                            assert!(
                                (projected(info.lightmap_vecs[index], *local) - luxel).abs() < 1e-4
                            );
                        }
                        assert!(
                            brush.sides.iter().all(|side| map
                                .planes
                                .get(side.plane)
                                .distance(*local)
                                <= 0.011)
                        );
                    }
                }
                let reopened = Vmf::parse(&vmf.write(&Default::default())).unwrap();
                let output = compiler::compile(&reopened, &game);
                assert!(!output.report.has_errors(), "{:?}", output.report);
                assert!(output.leak.is_none());
                assert_eq!(output.stats.brushes, 7);
                let bsp = output.bsp.unwrap();
                let models = bsp.records::<Model>().unwrap();
                assert_eq!(models.len(), 2);
                assert_eq!(models[1].origin, [0.0; 3]);
                assert!(
                    DVec3::from_array(models[1].mins.map(f64::from))
                        .abs_diff_eq(shape.mins - pivot, 1e-4)
                );
                assert!(
                    DVec3::from_array(models[1].maxs.map(f64::from))
                        .abs_diff_eq(shape.maxs - pivot, 1e-4)
                );
                assert!(
                    bsp.entities()
                        .unwrap()
                        .contains(&format!("\"origin\" \"{}\"", format_vec3(pivot)))
                );
                assert!(bsp.entities().unwrap().contains("\"model\" \"*1\""));
                assert!(
                    bsp.entities()
                        .unwrap()
                        .contains("\"targetname\" \"pivot_test\"")
                );
                assert!(
                    bsp.records::<bsp::Brush>()
                        .unwrap()
                        .iter()
                        .all(|brush| brush.contents & bsp::contents::ORIGIN == 0)
                );
                assert_eq!(vmf, source);
            }
        }
    }
}

#[test]
fn hidden_origin_volumes_do_not_override_explicit_entity_origins() {
    let (_directory, game) = game();
    let (mut vmf, _, origin, owner) = fixture("func_brush", true, false);
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == owner)
        .unwrap()
        .solids
        .iter_mut()
        .find(|solid| Some(solid.id) == origin)
        .unwrap()
        .hidden = true;
    let mut report = compiler::Report::default();
    let map = Map::load(&vmf, &game, &mut report).unwrap();
    assert!(!report.has_errors());
    assert_eq!(
        map.entities
            .iter()
            .find(|entity| entity.id == owner)
            .unwrap()
            .origin,
        Some(DVec3::new(80.125, 48.25, 96.375))
    );
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
}

#[test]
fn origin_volume_order_and_zero_pivots_do_not_change_body_output() {
    let (_directory, game) = game();
    let (mut vmf, _, origin, owner) = fixture("func_brush", true, true);
    let entity = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == owner)
        .unwrap();
    let volume = entity
        .solids
        .iter_mut()
        .find(|solid| Some(solid.id) == origin)
        .unwrap();
    for side in &mut volume.sides {
        for point in &mut side.plane.0 {
            *point -= DVec3::new(32.125, 16.25, 64.375);
        }
    }
    let first = compiler::compile(&vmf, &game);
    assert!(!first.report.has_errors(), "{:?}", first.report);
    let first = first.bsp.unwrap();
    assert!(first.entities().unwrap().contains("\"origin\" \"0 0 0\""));
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == owner)
        .unwrap()
        .solids
        .reverse();
    let second = compiler::compile(&vmf, &game);
    assert!(!second.report.has_errors(), "{:?}", second.report);
    assert_eq!(first.to_bytes(), second.bsp.unwrap().to_bytes());
}

#[test]
fn invalid_origin_volumes_fail_with_actionable_diagnostics() {
    let (_directory, game) = game();
    for mode in 0..6 {
        let (mut vmf, body, origin, owner) = fixture("func_brush", true, false);
        let entity = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == owner)
            .unwrap();
        match mode {
            0 => {
                let mut copy = entity
                    .solids
                    .iter()
                    .find(|solid| Some(solid.id) == origin)
                    .unwrap()
                    .clone();
                copy.id += 1000;
                for side in &mut copy.sides {
                    side.id += 1000;
                }
                entity.solids.push(copy);
            }
            1 => {
                entity.properties.set("classname", "func_detail");
            }
            2 => {
                entity.solids.retain(|solid| solid.id != body);
            }
            3 => {
                entity
                    .solids
                    .iter_mut()
                    .find(|solid| Some(solid.id) == origin)
                    .unwrap()
                    .sides[0]
                    .plane
                    .0[0]
                    .x = f64::NAN;
            }
            4 => {
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| Some(solid.id) == origin)
                    .unwrap();
                let solid = entity.solids.remove(index);
                vmf.world.solids.push(solid);
            }
            5 => {
                entity.properties.set("origin", "NaN 0 0");
            }
            _ => unreachable!(),
        }
        let output = compiler::compile(&vmf, &game);
        assert!(
            output.report.has_errors(),
            "mode={mode}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == compiler::Level::Error
                    && diagnostic.object.is_some())
        );
    }
}

#[test]
fn world_and_model_local_bounds_are_both_validated_before_bsp_construction() {
    let (_directory, game) = game();
    let (mut vmf, body, _, owner) = fixture("func_brush", false, false);
    let entity = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == owner)
        .unwrap();
    entity
        .properties
        .set("origin", format_vec3(DVec3::X * MAX_COMPILE_COORD));
    let solid = entity
        .solids
        .iter_mut()
        .find(|solid| solid.id == body)
        .unwrap();
    for side in &mut solid.sides {
        for point in &mut side.plane.0 {
            *point -= DVec3::X * (MAX_COMPILE_COORD - 128.0);
        }
    }
    let output = compiler::compile(&vmf, &game);
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local"))
    );
    let entity = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == owner)
        .unwrap();
    entity.properties.set("origin", "0 0 0");
    for side in &mut entity.solids[0].sides {
        for point in &mut side.plane.0 {
            *point += DVec3::X * (MAX_COMPILE_COORD * 3.0);
        }
    }
    let output = compiler::compile(&vmf, &game);
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
}
