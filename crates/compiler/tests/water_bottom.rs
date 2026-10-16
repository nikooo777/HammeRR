mod common;
#[path = "common/overlays.rs"]
mod overlay_fixture;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, Leaf, LeafWaterData, TexInfo, lump};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const TOP: &str = "water/top";
const BOTTOM: &str = "WATER/BELOW";
const SIDE: &str = "water/side";
const SIDE_BOTTOM: &str = "WATER/SIDE_BELOW";
const SIDE_TEXT: &str = "Water { $abovewater 1 $bottommaterial water/side_below }";

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/water");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, text) in [
        (
            "top",
            "Water { $abovewater 1 $bottommaterial water/below $underwateroverlay effects/water_warp01 }",
        ),
        (
            "top_patch",
            "Patch { include materials/water/top.vmt replace { $bottommaterial Materials\\water\\below.vmt } }",
        ),
        (
            "below",
            "Patch { include materials/water/below_base.vmt replace { $abovewater 0 } }",
        ),
        ("below_base", "Water { $abovewater 1 }"),
        ("side", SIDE_TEXT),
        ("side_below", "Water { $abovewater 0 }"),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), text).unwrap();
    }
    std::fs::write(
        directory.path().join("materials/nodraw.vmt"),
        "UnlitGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    (directory, game)
}

fn add_pool(vmf: Vmf, mins: [f64; 3], maxs: [f64; 3], material: &str) -> Vmf {
    let mut document = Document::new(vmf);
    document
        .edit("water surface", |tx| {
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
                    side.u_axis.offset = 17.0;
                    side.v_axis.offset = -13.0;
                    side.u_axis.scale = 0.75;
                    side.v_axis.scale = 0.5;
                }
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn compiled(vmf: &Vmf, game: &GameFs, physics: compiler::PhysicsMode) -> Bsp {
    let output = compiler::compile_with_options(
        vmf,
        game,
        compiler::Options {
            physics,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap()
}

fn paint_pool_side(vmf: &mut Vmf, normal: DVec3) {
    for side in &mut vmf.world.solids.last_mut().unwrap().sides {
        if Plane::from_points(side.plane.0)
            .unwrap()
            .normal
            .abs_diff_eq(normal, 1e-8)
        {
            side.material = SIDE.into();
            side.u_axis.offset = 31.0;
            side.v_axis.offset = -29.0;
            side.u_axis.scale = 0.25;
            side.v_axis.scale = -0.75;
        }
    }
}

fn material_faces(bsp: &Bsp, name: &str) -> Vec<Face> {
    let infos = bsp.records::<TexInfo>().unwrap();
    let data = bsp.records::<bsp::TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    bsp.records::<Face>()
        .unwrap()
        .into_iter()
        .filter(|face| {
            names[data[infos[face.texinfo as usize].texdata as usize].name_index as usize]
                .eq_ignore_ascii_case(name)
        })
        .collect()
}

fn face_points(bsp: &Bsp, face: &Face) -> Vec<DVec3> {
    let vertices = bsp.values::<[f32; 3]>(lump::VERTEXES).unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.values::<i32>(lump::SURFEDGES).unwrap();
    surfedges[face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        .iter()
        .map(|&edge| {
            let vertex = edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)];
            DVec3::from_array(vertices[vertex as usize].map(f64::from))
        })
        .collect()
}

#[test]
fn water_underside_uses_patched_bottom_material_and_keeps_top_projection_flags_and_fog() {
    let (_directory, game) = game();
    for top in [TOP, "water/top_patch"] {
        let vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [128.0, 128.0, 64.0],
            top,
        );
        let source = vmf.clone();
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        let tops = material_faces(&bsp, top);
        let bottoms = material_faces(&bsp, BOTTOM);
        assert_eq!(tops.len(), 1);
        assert_eq!(bottoms.len(), 1);
        let infos = bsp.records::<TexInfo>().unwrap();
        let top_info = infos[tops[0].texinfo as usize];
        let bottom_info = infos[bottoms[0].texinfo as usize];
        assert_eq!(top_info.texture_vecs, bottom_info.texture_vecs);
        assert_eq!(top_info.lightmap_vecs, bottom_info.lightmap_vecs);
        assert_eq!(top_info.flags, bottom_info.flags);
        assert_ne!(top_info.texdata, bottom_info.texdata);
        let planes = bsp.records::<bsp::Plane>().unwrap();
        assert_eq!(planes[tops[0].plane as usize].normal, [0.0, 0.0, 1.0]);
        assert_eq!(planes[bottoms[0].plane as usize].normal, [0.0, 0.0, -1.0]);
        for (face, normal) in [(&tops[0], DVec3::Z), (&bottoms[0], DVec3::NEG_Z)] {
            let points = face_points(&bsp, face);
            assert!(points.iter().all(|point| point.z == 64.0));
            assert!(
                (points[2] - points[0])
                    .cross(points[1] - points[0])
                    .dot(normal)
                    > 0.0
            );
            assert_eq!(face.fog_volume, 0);
            assert_eq!(face.light_offset, -1);
        }
        let water = bsp.records::<LeafWaterData>().unwrap();
        assert_eq!(water.len(), 1);
        assert_eq!(water[0].surface_texinfo, tops[0].texinfo);
        assert_eq!((water[0].surface_z, water[0].min_z), (64.0, 0.0));
        let leaves = bsp.records::<Leaf>().unwrap();
        let leaf_faces = bsp.values::<u16>(lump::LEAFFACES).unwrap();
        let all_faces = bsp.records::<Face>().unwrap();
        let top_id = all_faces.iter().position(|face| face == &tops[0]).unwrap() as u16;
        let bottom_id = all_faces
            .iter()
            .position(|face| face == &bottoms[0])
            .unwrap() as u16;
        let mut top_refs = 0;
        let mut bottom_refs = 0;
        for leaf in leaves {
            let refs = &leaf_faces[leaf.first_leaf_face as usize
                ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize];
            if refs.contains(&top_id) {
                assert_eq!(leaf.contents & bsp::contents::WATER, 0);
                top_refs += 1;
            }
            if refs.contains(&bottom_id) {
                assert_ne!(leaf.contents & bsp::contents::WATER, 0);
                assert_eq!(leaf.water_data, 0);
                bottom_refs += 1;
            }
        }
        assert!(top_refs > 0 && bottom_refs > 0);
        assert_eq!(all_faces, bsp.values::<Face>(lump::ORIGINALFACES).unwrap());
        assert_eq!(vmf, source);
    }
}

#[test]
fn connected_different_depths_keep_one_region_and_do_not_add_an_internal_water_backside() {
    let (_directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [0.0, 128.0, 64.0],
        TOP,
    );
    let vmf = add_pool(vmf, [0.0, -128.0, 32.0], [128.0, 128.0, 64.0], TOP);
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap().len(), 1);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap()[0].min_z, 0.0);
    let area: f32 = material_faces(&bsp, BOTTOM)
        .iter()
        .map(|face| face.area)
        .sum();
    assert_eq!(area, 256.0 * 256.0);

    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 32.0],
        TOP,
    );
    let vmf = add_pool(vmf, [-128.0, -128.0, 32.0], [128.0, 128.0, 64.0], TOP);
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let bottom_faces = material_faces(&bsp, BOTTOM);
    assert!(!bottom_faces.is_empty());
    assert!(
        bottom_faces
            .iter()
            .all(|face| face_points(&bsp, face).iter().all(|point| point.z == 64.0))
    );
}

