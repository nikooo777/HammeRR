mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit};
use source_formats::bsp::{self, Bsp, Leaf, LeafWaterData};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    for (name, text) in [
        ("water", "Water { $abovewater 1 }"),
        ("water2", "Water { $abovewater 1 }"),
        (
            "slime",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime }",
        ),
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn add_pool(vmf: Vmf, mins: [f64; 3], maxs: [f64; 3], material: &str) -> (Vmf, u32) {
    let mut document = Document::new(vmf);
    let id = document
        .edit("pool", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::from_array(mins),
                DVec3::from_array(maxs),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = material.into();
                }
            }
            Ok(id)
        })
        .unwrap();
    (document.vmf().clone(), id)
}

fn add_water_analog(vmf: Vmf, mins: [f64; 3], maxs: [f64; 3], material: &str) -> (Vmf, u32) {
    let (vmf, solid) = add_pool(vmf, mins, maxs, material);
    let mut document = Document::new(vmf);
    let ObjectId::Entity(id) = document
        .edit("water analog", |tx| {
            edit::tie_to_entity(tx, &[solid], "FuNc_WaTeR_AnAlOg", &[])
        })
        .unwrap()
    else {
        unreachable!()
    };
    (document.vmf().clone(), id)
}

fn compiled(vmf: &Vmf, game: &GameFs) -> compiler::Output {
    let output = compiler::compile(vmf, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert!(output.bsp.is_some());
    output
}

#[test]
fn pool_emits_native_leaf_surface_and_distance_records_without_changing_source() {
    let (_directory, game) = game();
    let (vmf, _) = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        "water",
    );
    let source = vmf.clone();
    let output = compiled(&vmf, &game);
    assert_eq!(output.stats.water_regions, 1);
    assert!(output.stats.water_leaves > 0);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let water = bsp.records::<LeafWaterData>().unwrap();
    assert_eq!(water.len(), 1);
    assert_eq!(water[0].surface_z, 64.0);
    assert_eq!(water[0].min_z, 0.0);
    assert_eq!(water[0].padding, 0);
    let texinfos = bsp.records::<bsp::TexInfo>().unwrap();
    assert!(texinfos[water[0].surface_texinfo as usize].flags & bsp::surf::WARP != 0);
    let leaves = bsp.records::<Leaf>().unwrap();
    let distances = bsp.values::<u16>(bsp::lump::LEAFMINDISTTOWATER).unwrap();
    assert_eq!(distances.len(), leaves.len());
    assert!(
        leaves
            .iter()
            .any(|leaf| leaf.contents & bsp::contents::TESTFOGVOLUME != 0)
    );
    assert_eq!(
        leaves.iter().filter(|leaf| leaf.water_data >= 0).count(),
        output.stats.water_leaves
    );
    for (leaf, distance) in leaves.iter().zip(distances) {
        if leaf.water_data >= 0 {
            assert_eq!(leaf.contents & bsp::contents::TESTFOGVOLUME, 0);
            assert_eq!(leaf.water_data, 0);
            assert!(leaf.contents & bsp::contents::WATER != 0);
            assert!(leaf.contents & bsp::contents::SOLID == 0);
            assert_eq!(distance, 0);
        } else if leaf.contents & bsp::contents::SOLID != 0 {
            assert_eq!(leaf.water_data, -1);
            assert_eq!(leaf.contents & bsp::contents::TESTFOGVOLUME, 0);
        }
    }
    let faces = bsp.records::<bsp::Face>().unwrap();
    let water_faces: Vec<_> = faces
        .iter()
        .filter(|face| texinfos[face.texinfo as usize].flags & bsp::surf::WARP != 0)
        .collect();
    assert!(!water_faces.is_empty());
    assert!(water_faces.iter().all(|face| face.fog_volume == 0));
    assert_eq!(
        faces,
        bsp.values::<bsp::Face>(bsp::lump::ORIGINALFACES).unwrap()
    );
    let entities = bsp.entities().unwrap();
    assert_eq!(
        entities
            .matches("\"classname\" \"water_lod_control\"")
            .count(),
        1
    );
    assert!(entities.contains("\"cheapwaterstartdistance\" \"1000\""));
    assert!(entities.contains("\"cheapwaterenddistance\" \"2000\""));
    assert_eq!(vmf, source);
}