#[test]
fn missing_bottom_key_warns_and_explicit_bad_bottom_assets_preserve_prior_output() {
    let (directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    let source = vmf.clone();
    let top = directory.path().join("materials/water/top.vmt");
    std::fs::write(&top, "Water { $abovewater 1 }").unwrap();
    let output = compiler::compile(&vmf, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("no $bottommaterial"))
    );
    assert!(material_faces(&output.bsp.unwrap(), BOTTOM).is_empty());
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous successful output").unwrap();
    for bottom in ["water/missing", "water/broken", "../outside", "water/cycle"] {
        std::fs::write(
            &top,
            format!("Water {{ $abovewater 1 $bottommaterial {bottom} }}"),
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/water/broken.vmt"),
            "Water { $abovewater }",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/water/cycle.vmt"),
            "Patch { include materials/water/cycle.vmt }",
        )
        .unwrap();
        let output = compiler::compile(&vmf, &game);
        assert!(
            output.report.has_errors(),
            "bottom {bottom}: {:?}",
            output.report
        );
        assert!(output.bsp.is_none());
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful output"
        );
        assert_eq!(vmf, source);
    }
}

#[test]
fn underside_materials_leave_collision_and_water_metadata_bytes_unchanged() {
    let (directory, mut game) = game();
    let mut vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    paint_pool_side(&mut vmf, DVec3::NEG_Y);
    let native = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|path| path.join(".local/share/Steam/steamapps/common/Counter-Strike Source"))
        .filter(|path| path.join("bin/linux64/vphysics.so").is_file());
    let mut modes = vec![compiler::PhysicsMode::None];
    if let Some(root) = native {
        game.base_dir = root;
        modes.push(compiler::PhysicsMode::Engine);
    }
    for physics in modes {
        let top = directory.path().join("materials/water/top.vmt");
        let side = directory.path().join("materials/water/side.vmt");
        std::fs::write(&top, "Water { $abovewater 1 }").unwrap();
        std::fs::write(&side, "Water { $abovewater 1 }").unwrap();
        let previous = compiled(&vmf, &game, physics);
        std::fs::write(&top, "Water { $abovewater 1 $bottommaterial water/below }").unwrap();
        std::fs::write(&side, SIDE_TEXT).unwrap();
        let current = compiled(&vmf, &game, physics);
        for lump in [
            lump::LEAFWATERDATA,
            lump::LEAFMINDISTTOWATER,
            lump::BRUSHES,
            lump::BRUSHSIDES,
            lump::PHYSCOLLIDE,
        ] {
            assert_eq!(
                previous.lumps[lump].data,
                current.lumps[lump].data,
                "lump {lump}, physics {}",
                physics.name()
            );
        }
    }
}