#[test]
fn opaque_slime_pairs_every_exposed_boundary_with_the_same_material_and_native_contents() {
    let (_directory, game) = game();
    let (mut source, _) = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        "slime",
    );
    let pool = source.world.solids.last_mut().unwrap();
    for side in &mut pool.sides {
        side.material = "slime".into();
    }
    let original = source.clone();
    let output = compiled(&source, &game);
    let bsp = output.bsp.as_ref().unwrap();
    let regions = bsp.records::<LeafWaterData>().unwrap();
    assert_eq!(regions.len(), 1);
    assert_eq!((regions[0].surface_z, regions[0].min_z), (96.0, 32.0));
    let infos = bsp.records::<bsp::TexInfo>().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let references = bsp.values::<u16>(bsp::lump::LEAFFACES).unwrap();
    let faces = bsp.records::<bsp::Face>().unwrap();
    let mut areas = [0.0_f32; 2];
    let mut counts = [0; 2];
    for (index, face) in faces.iter().enumerate() {
        let info = infos[face.texinfo as usize];
        if names[data[info.texdata as usize].name_index as usize] != "SLIME" {
            continue;
        }
        assert_ne!(info.flags & bsp::surf::NODECALS, 0);
        assert_eq!(info.flags & (bsp::surf::WARP | bsp::surf::NOLIGHT), 0);
        assert_eq!(face.fog_volume, -1);
        let membership = leaves
            .iter()
            .filter(|leaf| {
                references[leaf.first_leaf_face as usize
                    ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize]
                    .contains(&(index as u16))
            })
            .collect::<Vec<_>>();
        assert!(!membership.is_empty());
        let wet = membership[0].contents & bsp::contents::SLIME != 0;
        assert!(
            membership
                .iter()
                .all(|leaf| (leaf.contents & bsp::contents::SLIME != 0) == wet)
        );
        let normal = DVec3::from_array(planes[face.plane as usize].normal.map(f64::from));
        let center = DVec3::new(0.0, 0.0, 64.0);
        assert_eq!(
            normal.dot(center) > f64::from(planes[face.plane as usize].dist),
            wet
        );
        assert_eq!(
            output
                .face_origins
                .as_ref()
                .unwrap()
                .contributors(index)
                .unwrap()
                .next()
                .is_none(),
            wet
        );
        areas[usize::from(wet)] += face.area;
        counts[usize::from(wet)] += 1;
    }
    assert_eq!(areas, [196_608.0; 2]);
    assert_eq!(counts, [6, 6]);
    assert!(leaves.iter().any(|leaf| leaf.water_data == 0));
    for leaf in &leaves {
        assert_eq!(leaf.contents & bsp::contents::TESTFOGVOLUME, 0);
        if leaf.water_data >= 0 {
            assert_eq!(
                leaf.contents & (bsp::contents::WATER | bsp::contents::SLIME),
                bsp::contents::SLIME
            );
            assert_eq!(leaf.water_data, 0);
        }
    }
    assert_eq!(source, original);
}

#[test]
fn slime_inward_faces_intersect_both_dry_and_wet_boundary_cuts() {
    let (_directory, game) = game();
    let (source, _) = add_pool(
        common::room(),
        [-128.0, -128.0, 32.0],
        [128.0, 128.0, 96.0],
        "slime",
    );
    let mut document = Document::new(source);
    document
        .edit("crossed slime boundaries", |tx| {
            for (mins, maxs) in [
                ([-64.0, -128.0, 96.0], [0.0, 128.0, 128.0]),
                ([-128.0, -16.0, 32.0], [128.0, 16.0, 96.0]),
            ] {
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
    let output = compiled(document.vmf(), &game);
    let bsp = output.bsp.unwrap();
    let infos = bsp.records::<bsp::TexInfo>().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let mut areas = [0.0_f32; 2];
    for face in bsp.records::<bsp::Face>().unwrap() {
        if names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize] == "SLIME"
        {
            let plane = planes[face.plane as usize];
            assert_eq!(plane.normal[..2], [0.0; 2]);
            areas[usize::from(plane.normal[2] < 0.0)] += face.area;
        }
    }
    assert_eq!(areas, [43_008.0; 2]);
}

#[test]
fn unsupported_slime_combinations_fail_without_replacing_previous_output() {
    let (directory, game) = game();
    let (source, _) = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        "slime",
    );
    let previous = tempfile::tempdir().unwrap();
    let path = previous.path().join("previous.bsp");
    std::fs::write(&path, b"previous").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    std::fs::write(
        directory.path().join("materials/slime_base.vmt"),
        "LightmappedGeneric { %compileslime 1 $additive 0 $bottommaterial slime }",
    )
    .unwrap();
    let original = source.clone();
    for text in [
        "LightmappedGeneric { %compileslime 1 }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial \"\" }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial missing }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime_base }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial materials/slime.vmt }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime.vmt }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial \" slime \" }",
        "Water { %compileslime 1 $bottommaterial slime }",
        "Water_DX90 { %compileslime 1 $bottommaterial slime }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime $translucent 1 }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime $alphatest 1 }",
        "UnlitGeneric { %compileslime 1 $bottommaterial slime $additive 1 }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime $vertexalpha 1 }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime Proxies { Sine { resultVar $alpha } } }",
        "LightmappedGeneric { %compileslime 1 $bottommaterial slime >=DX90 { $alpha 0.5 } }",
        "Patch { include materials/slime_base.vmt replace { $bottommaterial slime_base } }",
        "Patch { include materials/slime_base.vmt replace { $additive 1 } }",
        "Patch { include materials/slime_base.vmt insert { Proxies { Sine { resultVar $alpha } } } }",
        "Refract { %compileslime 1 $bottommaterial slime }",
    ] {
        std::fs::write(directory.path().join("materials/slime.vmt"), text).unwrap();
        let output = compiler::compile(&source, &game);
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{text}: {:?}",
            output.report
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("Static slime requires opaque"))
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
        assert_eq!(
            std::fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(source, original);
    }
}

#[test]
fn slime_material_semantics_remain_owned_until_checked_publication() {
    for (name, material, changed) in [
        (
            "slime",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime }",
            "LightmappedGeneric { %compileslime 0 $bottommaterial slime }",
        ),
        (
            "slime",
            "UnlitGeneric { %compileslime 1 $bottommaterial SLIME }",
            "UnlitGeneric { %compileslime 1 $bottommaterial another_material }",
        ),
        (
            "slime_base",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime $alpha 1 $additive 0 $envmap env_cubemap }",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime $alpha 1 $additive 1 $envmap env_cubemap }",
        ),
    ] {
        let (directory, game) = game();
        let material_path = directory.path().join(format!("materials/{name}.vmt"));
        std::fs::write(&material_path, material).unwrap();
        if name == "slime_base" {
            std::fs::write(directory.path().join("materials/slime.vmt"), "Patch { include materials/slime_base.vmt insert { $vertexalpha 0 } replace { $alpha 1.0 } }").unwrap();
        }
        let (source, _) = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [128.0, 128.0, 64.0],
            "slime",
        );
        let previous = tempfile::tempdir().unwrap();
        let path = previous.path().join("previous.bsp");
        std::fs::write(&path, b"previous BSP").unwrap();
        std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
        let control = compiler::Control::default();
        let lease = compiler::OutputLease::acquire(&path, &control).unwrap();
        let reservation = compiler::WriteReservation::new(&path, &control).unwrap();
        let output = compiled(&source, &game);
        assert_eq!(output.stats.water_regions, 1);
        assert!(
            output
                .bsp
                .as_ref()
                .unwrap()
                .records::<Leaf>()
                .unwrap()
                .iter()
                .any(|leaf| leaf.water_data >= 0 && leaf.contents & bsp::contents::SLIME != 0)
        );
        let prepared = output
            .prepare_write_reserved(reservation, &control)
            .unwrap()
            .unwrap();
        std::fs::write(&material_path, changed).unwrap();
        assert!(prepared.commit_checked(&lease, &control).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn connected_pools_share_depth_while_material_and_height_boundaries_keep_separate_regions() {
    let (_directory, game) = game();
    for mode in 0..3 {
        let (vmf, _) = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [0.0, 128.0, 64.0],
            "water",
        );
        let (vmf, _) = add_pool(
            vmf,
            [0.0, -128.0, 32.0],
            [128.0, 128.0, if mode == 2 { 96.0 } else { 64.0 }],
            if mode == 1 { "water2" } else { "water" },
        );
        let output = compiled(&vmf, &game);
        assert_eq!(
            output.stats.water_regions,
            if mode == 0 { 1 } else { 2 },
            "mode {mode}"
        );
        let records = output.bsp.unwrap().records::<LeafWaterData>().unwrap();
        assert!(
            records
                .iter()
                .any(|record| record.min_z == 0.0 && record.surface_z == 64.0)
        );
        if mode == 2 {
            assert!(records.iter().any(|record| record.surface_z == 96.0));
        }
    }
}