#[test]
fn shared_bottom_material_keeps_distinct_top_regions_and_omits_hidden_or_model_water() {
    let (directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [0.0, 128.0, 64.0],
        TOP,
    );
    let vmf = add_pool(
        vmf,
        [0.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        "water/top_patch",
    );
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    assert_eq!(bsp.records::<LeafWaterData>().unwrap().len(), 2);
    let bottom = material_faces(&bsp, BOTTOM);
    assert_eq!(bottom.len(), 2);
    assert_ne!(bottom[0].fog_volume, bottom[1].fog_volume);

    std::fs::write(
        directory.path().join("materials/water/below.vmt"),
        "Water { $abovewater }",
    )
    .unwrap();
    for model in [false, true] {
        let mut vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [128.0, 128.0, 64.0],
            TOP,
        );
        let pool = vmf.world.solids.last_mut().unwrap();
        if model {
            let id = pool.id;
            let mut document = Document::new(vmf);
            document
                .edit("model water", |tx| {
                    edit::tie_to_entity(tx, &[id], "func_brush", &[])
                })
                .unwrap();
            vmf = document.vmf().clone();
        } else {
            pool.hidden = true;
        }
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        assert!(material_faces(&bsp, BOTTOM).is_empty());
        assert!(bsp.records::<LeafWaterData>().unwrap().is_empty());
    }
}

#[test]
fn authored_top_overlay_does_not_bind_the_generated_bottom_face() {
    let (_directory, game) = game();
    let vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    let top = vmf
        .world
        .solids
        .last()
        .unwrap()
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let mut vmf = overlay_fixture::add_overlay(vmf, &[top]);
    vmf.entities
        .last_mut()
        .unwrap()
        .properties
        .set("BasisOrigin", "0 0 64");
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let faces = bsp.records::<Face>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let overlays = bsp.overlay_data().unwrap().unwrap();
    assert_eq!(overlays.overlays.len(), 1);
    let overlay = overlays.overlays[0];
    assert_eq!(overlay.face_count(), material_faces(&bsp, TOP).len());
    assert!(
        overlay.faces[..overlay.face_count()]
            .iter()
            .all(|&face| { planes[faces[face as usize].plane as usize].normal == [0.0, 0.0, 1.0] })
    );
    assert!(!material_faces(&bsp, BOTTOM).is_empty());
}

#[test]
fn touching_solid_and_water_covers_clip_only_the_covered_underside_area() {
    let (_directory, game) = game();
    for liquid_cover in [false, true] {
        for low in [-32.0, 48.0] {
            let high = low + 64.0;
            let mut vmf = add_pool(
                common::room(),
                [-128.0, -128.0, 0.0],
                [128.0, 128.0, 64.0],
                TOP,
            );
            if liquid_cover {
                vmf = add_pool(vmf, [low, low, 64.0], [high, high, 96.0], TOP);
            } else {
                let mut document = Document::new(vmf);
                document
                    .edit("touching water cover", |tx| {
                        edit::create_block(
                            tx,
                            DVec3::new(low, low, 64.0),
                            DVec3::new(high, high, 96.0),
                            "dev/dev_measuregeneric01",
                        )
                    })
                    .unwrap();
                vmf = document.vmf().clone();
            }
            let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
            let below = material_faces(&bsp, BOTTOM);
            let mut lower_area = 0.0;
            let mut upper_area = 0.0;
            for face in below {
                let points = face_points(&bsp, &face);
                if points.iter().all(|point| point.z == 64.0) {
                    lower_area += face.area;
                    assert!(
                        points.iter().all(|point| point.x <= low)
                            || points.iter().all(|point| point.x >= high)
                            || points.iter().all(|point| point.y <= low)
                            || points.iter().all(|point| point.y >= high)
                    );
                } else {
                    assert!(liquid_cover);
                    assert!(points.iter().all(|point| point.z == 96.0));
                    upper_area += face.area;
                }
            }
            assert_eq!(
                lower_area,
                256.0 * 256.0 - 64.0 * 64.0,
                "liquid={liquid_cover}, low={low}"
            );
            assert_eq!(upper_area, if liquid_cover { 64.0 * 64.0 } else { 0.0 });
        }
    }
}