#[test]
fn unsupported_water_surfaces_and_overlaps_fail_without_overwriting_previous_output() {
    let (_directory, game) = game();
    for material in ["water", "slime"] {
        for mode in 0..3 {
            let (mut vmf, id) = add_pool(
                common::room(),
                [-128.0, -128.0, 16.0],
                [128.0, 128.0, 64.0],
                material,
            );
            if mode == 0 {
                let mut document = Document::new(vmf);
                document
                    .edit("sloped water", |tx| {
                        edit::transform(
                            tx,
                            &[ObjectId::Solid(id)],
                            map_core::transform::Transform {
                                rotation: DVec3::new(0.0, 15.0, 0.0),
                                pivot: DVec3::new(0.0, 0.0, 40.0),
                                ..Default::default()
                            },
                        )
                    })
                    .unwrap();
                vmf = document.vmf().clone();
            } else if mode == 1 {
                let top = vmf
                    .world
                    .solids
                    .iter_mut()
                    .find(|solid| solid.id == id)
                    .unwrap()
                    .sides
                    .iter_mut()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap();
                top.material = "nodraw".into();
                for side in &mut vmf
                    .world
                    .solids
                    .iter_mut()
                    .find(|solid| solid.id == id)
                    .unwrap()
                    .sides
                {
                    if Plane::from_points(side.plane.0).unwrap().normal.x > 0.99 {
                        side.material = material.into();
                    }
                }
            } else {
                vmf = add_pool(vmf, [-64.0, -64.0, 16.0], [192.0, 192.0, 96.0], "water2").0;
            }
            let output = compiler::compile(&vmf, &game);
            assert!(
                output.report.has_errors(),
                "mode={mode}: {:?}",
                output.report
            );
            assert!(output.bsp.is_none());
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("previous.bsp");
            std::fs::write(&path, b"previous").unwrap();
            assert!(!output.write(&path).unwrap());
            assert_eq!(std::fs::read(path).unwrap(), b"previous");
        }
    }
}

#[test]
fn hidden_and_brush_model_liquids_do_not_receive_world_water_regions() {
    let (_directory, game) = game();
    for model in [false, true] {
        let (mut vmf, id) = add_pool(
            common::room(),
            [-128.0, -128.0, 16.0],
            [128.0, 128.0, 64.0],
            "water",
        );
        if model {
            let mut document = Document::new(vmf);
            document
                .edit("liquid submodel", |tx| {
                    edit::tie_to_entity(tx, &[id], "func_brush", &[])
                })
                .unwrap();
            vmf = document.vmf().clone();
        } else {
            vmf.world
                .solids
                .iter_mut()
                .find(|solid| solid.id == id)
                .unwrap()
                .hidden = true;
        }
        let output = compiled(&vmf, &game);
        assert_eq!(output.stats.water_regions, 0);
        assert_eq!(output.stats.water_leaves, 0);
        let bsp = output.bsp.unwrap();
        assert!(bsp.records::<LeafWaterData>().unwrap().is_empty());
        assert!(
            bsp.records::<Leaf>()
                .unwrap()
                .iter()
                .all(|leaf| leaf.water_data == -1)
        );
        if model {
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("brush-model fluid"))
            );
        }
    }
}

#[test]
fn water_analogs_fail_before_emission_and_preserve_previous_output() {
    let (_directory, game) = game();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    let pointfile = path.with_extension("lin");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    std::fs::write(&pointfile, b"previous pointfile").unwrap();
    for physics in [compiler::PhysicsMode::None, compiler::PhysicsMode::Engine] {
        for variant in [
            "empty",
            "dry",
            "liquid",
            "hidden brushes",
            "duplicate class",
        ] {
            let (mut vmf, id) = add_water_analog(
                common::room(),
                [-32.0, -32.0, 16.0],
                [32.0, 32.0, 64.0],
                if variant == "liquid" {
                    "water"
                } else {
                    "nodraw"
                },
            );
            let entity = vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == id)
                .unwrap();
            match variant {
                "empty" => entity.solids.clear(),
                "hidden brushes" => entity.solids[0].hidden = true,
                "duplicate class" => {
                    entity.properties.set("classname", "func_brush");
                    entity
                        .properties
                        .0
                        .push(("ClassName".into(), "func_water_analog".into()));
                }
                _ => {}
            }
            let before = vmf.clone();
            let output = compiler::compile_with_options(
                &vmf,
                &game,
                compiler::Options {
                    physics,
                    ..Default::default()
                },
            );
            assert!(
                output.report.has_errors(),
                "{physics:?}/{variant}: {:?}",
                output.report
            );
            assert!(output.bsp.is_none());
            assert!(output.leak.is_none());
            assert_eq!(output.stats.physics_models, 0);
            assert!(output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.level == compiler::Level::Error
                    && diagnostic.object.as_deref() == Some(format!("entity:{id}").as_str())
                    && diagnostic
                        .message
                        .contains("Cannot compile func_water_analog")
            }));
            assert!(!output.write(&path).unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), b"previous successful BSP");
            assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
            assert_eq!(vmf, before);
        }
    }
}

#[test]
fn hidden_water_analogs_do_not_block_otherwise_supported_maps() {
    let (_directory, game) = game();
    let baseline = compiled(&common::room(), &game).bsp.unwrap().to_bytes();
    let (mut vmf, id) = add_water_analog(
        common::room(),
        [-32.0, -32.0, 16.0],
        [32.0, 32.0, 64.0],
        "water",
    );
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .hidden = true;
    assert_eq!(compiled(&vmf, &game).bsp.unwrap().to_bytes(), baseline);
}

#[test]
fn region_omitted_water_analogs_do_not_block_compilation() {
    let (directory, game) = game();
    std::fs::create_dir(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric {}",
    )
    .unwrap();
    let (mut vmf, id) = add_water_analog(
        common::room(),
        [992.0, -32.0, 32.0],
        [1056.0, 32.0, 96.0],
        "water",
    );
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("origin", "1024 0 64");
    let mut document = Document::new(vmf);
    document
        .edit("compile region", |tx| {
            map_core::compile_region::apply(
                tx,
                Some(map_core::compile_region::Definition {
                    enabled: true,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 128.0),
                    },
                }),
            )
        })
        .unwrap();
    let before = document.vmf().clone();
    let output = compiled(document.vmf(), &game);
    assert!(
        output
            .stats
            .compile_region
            .unwrap()
            .omitted
            .contains(&format!("entity:{id}"))
    );
    assert!(
        !output
            .bsp
            .unwrap()
            .entities()
            .unwrap()
            .to_ascii_lowercase()
            .contains("func_water_analog")
    );
    assert_eq!(document.vmf(), &before);
}