#[test]
fn shared_top_and_bottom_texinfo_does_not_generate_a_backside_for_the_authored_bottom() {
    let (_directory, game) = game();
    let mut document = Document::new(common::room());
    let ObjectId::Solid(id) = document
        .edit("all-water slab", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 32.0),
                DVec3::new(128.0, 128.0, 64.0),
                TOP,
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    let map =
        compiler::map::Map::load(document.vmf(), &game, &mut compiler::Report::default()).unwrap();
    let brush = map.brushes.iter().find(|brush| brush.id == id).unwrap();
    let top = brush
        .sides
        .iter()
        .find(|side| map.planes.get(side.plane).normal == DVec3::Z)
        .unwrap();
    let bottom = brush
        .sides
        .iter()
        .find(|side| map.planes.get(side.plane).normal == DVec3::NEG_Z)
        .unwrap();
    assert_eq!(top.texinfo, bottom.texinfo);
    let bsp = compiled(document.vmf(), &game, compiler::PhysicsMode::None);
    let below = material_faces(&bsp, BOTTOM);
    assert!(!below.is_empty());
    let planes = bsp.records::<bsp::Plane>().unwrap();
    assert!(
        below
            .iter()
            .any(|face| planes[face.plane as usize].normal[2] != 0.0)
    );
    for face in below {
        let normal = planes[face.plane as usize].normal;
        if normal[2] != 0.0 {
            assert_eq!(normal, [0.0, 0.0, -1.0]);
            assert!(face_points(&bsp, &face).iter().all(|point| point.z == 64.0));
        }
    }
}

#[test]
fn vertical_water_faces_use_their_own_material_and_clip_touching_solid_and_wet_covers() {
    let (_directory, game) = game();
    let mut vmf = add_pool(
        common::room(),
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 64.0],
        TOP,
    );
    paint_pool_side(&mut vmf, DVec3::NEG_Y);
    let mut document = Document::new(vmf);
    document
        .edit("touching vertical water cover", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -160.0, 0.0),
                DVec3::new(32.0, -128.0, 64.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let vmf = add_pool(
        document.vmf().clone(),
        [64.0, -160.0, 0.0],
        [96.0, -128.0, 32.0],
        TOP,
    );
    let source = vmf.clone();
    let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
    let inward = material_faces(&bsp, SIDE_BOTTOM);
    assert!(!inward.is_empty());
    assert_eq!(
        inward.iter().map(|face| face.area).sum::<f32>(),
        256.0 * 64.0 - 64.0 * 64.0 - 32.0 * 32.0
    );
    let infos = bsp.records::<TexInfo>().unwrap();
    let outward = material_faces(&bsp, SIDE);
    assert_eq!(
        outward.iter().map(|face| face.area).sum::<f32>(),
        inward.iter().map(|face| face.area).sum::<f32>()
    );
    let side_info = infos[outward[0].texinfo as usize];
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let water = bsp.records::<LeafWaterData>().unwrap();
    let region = water
        .iter()
        .position(|region| region.surface_z == 64.0)
        .unwrap() as i16;
    for face in inward {
        assert_eq!(planes[face.plane as usize].normal, [0.0, 1.0, 0.0]);
        assert_eq!(face.fog_volume, region);
        let info = infos[face.texinfo as usize];
        assert_eq!(info.texture_vecs, side_info.texture_vecs);
        assert_eq!(info.lightmap_vecs, side_info.lightmap_vecs);
        assert_eq!(info.flags, side_info.flags);
        assert_ne!(info.texdata, side_info.texdata);
        let points = face_points(&bsp, &face);
        assert!(points.iter().all(|point| point.y == -128.0));
        assert!(
            (points[2] - points[0])
                .cross(points[1] - points[0])
                .dot(DVec3::Y)
                > 0.0
        );
        assert!(
            points.iter().all(|point| point.x <= -32.0)
                || points.iter().all(|point| point.x >= 32.0)
        );
        assert!(
            points.iter().all(|point| point.x <= 64.0)
                || points.iter().all(|point| point.x >= 96.0)
                || points.iter().all(|point| point.z >= 32.0)
        );
    }
    assert_eq!(vmf, source);
}