#[test]
fn authored_water_lod_controls_are_preserved_and_not_duplicated() {
    let (_directory, game) = game();
    let (vmf, _) = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        "water",
    );
    let mut document = Document::new(vmf);
    document
        .edit("water lod", |tx| {
            edit::create_entity(
                tx,
                "water_lod_control",
                None,
                &[
                    ("cheapwaterstartdistance".into(), "123".into()),
                    ("cheapwaterenddistance".into(), "456".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let output = compiled(document.vmf(), &game);
    let bsp = output.bsp.unwrap();
    let text = bsp.entities().unwrap();
    assert_eq!(
        text.matches("\"classname\" \"water_lod_control\"").count(),
        1
    );
    assert!(text.contains("\"cheapwaterstartdistance\" \"123\""));
    assert!(text.contains("\"cheapwaterenddistance\" \"456\""));
}

#[test]
fn engine_fluids_use_connected_regions_and_clipped_wet_leaf_hulls() {
    let Some(root) = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|path| path.join(".local/share/Steam/steamapps/common/Counter-Strike Source"))
        .filter(|path| path.join("bin/linux64/vphysics.so").is_file())
    else {
        eprintln!("CS:S not installed; skipping native fluid fixture");
        return;
    };
    let (directory, mut fs) = game();
    fs.base_dir = root;
    std::fs::create_dir(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric {}",
    )
    .unwrap();
    for (first, second, controlled, regions, regional) in [
        ("water", "water", false, 1, false),
        ("slime", "slime", false, 1, false),
        ("water", "slime", true, 2, false),
        ("slime", "slime", true, 1, true),
    ] {
        let (vmf, _) = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [0.0, 128.0, 64.0],
            first,
        );
        let (vmf, _) = add_pool(vmf, [0.0, -128.0, 0.0], [128.0, 128.0, 64.0], second);
        let mut vmf = vmf;
        if controlled {
            vmf.world
                .properties
                .set(compiler::WATER_CURRENT_VELOCITY_KEY, "64 -32 16");
            vmf.world
                .properties
                .set(compiler::WATER_DAMPING_KEY, "0.005");
        }
        let mut document = Document::new(vmf);
        document
            .edit("column in fluid", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 0.0),
                    DVec3::new(16.0, 16.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?;
                if regional {
                    compile_region::apply(
                        tx,
                        Some(compile_region::Definition {
                            enabled: true,
                            bounds: compile_region::Bounds {
                                mins: DVec3::new(-192.0, -192.0, 0.0),
                                maxs: DVec3::new(192.0, 192.0, 192.0),
                            },
                        }),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let before = document.vmf().write(&Default::default());
        let output = compiler::compile_with_options(
            document.vmf(),
            &fs,
            compiler::Options {
                physics: compiler::PhysicsMode::Engine,
                ..compiler::Options::default()
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.water_regions, regions);
        assert_eq!(output.stats.physics_fluids, regions);
        assert_eq!(output.stats.physics_solids, regions + 1);
        assert_eq!(output.stats.compile_region.is_some(), regional);
        let bsp = output.bsp.unwrap();
        let physics = bsp.physics().unwrap().unwrap();
        let keys = std::str::from_utf8(physics.models[0].key_data).unwrap();
        let nodes = source_formats::keyvalues::parse(&keys[..keys.len() - 1]).unwrap();
        let fluids = nodes
            .iter()
            .filter(|node| node.key == "fluid")
            .collect::<Vec<_>>();
        assert_eq!(fluids.len(), regions);
        let mut properties = Vec::new();
        for node in fluids {
            let fluid = node.children().unwrap();
            let field = |key| {
                fluid
                    .iter()
                    .find(|node| node.key == key)
                    .unwrap()
                    .as_str()
                    .unwrap()
            };
            let property = field("surfaceprop");
            assert_eq!(
                field("contents"),
                match property {
                    "water" => "32",
                    "slime" => "16",
                    other => panic!("Unexpected fluid property {other}"),
                }
            );
            properties.push(property);
            assert_eq!(field("damping"), if controlled { "0.005" } else { "0.01" });
            assert_eq!(field("surfaceplane"), "0 0 1 64");
            assert_eq!(
                field("currentvelocity"),
                if controlled {
                    "1.6256 -0.8128 0.4064"
                } else {
                    "0 0 0"
                }
            );
        }
        properties.sort_unstable();
        assert_eq!(
            properties,
            if first != second {
                vec!["slime", "water"]
            } else {
                vec![first]
            }
        );
        assert_eq!(physics.models[0].solids.len(), regions + 1);
        let leaves = bsp.records::<Leaf>().unwrap();
        let wet = leaves.iter().filter(|leaf| leaf.water_data >= 0).count();
        assert!(wet > 2);
        assert_eq!(
            output.stats.physics_convexes,
            if regional { 8 + wet } else { 7 + wet }
        );
        assert_eq!(document.vmf().write(&Default::default()), before);
    }
}