#[test]
fn shared_vertical_texinfo_keeps_generated_faces_in_their_own_wet_regions() {
    let (_directory, game) = game();
    for (height, top) in [(64.0, "water/top_patch"), (96.0, TOP)] {
        let mut vmf = add_pool(
            common::room(),
            [-128.0, -128.0, 0.0],
            [0.0, 128.0, 64.0],
            TOP,
        );
        paint_pool_side(&mut vmf, DVec3::NEG_Y);
        paint_pool_side(&mut vmf, DVec3::X);
        let first = vmf.world.solids.last().unwrap().id;
        let mut vmf = add_pool(vmf, [0.0, -128.0, 0.0], [128.0, 128.0, height], top);
        paint_pool_side(&mut vmf, DVec3::NEG_Y);
        paint_pool_side(&mut vmf, DVec3::NEG_X);
        let second = vmf.world.solids.last().unwrap().id;
        let map = compiler::map::Map::load(&vmf, &game, &mut compiler::Report::default()).unwrap();
        let front_infos = [first, second].map(|id| {
            map.brushes
                .iter()
                .find(|brush| brush.id == id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map.planes.get(side.plane).normal == DVec3::NEG_Y)
                .unwrap()
                .texinfo
        });
        assert_eq!(front_infos[0], front_infos[1]);
        let bsp = compiled(&vmf, &game, compiler::PhysicsMode::None);
        let records = bsp.records::<LeafWaterData>().unwrap();
        assert_eq!(records.len(), 2);
        let infos = bsp.records::<TexInfo>().unwrap();
        let texdata = bsp.records::<bsp::TexData>().unwrap();
        let names = bsp.texdata_strings().unwrap();
        let region_names: Vec<_> = records
            .iter()
            .map(|region| {
                names[texdata[infos[region.surface_texinfo as usize].texdata as usize].name_index
                    as usize]
                    .as_str()
            })
            .collect();
        let planes = bsp.records::<bsp::Plane>().unwrap();
        let leaves = bsp.records::<Leaf>().unwrap();
        let leaf_faces = bsp.values::<u16>(lump::LEAFFACES).unwrap();
        let all_faces = bsp.records::<Face>().unwrap();
        for (material, wet) in [(SIDE_BOTTOM, true), (SIDE, false)] {
            let mut areas = [0.0; 2];
            for face in material_faces(&bsp, material) {
                let normal = planes[face.plane as usize].normal;
                if normal[0] != 0.0 {
                    assert!(height > 64.0);
                    assert_eq!(normal, [if wet { 1.0 } else { -1.0 }, 0.0, 0.0]);
                    assert_eq!(records[face.fog_volume as usize].surface_z, height as f32);
                    assert!(face_points(&bsp, &face).iter().all(|point| point.z >= 64.0));
                    continue;
                }
                assert_eq!(normal, [0.0, if wet { 1.0 } else { -1.0 }, 0.0]);
                let region = face.fog_volume as usize;
                assert!(region < records.len());
                let points = face_points(&bsp, &face);
                let first_region = if height == 64.0 {
                    region_names[region].eq_ignore_ascii_case(TOP)
                } else {
                    records[region].surface_z == 64.0
                };
                assert!(
                    points
                        .iter()
                        .all(|point| point.y == -128.0
                            && point.z <= records[region].surface_z as f64)
                );
                assert!(points.iter().all(|point| if first_region {
                    point.x <= 0.0
                } else {
                    point.x >= 0.0
                }));
                let id = all_faces
                    .iter()
                    .position(|candidate| candidate == &face)
                    .unwrap() as u16;
                let references: Vec<_> = leaves
                    .iter()
                    .filter(|leaf| {
                        leaf_faces[leaf.first_leaf_face as usize
                            ..leaf.first_leaf_face as usize + leaf.leaf_face_count as usize]
                            .contains(&id)
                    })
                    .collect();
                assert!(!references.is_empty());
                assert!(
                    references
                        .iter()
                        .all(|leaf| (leaf.contents & bsp::contents::WATER != 0) == wet
                            && (!wet || leaf.water_data == face.fog_volume))
                );
                areas[usize::from(!first_region)] += face.area;
            }
            assert_eq!(areas, [128.0 * 64.0, 128.0 * height as f32]);
        }
    }
}
